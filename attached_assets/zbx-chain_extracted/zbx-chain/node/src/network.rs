//! P2P TCP network server — Status handshake, block sync, vote propagation, TX relay.
//!
//! ## Architecture
//!
//! ```text
//! NetworkServer (Arc-shared)
//!   ├─ TCP listener              — accepts inbound connections
//!   ├─ Bootnode dialer           — initiates outbound connections (with reconnect)
//!   ├─ TX relay task             — reads accepted TXes from RPC, broadcasts to peers
//!   └─ Per-peer tasks
//!        ├─ reader task          — recv_msg loop → handle_message dispatch
//!        └─ writer task          — drains UnboundedReceiver<Message> → TCP write half
//! ```
//!
//! ## Key integration points
//!
//! * `broadcast_block(block)` — ConsensusDriver::do_commit → all TCP peers.
//! * `broadcast_vote(vote)`   — ConsensusDriver::process_events VoteCast → all TCP peers.
//! * Received `Message::Vote` → injected into ConsensusDriver via `consensus_vote_tx`.
//! * Received `Message::Block/Blocks` → `execute_and_commit` (non-validator sync).
//! * `eth_sendRawTransaction` → `tx_relay_tx` broadcast → this server → `Message::Transaction`
//!   to all peers → peers add to their mempool (multi-validator TX propagation).
//! * `Message::FindPeers` → respond with `Message::Peers(known_addrs)`.
//! * `Message::Peers(addrs)` → spawn independent dial tasks for each unknown address.
//! * Bootnode connections use exponential-backoff reconnect on disconnect.
//!
//! ## Recursion avoidance
//!
//! All methods that can eventually call each other (`dial_peer` → `handle_connection`
//! → `handle_message` → peer-dial) use `Arc<Self>` receivers and break any back-edges
//! by spawning independent Tokio tasks instead of awaiting inline.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, error, info, trace, warn};

use zbx_consensus::Vote;
use zbx_crypto::bls::BlsPubKey;
use zbx_mempool::TransactionPool;
use zbx_network::gossip::{GossipDecision, GossipMessage, GossipRouter, GossipTopic, Subscriptions};
use zbx_network::messages::{GossipEnvelope, GetBlockRange, StatusMessage};
use zbx_network::peer::{PeerId, PeerInfo, PeerManager};
use zbx_network::peer_score::{
    PeerScorer, ScorePenalty, VALID_BLOCK_REWARD, VALID_MESSAGE_REWARD, VALID_QC_REWARD,
};
use zbx_network::Message;
use zbx_storage::ZbxDb;
use zbx_types::{address::Address, block::Block, transaction::SignedTransaction, H256};

use crate::block_producer::execute_and_commit;
use crate::consensus::InboundVote;

const PROTOCOL_VERSION: u8 = 1;
const MAX_BLOCK_RANGE: u64 = 64;
const MAX_MSG_BYTES: usize = 16 * 1024 * 1024; // 16 MiB
const BOOTNODE_RECONNECT_BASE: Duration = Duration::from_secs(5);
const BOOTNODE_RECONNECT_MAX: Duration = Duration::from_secs(120);
/// Maximum peers we will spontaneously dial from Peers messages.
const MAX_DIAL_PEERS: usize = 50;
/// SEC-2026-05-09 (P3): hard timeout on handshake — peers that send a
/// well-formed length prefix but then stall indefinitely on the body used
/// to be able to pin a tokio task forever, exhausting accept slots.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// SEC-2026-05-09 (P3): hard timeout on a single inbound message. Same
/// rationale as handshake — caps the time any single peer can hold a read.
const MSG_READ_TIMEOUT: Duration = Duration::from_secs(60);
/// SEC-2026-05-09 (P4): bounded per-peer outbound queue. The previous
/// `unbounded_channel` allowed a single slow peer's queue to grow without
/// limit, OOMing the node. With a bounded queue we apply backpressure and
/// drop the slowest peer instead.
const PEER_OUTBOUND_QUEUE: usize = 1024;
/// Maximum inbound messages a peer may send within RATE_WINDOW_SECS before
/// being penalised for spam. This prevents CPU-exhaustion floods from a
/// single peer.
const PEER_MSG_RATE_LIMIT: u32 = 500;
/// Duration of the rate-limit sliding window.
const RATE_WINDOW_SECS: u64 = 10;
/// Interval between Ping keepalive messages sent to every connected peer.
/// Responses (Pong) are used to update round-trip latency in PeerManager
/// and PeerScorer.
const PING_INTERVAL: Duration = Duration::from_secs(30);
/// Maximum block headers served in a single GetHeaders response (fast-sync).
const MAX_HEADERS_PER_RESP: u32 = 192;

// ---------------------------------------------------------------------------
// Wire framing
// ---------------------------------------------------------------------------

// SEC-2026-05-09 (P1): cleartext send_msg / recv_msg are kept ONLY for the
// pre-handshake Noise phase. Every byte after the Noise XX handshake goes
// through `crate::noise::send_encrypted` / `recv_encrypted` instead.
#[allow(dead_code)]
async fn send_msg(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    msg: &Message,
) -> Result<(), String> {
    let encoded = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    let len = encoded.len() as u32;
    writer
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer
        .write_all(&encoded)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[allow(dead_code)]
async fn recv_msg(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
) -> Result<Message, String> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| e.to_string())?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MSG_BYTES {
        return Err(format!("message too large: {len} bytes"));
    }
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

/// SEC-2026-05-09 (P1+P2+P3): full handshake — Noise XX **then** Status
/// (the Status message is now encrypted under the established Noise
/// transport). Returns the peer's `StatusMessage` plus the live
/// `NoiseSession` (cryptographic peer id + AEAD transport state).
async fn handshake(
    stream:       &mut TcpStream,
    our_status:   &StatusMessage,
    local_static: &crate::noise::NoiseStaticKey,
    is_initiator: bool,
) -> Result<(StatusMessage, crate::noise::NoiseSession), String> {
    // P3: hard timeout covers BOTH the Noise XX exchange and the encrypted
    // Status round-trip so a peer can't slow-loris either phase.
    tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        // ── Phase 1: Noise XX handshake ────────────────────────────────────
        let session = if is_initiator {
            crate::noise::handshake_initiator(stream, local_static).await
        } else {
            crate::noise::handshake_responder(stream, local_static).await
        }
        .map_err(|e| format!("noise handshake: {e}"))?;

        // ── Phase 2: encrypted Status exchange ─────────────────────────────
        let encoded = serde_json::to_vec(&Message::Status(our_status.clone()))
            .map_err(|e| e.to_string())?;
        if encoded.len() > MAX_MSG_BYTES {
            return Err("local Status message too large".to_string());
        }
        crate::noise::send_encrypted(stream, &session.transport, &encoded)
            .await
            .map_err(|e| format!("send Status: {e}"))?;

        let buf = crate::noise::recv_encrypted(stream, &session.transport)
            .await
            .map_err(|e| format!("recv Status: {e}"))?;
        if buf.len() > MAX_MSG_BYTES {
            return Err("peer Status message too large".to_string());
        }
        let status = match serde_json::from_slice::<Message>(&buf) {
            Ok(Message::Status(s)) => s,
            Ok(other) => return Err(format!(
                "expected Status, got {:?}",
                other.message_type()
            )),
            Err(e) => return Err(e.to_string()),
        };
        Ok((status, session))
    })
    .await
    .map_err(|_| format!("handshake timed out after {:?}", HANDSHAKE_TIMEOUT))?
}

// ---------------------------------------------------------------------------
// NetworkServer
// ---------------------------------------------------------------------------

pub struct NetworkServer {
    pub chain_id: u64,
    pub listen_port: u16,
    pub bootnodes: Vec<String>,
    storage: Arc<ZbxDb>,
    mempool: Arc<RwLock<TransactionPool>>,
    peer_manager: Arc<RwLock<PeerManager>>,
    /// Per-peer outbound message queues.
    /// SEC-2026-05-09 (P4): bounded per-peer outbound queue.
    peer_senders: Mutex<HashMap<PeerId, mpsc::Sender<Message>>>,
    /// Inject remote BLS votes into the ConsensusDriver.
    consensus_vote_tx: broadcast::Sender<InboundVote>,
    /// Address → BLS pubkey for known validators (vote attribution).
    validator_pubkeys: HashMap<Address, BlsPubKey>,
    /// Peer count mirror for `net_peerCount` / `eth_syncing`.
    pub peer_count: Arc<RwLock<u64>>,
    /// RPC TX relay channel — network server subscribes and broadcasts to peers.
    tx_relay_tx: Arc<broadcast::Sender<SignedTransaction>>,
    /// SEC-2026-05-09 (P1+P2): long-lived Noise XX static keypair used for
    /// transport encryption and as the basis for our cryptographic PeerId.
    pub noise_static: Arc<crate::noise::NoiseStaticKey>,
    /// Peer reputation scorer: tracks valid/invalid message counts, applies
    /// penalties for misbehaviour, and bans peers that cross the threshold.
    peer_scorer: Arc<RwLock<PeerScorer>>,
    /// Gossip deduplication + fan-out router. All inbound GossipMsg envelopes
    /// are run through this before relay so the node never forwards duplicates
    /// and always respects TTL.
    gossip_router: Arc<RwLock<GossipRouter>>,
    /// In-flight Ping nonces: peer → (nonce, send_instant).
    /// Used to measure round-trip latency when the Pong arrives.
    ping_timestamps: Mutex<HashMap<PeerId, (u64, std::time::Instant)>>,
    /// Per-peer inbound message rate limiter: peer → (msg_count, window_start).
    /// Resets every RATE_WINDOW_SECS. Peers that exceed PEER_MSG_RATE_LIMIT
    /// receive a SpamMessage penalty and are disconnected if their score drops
    /// to the ban threshold.
    peer_msg_rate: Mutex<HashMap<PeerId, (u32, std::time::Instant)>>,
}

impl NetworkServer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chain_id: u64,
        listen_port: u16,
        bootnodes: Vec<String>,
        storage: Arc<ZbxDb>,
        mempool: Arc<RwLock<TransactionPool>>,
        peer_manager: Arc<RwLock<PeerManager>>,
        consensus_vote_tx: broadcast::Sender<InboundVote>,
        validator_pubkeys: HashMap<Address, BlsPubKey>,
        peer_count: Arc<RwLock<u64>>,
        tx_relay_tx: Arc<broadcast::Sender<SignedTransaction>>,
        noise_static: Arc<crate::noise::NoiseStaticKey>,
    ) -> Self {
        // Build a full-node gossip router subscribed to all topics.
        let gossip_router = {
            let mut subs = Subscriptions::default();
            subs.subscribe_all();
            GossipRouter::new(subs)
        };
        Self {
            chain_id,
            listen_port,
            bootnodes,
            storage,
            mempool,
            peer_manager,
            peer_senders: Mutex::new(HashMap::new()),
            noise_static,
            consensus_vote_tx,
            validator_pubkeys,
            peer_count,
            tx_relay_tx,
            peer_scorer:      Arc::new(RwLock::new(PeerScorer::new())),
            gossip_router:    Arc::new(RwLock::new(gossip_router)),
            ping_timestamps:  Mutex::new(HashMap::new()),
            peer_msg_rate:    Mutex::new(HashMap::new()),
        }
    }

    // ── Broadcast helpers (sync — safe from do_commit / process_events) ──────

    pub fn broadcast_block(&self, block: &Block) {
        let msg = Message::Block(Box::new(block.clone()));
        self.broadcast(msg);
    }

    pub fn broadcast_vote(&self, vote: &Vote) {
        self.broadcast(Message::Vote(vote.clone()));
    }

    fn broadcast_tx_msg(&self, signed_tx: &SignedTransaction) {
        self.broadcast(Message::Transaction(signed_tx.clone()));
    }

    /// SEC-2026-05-09 (P4): broadcast helper using bounded `try_send`.
    /// Slow peers whose queue is full are dropped instead of letting the
    /// queue grow without bound (the prior unbounded design was a trivial
    /// memory-exhaustion vector — a single slow peer could OOM the node).
    fn broadcast(&self, msg: Message) {
        let mut dead = Vec::new();
        {
            let senders = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner());
            for (id, tx) in senders.iter() {
                match tx.try_send(msg.clone()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn!(peer = ?id, "P2P (P4): peer queue full — dropping peer");
                        dead.push(id.clone());
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => dead.push(id.clone()),
                }
            }
        }
        if !dead.is_empty() {
            let mut senders = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner());
            for id in dead {
                senders.remove(&id);
            }
        }
    }

    // ── Main run loop ─────────────────────────────────────────────────────────

    pub async fn run(self: Arc<Self>, mut shutdown_rx: watch::Receiver<bool>) {
        // P1-PROD: convert to graceful error instead of panic on bad port.
        let bind_addr: SocketAddr = match format!("0.0.0.0:{}", self.listen_port).parse() {
            Ok(a) => a,
            Err(e) => {
                error!(port = self.listen_port, error = %e,
                    "P2P: listen_port is invalid, cannot bind — node will run without P2P");
                return;
            }
        };

        // ── TX relay task: subscribe to RPC channel, forward to all peers ────
        {
            let srv = Arc::clone(&self);
            let mut tx_rx = self.tx_relay_tx.subscribe();
            tokio::spawn(async move {
                loop {
                    match tx_rx.recv().await {
                        Ok(signed_tx) => srv.broadcast_tx_msg(&signed_tx),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(dropped = n, "TX relay channel lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        // ── Periodic Ping keepalive + score decay ─────────────────────────────
        //
        // Every PING_INTERVAL (30 s) we send a Ping with a per-peer nonce to
        // every connected peer and record (nonce, Instant) so the matching Pong
        // updates latency in both PeerManager and PeerScorer.
        //
        // Every 10 ticks (~5 min) we apply the DECAY_INTERVAL score decay so
        // idle-but-honest peers don't silently accumulate negative scores.
        {
            let srv = Arc::clone(&self);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(PING_INTERVAL);
                let mut decay_ticks: u32 = 0;
                loop {
                    interval.tick().await;

                    // Compute a base nonce from wall-clock nanoseconds.
                    let now_ns = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0);

                    // Send a Ping to each peer, XOR-ing with the peer-id prefix
                    // so every peer gets a distinct nonce this tick.
                    let peer_ids: Vec<PeerId> = {
                        let senders = srv.peer_senders.lock().unwrap_or_else(|p| p.into_inner());
                        senders.keys().cloned().collect()
                    };
                    for id in &peer_ids {
                        let id_prefix = u64::from_le_bytes(
                            id.0[..8].try_into().unwrap_or([0u8; 8])
                        );
                        let nonce = now_ns ^ id_prefix;
                        let sent = {
                            let senders = srv.peer_senders.lock().unwrap_or_else(|p| p.into_inner());
                            if let Some(tx) = senders.get(id) {
                                tx.try_send(Message::Ping { nonce }).ok().map(|_| std::time::Instant::now())
                            } else {
                                None
                            }
                        };
                        if let Some(ts) = sent {
                            srv.ping_timestamps
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .insert(id.clone(), (nonce, ts));
                        }
                    }

                    // Score decay every ~5 minutes.
                    decay_ticks += 1;
                    if decay_ticks >= 10 {
                        decay_ticks = 0;
                        srv.peer_scorer.write().decay_all();
                        debug!("P2P: periodic peer score decay applied");
                    }
                }
            });
        }

        // ── Bootnode dialer with reconnect ────────────────────────────────────
        for node in self.bootnodes.clone() {
            let srv = Arc::clone(&self);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                srv.dial_with_reconnect(node).await;
            });
        }

        let listener = match TcpListener::bind(bind_addr).await {
            Ok(l) => {
                info!(port = self.listen_port, "P2P server listening");
                l
            }
            Err(e) => {
                error!(error = %e, port = self.listen_port, "P2P bind failed");
                return;
            }
        };

        loop {
            tokio::select! {
                res = listener.accept() => {
                    match res {
                        Ok((stream, addr)) => {
                            let srv = Arc::clone(&self);
                            tokio::spawn(async move {
                                // SEC-2026-05-09 (P1): we are the responder
                                // for the Noise XX handshake on inbound conns.
                                srv.handle_connection(stream, addr, false).await;
                            });
                        }
                        Err(e) => warn!(error = %e, "P2P accept error"),
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("P2P server: shutdown signal received");
                    return;
                }
            }
        }
    }

    // ── Bootnode dial with exponential-backoff reconnect ─────────────────────
    //
    // Runs until cancelled.  `dial_peer` blocks for the lifetime of one
    // connection session, so when it returns we know the peer disconnected.

    async fn dial_with_reconnect(self: Arc<Self>, addr: String) {
        let mut backoff = BOOTNODE_RECONNECT_BASE;
        loop {
            Arc::clone(&self).dial_peer(addr.clone()).await;
            warn!(%addr, ?backoff, "P2P: bootnode disconnected, retrying");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BOOTNODE_RECONNECT_MAX);
        }
    }

    /// Connect to `addr`, run handshake, then hand off to `handle_connection`.
    /// Blocks until the connection ends.  Callers should spawn this into a task.
    async fn dial_peer(self: Arc<Self>, addr: String) {
        use std::net::ToSocketAddrs;

        let resolved: Vec<SocketAddr> = match addr.to_socket_addrs() {
            Ok(it) => it.collect(),
            Err(e) => {
                warn!(%addr, error = %e, "P2P: DNS resolve failed");
                return;
            }
        };
        let sock = match resolved.first() {
            Some(s) => *s,
            None => {
                warn!(%addr, "P2P: DNS resolved to 0 addresses");
                return;
            }
        };
        match TcpStream::connect(sock).await {
            Ok(stream) => {
                info!(peer = %sock, "P2P: outbound connection established");
                // SEC-2026-05-09 (P1): we are the initiator for the
                // Noise XX handshake on outbound conns.
                self.handle_connection(stream, sock, true).await;
            }
            Err(e) => warn!(peer = %sock, error = %e, "P2P: outbound dial failed"),
        }
    }

    // ── Per-peer lifecycle ────────────────────────────────────────────────────

    async fn handle_connection(
        self:         Arc<Self>,
        mut stream:   TcpStream,
        addr:         SocketAddr,
        is_initiator: bool,
    ) {
        let _ = stream.set_nodelay(true);

        // ── Status handshake ─────────────────────────────────────────────────
        let local_height = self.storage.get_latest_block_number().unwrap_or(0);
        let local_hash: H256 = self
            .storage
            .get_block_by_number(local_height)
            .ok()
            .flatten()
            .map(|b| b.hash())
            .unwrap_or_else(H256::zero);
        let genesis_hash: H256 = self
            .storage
            .get_block_by_number(0)
            .ok()
            .flatten()
            .map(|b| b.hash())
            .unwrap_or_else(H256::zero);

        // SEC-P2P-NODE-PUBKEY: advertise our Noise XX static public key (32 bytes)
        // so remote peers can derive our node identity and EVM address independently.
        let our_status = StatusMessage {
            protocol_version: PROTOCOL_VERSION,
            chain_id: self.chain_id,
            genesis_hash,
            best_block_hash: local_hash,
            best_block_number: local_height,
            node_pubkey: self.noise_static.public.clone(),
        };

        // SEC-2026-05-09 (P1+P2): Noise XX handshake → encrypted Status.
        let (peer_status, noise_session) = match handshake(
            &mut stream,
            &our_status,
            &self.noise_static,
            is_initiator,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                warn!(peer = %addr, error = %e, "P2P: handshake failed");
                return;
            }
        };

        if peer_status.chain_id != self.chain_id {
            warn!(
                peer = %addr,
                peer_chain = peer_status.chain_id,
                "P2P: chain_id mismatch, dropping"
            );
            return;
        }
        if peer_status.genesis_hash != genesis_hash {
            warn!(peer = %addr, "P2P: genesis hash mismatch, dropping");
            return;
        }

        // ── Peer registration ────────────────────────────────────────────────
        // SEC-2026-05-09 (P2): peer id is keccak256(remote noise static
        // pubkey), not socket-address-derived. Impersonation now requires
        // breaking X25519, not just spoofing a source port.
        let peer_id = noise_session.peer_id.clone();
        debug!(
            peer = %addr,
            peer_id = ?peer_id,
            remote_static = %hex::encode(&noise_session.remote_static),
            "P2P (P2): cryptographic peer id established",
        );
        let peer_info = PeerInfo {
            id: peer_id.clone(),
            addr,
            node_address: Address::ZERO,
            protocol_version: peer_status.protocol_version,
            chain_id: peer_status.chain_id,
            best_block: peer_status.best_block_number,
            latency_ms: 0,
            last_seen: unix_now_secs(),
        };

        {
            let mut pm = self.peer_manager.write();
            if pm.add_peer(peer_info).is_err() {
                warn!(peer = %addr, "P2P: peer rejected (max_peers)");
                return;
            }
        }
        // Register peer with the reputation scorer at the initial score.
        self.peer_scorer.write().add_peer(peer_id.clone());

        // SEC-2026-05-09 (P4): bounded outbound queue (was unbounded).
        let (msg_tx, msg_rx) = mpsc::channel::<Message>(PEER_OUTBOUND_QUEUE);
        self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).insert(peer_id.clone(), msg_tx);
        *self.peer_count.write() = self.peer_manager.read().connected_count() as u64;

        info!(
            peer = %addr,
            peer_head = peer_status.best_block_number,
            local_head = local_height,
            "P2P: peer connected"
        );

        // If peer is ahead, request missing blocks immediately.
        if peer_status.best_block_number > local_height + 1 {
            let to = (local_height + MAX_BLOCK_RANGE).min(peer_status.best_block_number);
            if let Some(tx) = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).get(&peer_id) {
                // SEC-2026-05-09 (P4): try_send on bounded queue.
                let _ = tx.try_send(Message::GetBlockRange(GetBlockRange {
                    from: local_height + 1,
                    to,
                }));
            }
        }

        // Request peer list for mesh expansion.
        if let Some(tx) = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).get(&peer_id) {
            let _ = tx.try_send(Message::FindPeers { target: H256::zero() });
        }

        // ── Split stream into independent read / write halves ─────────────────
        // SEC-2026-05-09 (P1): both halves share the same Noise transport
        // state via Arc<Mutex<...>>. ChaCha20-Poly1305 in Noise tracks send
        // and receive nonces independently, so concurrent encrypt + decrypt
        // are safe under the lock.
        let (mut read_half, write_half) = stream.into_split();
        let transport_writer = Arc::clone(&noise_session.transport);
        let transport_reader = Arc::clone(&noise_session.transport);

        let write_handle = tokio::spawn(async move {
            writer_loop(write_half, msg_rx, transport_writer).await;
        });

        loop {
            // SEC-2026-05-09 (P3): per-message read timeout so a stalled
            // peer cannot hold the read half indefinitely.
            let recv = tokio::time::timeout(
                MSG_READ_TIMEOUT,
                recv_msg_encrypted(&mut read_half, &transport_reader),
            )
            .await;
            let recv = match recv {
                Ok(r) => r,
                Err(_) => {
                    debug!(peer = %addr, "P2P: read timeout, closing");
                    break;
                }
            };
            match recv {
                Ok(msg) => {
                    let srv = Arc::clone(&self);
                    let pid = peer_id.clone();
                    // Synchronous — no .await needed (handle_message has no async ops).
                    if let Err(e) = Self::handle_message(srv, msg, pid) {
                        debug!(peer = %addr, error = %e, "P2P: handler error");
                    }
                }
                Err(e) => {
                    debug!(peer = %addr, error = %e, "P2P: read error, closing");
                    break;
                }
            }
        }

        write_handle.abort();
        self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).remove(&peer_id);
        self.peer_manager.write().remove_peer(&peer_id);
        self.peer_scorer.write().remove_peer(&peer_id);
        self.gossip_router.write().remove_peer(&peer_id);
        self.ping_timestamps.lock().unwrap_or_else(|p| p.into_inner()).remove(&peer_id);
        self.peer_msg_rate.lock().unwrap_or_else(|p| p.into_inner()).remove(&peer_id);
        *self.peer_count.write() = self.peer_manager.read().connected_count() as u64;
        info!(peer = %addr, "P2P: peer disconnected");
    }

    // ── Message dispatch ──────────────────────────────────────────────────────
    //
    // Deliberately synchronous — no `.await` inside, so there are no
    // suspension points and no Send constraints on temporaries.
    // Takes `Arc<Self>` so `Message::Peers` can spawn independent dial tasks.

    fn handle_message(
        self: Arc<Self>,
        msg: Message,
        peer_id: PeerId,
    ) -> Result<(), String> {
        // ── Per-peer rate limiter ─────────────────────────────────────────────
        //
        // Each peer is allowed PEER_MSG_RATE_LIMIT messages per RATE_WINDOW_SECS.
        // Exceeding the limit applies a SpamMessage penalty; if that drives the
        // peer's score to the ban threshold we evict and ban them immediately.
        {
            let now = std::time::Instant::now();
            let mut rates = self.peer_msg_rate.lock().unwrap_or_else(|p| p.into_inner());
            let entry = rates.entry(peer_id.clone()).or_insert((0u32, now));
            if now.duration_since(entry.1).as_secs() >= RATE_WINDOW_SECS {
                *entry = (1, now);
            } else {
                entry.0 += 1;
                if entry.0 > PEER_MSG_RATE_LIMIT {
                    let should_ban = self.peer_scorer.write()
                        .penalise(&peer_id, ScorePenalty::SpamMessage);
                    if should_ban {
                        self.peer_manager.write().ban_peer(&peer_id, "rate limit: spam");
                        self.peer_senders
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .remove(&peer_id);
                    }
                    return Err(format!(
                        "rate limit exceeded ({} msgs / {}s window)", entry.0, RATE_WINDOW_SECS
                    ));
                }
            }
        }

        match msg {
            // ── Keepalive ─────────────────────────────────────────────────────
            Message::Ping { nonce } => {
                if let Some(tx) = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).get(&peer_id) {
                    let _ = tx.try_send(Message::Pong { nonce });
                }
            }

            // Measure round-trip latency from the stored send timestamp.
            Message::Pong { nonce } => {
                let latency_opt = {
                    let mut pings = self.ping_timestamps.lock().unwrap_or_else(|p| p.into_inner());
                    pings.remove(&peer_id).and_then(|(stored_nonce, sent)| {
                        if stored_nonce == nonce {
                            Some(sent.elapsed().as_millis() as u32)
                        } else {
                            None // stale / wrong nonce, ignore
                        }
                    })
                };
                if let Some(latency_ms) = latency_opt {
                    self.peer_manager.write().update_latency(&peer_id, latency_ms);
                    self.peer_scorer.write().update_latency(&peer_id, latency_ms);
                    debug!(peer = ?peer_id, latency_ms, "P2P: Pong — latency updated");
                }
            }

            // ── Block sync: serve a single block by hash ──────────────────────
            Message::GetBlockByHash(hash) => {
                match self.storage.get_block_by_hash(&hash) {
                    Ok(Some(block)) => {
                        if let Some(tx) = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).get(&peer_id) {
                            let _ = tx.try_send(Message::Block(Box::new(block)));
                        }
                    }
                    _ => {
                        // Peer is asking for a block we don't have — minor penalty
                        // to disincentivise probing for non-existent hashes.
                        self.peer_scorer.write().penalise(&peer_id, ScorePenalty::UnknownBlock);
                    }
                }
            }

            // ── Block sync: serve a contiguous range ──────────────────────────
            Message::GetBlockRange(req) => {
                let from = req.from;
                let to = req.to.min(req.from.saturating_add(MAX_BLOCK_RANGE - 1));
                let mut blocks: Vec<Box<Block>> = Vec::new();
                for n in from..=to {
                    match self.storage.get_block_by_number(n) {
                        Ok(Some(b)) => blocks.push(Box::new(b)),
                        _ => break,
                    }
                }
                debug!(from, to, count = blocks.len(), "P2P: serving GetBlockRange");
                if !blocks.is_empty() {
                    if let Some(tx) = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).get(&peer_id) {
                        let _ = tx.try_send(Message::Blocks(blocks));
                    }
                }
            }

            // ── Fast-sync: serve a contiguous header range ────────────────────
            //
            // Headers-first sync (Bitcoin / go-ethereum eth/63 style): the
            // syncing peer downloads + verifies the header chain to a recent
            // finalized pivot, then snap-syncs state at that pivot, then
            // live-catches-up with full blocks. We cap responses at
            // MAX_HEADERS_PER_RESP regardless of what the peer requests.
            Message::GetHeaders(req) => {
                let count = (req.count.min(MAX_HEADERS_PER_RESP)) as u64;
                let mut headers = Vec::new();
                if req.reverse {
                    let mut n = req.from;
                    for _ in 0..count {
                        match self.storage.get_block_by_number(n) {
                            Ok(Some(b)) => {
                                headers.push(b.header);
                                if n == 0 { break; }
                                n -= 1;
                            }
                            _ => break,
                        }
                    }
                } else {
                    for n in req.from..req.from.saturating_add(count) {
                        match self.storage.get_block_by_number(n) {
                            Ok(Some(b)) => headers.push(b.header),
                            _ => break,
                        }
                    }
                }
                debug!(
                    from    = req.from,
                    count   = headers.len(),
                    reverse = req.reverse,
                    "P2P: serving GetHeaders"
                );
                if !headers.is_empty() {
                    if let Some(tx) = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).get(&peer_id) {
                        let _ = tx.try_send(Message::Headers(headers));
                    }
                }
            }

            // ── Block import (single) ─────────────────────────────────────────
            Message::Block(block) => {
                let height = block.header.number;
                let local_head = self.storage.get_latest_block_number().unwrap_or(0);
                if height == local_head + 1 {
                    match execute_and_commit(&self.storage, &self.mempool, *block) {
                        Ok(b) => {
                            let new_head = b.header.number;
                            info!(height = new_head, "P2P: imported block from peer");
                            self.peer_scorer.write().reward(&peer_id, VALID_BLOCK_REWARD);
                            if let Some(tx) = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).get(&peer_id) {
                                let _ = tx.try_send(Message::GetBlockRange(GetBlockRange {
                                    from: new_head + 1,
                                    to:   new_head + MAX_BLOCK_RANGE,
                                }));
                            }
                        }
                        Err(e) => {
                            debug!(error = %e, height, "P2P: block import failed");
                            self.peer_scorer.write()
                                .penalise(&peer_id, ScorePenalty::InvalidMessage);
                        }
                    }
                } else {
                    debug!(height, local_head, "P2P: out-of-order block, ignored");
                }
            }

            // ── Block import (batch) ──────────────────────────────────────────
            Message::Blocks(blocks) => {
                for block in blocks {
                    let height = block.header.number;
                    let local_head = self.storage.get_latest_block_number().unwrap_or(0);
                    if height == local_head + 1 {
                        match execute_and_commit(&self.storage, &self.mempool, *block) {
                            Ok(b) => {
                                info!(height = b.header.number, "P2P: synced block");
                                self.peer_scorer.write().reward(&peer_id, VALID_BLOCK_REWARD);
                            }
                            Err(e) => {
                                debug!(error = %e, height, "P2P: sync import failed");
                                self.peer_scorer.write()
                                    .penalise(&peer_id, ScorePenalty::InvalidMessage);
                                break;
                            }
                        }
                    }
                }
            }

            // ── Vote relay ────────────────────────────────────────────────────
            Message::Vote(vote) => {
                if let Some(pubkey) = self.validator_pubkeys.get(&vote.voter) {
                    debug!(voter = ?vote.voter, "P2P: injecting remote vote");
                    let _ = self.consensus_vote_tx.send(InboundVote {
                        vote,
                        pubkey: pubkey.clone(),
                    });
                    self.peer_scorer.write().reward(&peer_id, VALID_MESSAGE_REWARD);
                } else {
                    debug!(voter = ?vote.voter, "P2P: vote from unknown validator, penalising");
                    self.peer_scorer.write()
                        .penalise(&peer_id, ScorePenalty::InvalidMessage);
                }
            }

            // ── HotStuff-2: block proposal (Hs2Proposal) ─────────────────────
            //
            // Full HotStuff-2 consensus integration (ZEP-022) will wire a
            // dedicated inbound channel to the ConsensusDriver. For now we
            // accept the message, log it at debug level, and reward the peer
            // so the wire protocol is fully exercised without dropping these
            // with an error.
            Message::Hs2Proposal(_proposal) => {
                debug!(peer = ?peer_id, "P2P: HotStuff-2 Hs2Proposal received");
                self.peer_scorer.write().reward(&peer_id, VALID_QC_REWARD);
            }

            // ── HotStuff-2: Jolteon timeout share ─────────────────────────────
            Message::TimeoutShareMsg(_ts) => {
                debug!(peer = ?peer_id, "P2P: HotStuff-2 TimeoutShare received");
                self.peer_scorer.write().reward(&peer_id, VALID_MESSAGE_REWARD);
            }

            // ── HotStuff-2: formed Timeout Certificate ────────────────────────
            Message::TimeoutCertMsg(_tc) => {
                debug!(peer = ?peer_id, "P2P: HotStuff-2 TimeoutCertificate received");
                self.peer_scorer.write().reward(&peer_id, VALID_QC_REWARD);
            }

            // ── TX relay (single) ─────────────────────────────────────────────
            Message::Transaction(signed_tx) => {
                let sender = signed_tx.from;
                let account = self
                    .storage
                    .get_account(&sender)
                    .ok()
                    .unwrap_or_default();
                match self.mempool.write().add_transaction(
                    signed_tx,
                    account.balance_u128(),
                    account.nonce,
                ) {
                    Ok(hash) => {
                        debug!(
                            hash = %hex::encode(hash.as_bytes()),
                            "P2P: relayed TX added to mempool"
                        );
                        self.peer_scorer.write().reward(&peer_id, VALID_MESSAGE_REWARD);
                    }
                    Err(e) => {
                        debug!(error = %e, "P2P: TX relay rejected by mempool");
                        self.peer_scorer.write()
                            .penalise(&peer_id, ScorePenalty::InvalidMessage);
                    }
                }
            }

            // ── TX relay (batch) ──────────────────────────────────────────────
            //
            // Process each TX independently. If more TXes fail than succeed
            // the peer is probably sending garbage — apply a spam penalty.
            Message::Transactions(txs) => {
                let mut ok   = 0u32;
                let mut fail = 0u32;
                for signed_tx in txs {
                    let sender = signed_tx.from;
                    let account = self
                        .storage
                        .get_account(&sender)
                        .ok()
                        .unwrap_or_default();
                    match self.mempool.write().add_transaction(
                        signed_tx,
                        account.balance_u128(),
                        account.nonce,
                    ) {
                        Ok(_)  => ok   += 1,
                        Err(_) => fail += 1,
                    }
                }
                debug!(ok, fail, "P2P: batch TX relay processed");
                if ok > 0 {
                    self.peer_scorer.write().reward(&peer_id, VALID_MESSAGE_REWARD);
                }
                if fail > ok {
                    // More rejections than acceptances → probable spam batch.
                    self.peer_scorer.write()
                        .penalise(&peer_id, ScorePenalty::SpamMessage);
                }
            }

            // ── Gossip relay ──────────────────────────────────────────────────
            //
            // Inbound GossipMsg envelopes are routed through GossipRouter which
            // handles deduplication (LRU seen-cache), TTL enforcement, and
            // fan-out target selection. Only messages that pass all three checks
            // are forwarded; duplicates and expired messages are silently dropped.
            Message::GossipMsg(envelope) => {
                let topic_byte = envelope.topic;
                let original_ttl = envelope.ttl;

                // Map the topic byte to the typed GossipTopic enum.
                let topic = match topic_byte {
                    0x01 => GossipTopic::NewBlock,
                    0x02 => GossipTopic::Transaction,
                    0x03 => GossipTopic::ConsensusVote,
                    0x04 => GossipTopic::TimeoutShare,
                    0x05 => GossipTopic::Proposal,
                    0x06 => GossipTopic::TimeoutCert,
                    _ => {
                        debug!(topic = topic_byte, peer = ?peer_id, "P2P: unknown gossip topic");
                        self.peer_scorer.write()
                            .penalise(&peer_id, ScorePenalty::InvalidMessage);
                        return Ok(());
                    }
                };

                // Build the internal GossipMessage for router processing.
                // We clone the payload here so we can move the original
                // envelope bytes into the forwarded message on Relay.
                let gossip_msg = GossipMessage {
                    topic,
                    payload:    envelope.payload.clone(),
                    message_id: envelope.message_id,
                    ttl:        original_ttl,
                    origin:     Some(peer_id.clone()),
                };

                let all_peer_ids: Vec<PeerId> = self
                    .peer_manager
                    .read()
                    .peers
                    .keys()
                    .cloned()
                    .collect();

                let decision = self.gossip_router.write()
                    .process_inbound(gossip_msg, &all_peer_ids);

                match decision {
                    GossipDecision::Relay(targets) => {
                        debug!(
                            topic    = topic_byte,
                            targets  = targets.len(),
                            ttl      = original_ttl,
                            "P2P: relaying gossip message"
                        );
                        // Forward the envelope with TTL decremented so the
                        // receiving peers' routers see the correct hop count.
                        let forwarded = Message::GossipMsg(GossipEnvelope {
                            topic:      topic_byte,
                            payload:    envelope.payload,        // moved
                            message_id: envelope.message_id,
                            ttl:        original_ttl.saturating_sub(1),
                        });
                        let senders = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner());
                        for target in &targets {
                            if let Some(tx) = senders.get(target) {
                                let _ = tx.try_send(forwarded.clone());
                            }
                        }
                        self.peer_scorer.write().reward(&peer_id, VALID_MESSAGE_REWARD);
                    }
                    GossipDecision::Duplicate => {
                        trace!(peer = ?peer_id, topic = topic_byte, "P2P: gossip duplicate dropped");
                    }
                    GossipDecision::TtlExpired => {
                        trace!(peer = ?peer_id, topic = topic_byte, "P2P: gossip TTL expired");
                    }
                    GossipDecision::NotSubscribed => {
                        trace!(peer = ?peer_id, topic = topic_byte, "P2P: gossip topic not subscribed");
                    }
                }
            }

            // ── Peer discovery: FindPeers → respond with known peer addrs ──────
            Message::FindPeers { .. } => {
                let addrs: Vec<String> = self
                    .peer_manager
                    .read()
                    .peers
                    .values()
                    .map(|p| p.addr.to_string())
                    .collect();
                if !addrs.is_empty() {
                    if let Some(tx) = self.peer_senders.lock().unwrap_or_else(|p| p.into_inner()).get(&peer_id) {
                        let _ = tx.try_send(Message::Peers(addrs));
                    }
                }
            }

            // ── Peer discovery: Peers → spawn dial tasks for unknown addrs ─────
            //
            // We spawn each dial as an independent task to avoid async recursion
            // through: handle_message → dial_peer → handle_connection → handle_message.
            Message::Peers(addrs) => {
                let current_count = self.peer_manager.read().connected_count();
                if current_count >= MAX_DIAL_PEERS {
                    return Ok(());
                }
                // SEC-2026-05-09 (P5): cap dial tasks from a single Peers message.
                const MAX_DIAL_PER_MSG: usize = 16;
                let mut dialed = 0usize;
                for addr_str in addrs {
                    if dialed >= MAX_DIAL_PER_MSG { break; }
                    let sock: SocketAddr = match addr_str.parse() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    // SEC-2026-05-09 (P5): SSRF protection — reject non-routable
                    // addresses supplied by remote peers.
                    if !is_publicly_routable(&sock) {
                        warn!(addr = %sock, "P2P (P5): refusing non-routable address from peer");
                        continue;
                    }
                    let already = self
                        .peer_manager
                        .read()
                        .peers
                        .values()
                        .any(|p| p.addr == sock);
                    if !already {
                        let srv = Arc::clone(&self);
                        let addr_owned = addr_str.clone();
                        debug!(addr = %sock, "P2P: discovered peer, dialling");
                        tokio::spawn(async move { srv.dial_peer(addr_owned).await; });
                        dialed += 1;
                    }
                }
            }

            // ── Inbound-only / not-yet-fully-integrated messages ──────────────
            //
            // These are valid wire messages we can receive but don't yet have a
            // full inbound handler for. We log at trace (not warn) so production
            // logs aren't flooded; the explicit match arms ensure we notice at
            // compile time if we add a new Message variant without handling it.
            Message::Headers(_) => {
                trace!(peer = ?peer_id, "P2P: Headers (fast-sync response — no inbound handler)");
            }
            Message::SnapshotMetaResp(_) => {
                trace!(peer = ?peer_id, "P2P: SnapshotMetaResp (no inbound handler)");
            }
            Message::SnapshotChunkResp(_) => {
                trace!(peer = ?peer_id, "P2P: SnapshotChunkResp (no inbound handler)");
            }
            Message::QuorumCert(_) => {
                trace!(peer = ?peer_id, "P2P: QuorumCert (no inbound handler)");
            }
            Message::Proposal { .. } => {
                trace!(peer = ?peer_id, "P2P: HotStuff-v1 Proposal (superseded by Hs2Proposal)");
            }
            Message::Timeout { .. } => {
                trace!(peer = ?peer_id, "P2P: HotStuff-v1 Timeout (superseded by TimeoutShareMsg)");
            }
            Message::GetSnapshotMeta { .. } => {
                trace!(peer = ?peer_id, "P2P: GetSnapshotMeta (fast-sync not yet served)");
            }
            Message::GetSnapshotChunk(_) => {
                trace!(peer = ?peer_id, "P2P: GetSnapshotChunk (fast-sync not yet served)");
            }
            Message::Status(_) => {
                // Status is only valid during the handshake phase.
                debug!(peer = ?peer_id, "P2P: spurious Status after handshake, ignoring");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper tasks and functions
// ---------------------------------------------------------------------------

async fn writer_loop(
    mut writer:    tokio::net::tcp::OwnedWriteHalf,
    mut rx:        mpsc::Receiver<Message>,
    transport:     Arc<parking_lot::Mutex<snow::TransportState>>,
) {
    while let Some(msg) = rx.recv().await {
        let bytes = match serde_json::to_vec(&msg) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if bytes.len() > MAX_MSG_BYTES {
            warn!(len = bytes.len(), "P2P (P1): refusing to send oversized msg");
            continue;
        }
        // SEC-2026-05-09 (P1): every byte after the handshake is encrypted
        // with the per-session Noise ChaCha20-Poly1305 transport.
        if crate::noise::send_encrypted(&mut writer, &transport, &bytes)
            .await
            .is_err()
        {
            break;
        }
    }
}

/// SEC-2026-05-09 (P1): receive one decrypted JSON `Message` from the peer.
async fn recv_msg_encrypted(
    reader:    &mut tokio::net::tcp::OwnedReadHalf,
    transport: &Arc<parking_lot::Mutex<snow::TransportState>>,
) -> Result<Message, String> {
    let buf = crate::noise::recv_encrypted(reader, transport)
        .await
        .map_err(|e| e.to_string())?;
    if buf.len() > MAX_MSG_BYTES {
        return Err(format!("decrypted message too large: {} bytes", buf.len()));
    }
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

/// SEC-2026-05-09 (P5): is this socket address safe for an outbound dial
/// triggered by a remote peer? Rejects loopback, private RFC1918 ranges,
/// link-local, multicast, and unspecified addresses to defeat SSRF-style
/// attacks where a hostile peer tries to make us connect to internal
/// infrastructure (databases, admin RPC, cloud metadata service, etc.).
fn is_publicly_routable(sock: &SocketAddr) -> bool {
    use std::net::IpAddr;
    match sock.ip() {
        IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || v4.is_unspecified()
                // 169.254.169.254 — cloud metadata service
                || v4.octets() == [169, 254, 169, 254])
        }
        IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                // fc00::/7 — unique local
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // fe80::/10 — link-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}


fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
