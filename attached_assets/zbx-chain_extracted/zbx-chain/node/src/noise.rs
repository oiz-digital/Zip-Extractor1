//! SEC-2026-05-09 (P1+P2): Noise XX transport encryption + cryptographic
//! PeerId for the P2P layer.
//!
//! ## What this gives us
//!
//! Before this module the P2P transport was JSON-over-cleartext-TCP and the
//! `PeerId` was derived from the remote socket address. That had two
//! consequences:
//!
//!   * **P1** — anyone on the path between two validators (an upstream ISP,
//!     a malicious peer relaying traffic, a passive collector) could read
//!     and modify every gossip message, vote, transaction, and block in
//!     transit.
//!   * **P2** — peer identity was nothing more than `(ip, port)`, so one
//!     attacker could impersonate any number of peers simply by dialling
//!     us from different source ports.
//!
//! After this module:
//!
//!   * The transport is wrapped in a `Noise_XX_25519_ChaChaPoly_SHA256`
//!     handshake, providing mutual authentication, forward secrecy, and
//!     authenticated encryption for every subsequent frame.
//!   * The `PeerId` is `keccak256(remote_static_x25519_pubkey)`, so
//!     impersonation requires solving discrete log on Curve25519.
//!
//! ## Static key persistence
//!
//! The node's long-lived X25519 static keypair lives at
//! `<data_dir>/p2p_static.key` (32 raw bytes, mode 0600). On first boot we
//! generate one and write it; on subsequent boots we load it. Losing this
//! file is equivalent to changing your node's PeerId — peers that pinned
//! you by ID will need to be told the new one.

use rand::RngCore;
use snow::{Builder, HandshakeState, TransportState};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use zbx_crypto::keccak::keccak256;
use zbx_network::peer::PeerId;

/// Standardised Noise pattern. XX is the only mutually-authenticating
/// pattern in the Noise handshake suite that does not require either side
/// to know the other's static public key in advance — perfect for a
/// permissionless P2P layer.
pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

/// Maximum encrypted frame size (Noise spec: 65535 bytes). We split larger
/// payloads into multiple frames.
pub const NOISE_MAX_FRAME: usize = 65535;
/// Authenticated-encryption tag overhead per frame.
pub const NOISE_TAG_LEN: usize = 16;
/// Maximum cleartext bytes per frame.
pub const NOISE_MAX_PLAINTEXT: usize = NOISE_MAX_FRAME - NOISE_TAG_LEN;

#[derive(Debug, thiserror::Error)]
pub enum NoiseError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("snow: {0}")]
    Snow(#[from] snow::Error),
    #[error("frame too large: {0} > {NOISE_MAX_FRAME}")]
    FrameTooLarge(usize),
}

/// A long-lived X25519 static keypair.
#[derive(Clone)]
pub struct NoiseStaticKey {
    pub private: Vec<u8>, // 32 bytes
    pub public:  Vec<u8>, // 32 bytes
}

impl NoiseStaticKey {
    /// Generate a brand-new keypair using the OS RNG.
    pub fn generate() -> Result<Self, NoiseError> {
        let builder = Builder::new(NOISE_PARAMS.parse()?);
        let kp = builder.generate_keypair()?;
        Ok(NoiseStaticKey { private: kp.private, public: kp.public })
    }

    /// Load a keypair from `<data_dir>/p2p_static.key`, generating + writing
    /// a fresh one if no file exists.
    pub fn load_or_create(data_dir: &Path) -> Result<Self, NoiseError> {
        let path = Self::path(data_dir);
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            if bytes.len() != 32 {
                return Err(NoiseError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("p2p_static.key: expected 32 bytes, got {}", bytes.len()),
                )));
            }
            let pubkey = derive_public(&bytes);
            Ok(NoiseStaticKey { private: bytes, public: pubkey })
        } else {
            let kp = Self::generate()?;
            // Task #12: route through `zbx_keystore::secure_write` so the
            // file is created with mode 0o600 atomically (no umask race
            // window between `fs::write` and `set_permissions` like the
            // pre-Task-#12 code had). Failures are bubbled up rather than
            // best-effort dropped — losing perms on the P2P static key is
            // a P2 security regression we now refuse to silently ship.
            zbx_keystore::secure_write(&path, &kp.private).map_err(|e| {
                NoiseError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    format!("p2p_static.key secure_write: {e}"),
                ))
            })?;
            Ok(kp)
        }
    }

    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("p2p_static.key")
    }

    /// Our PeerId — `keccak256(public_key)`.
    pub fn peer_id(&self) -> PeerId {
        peer_id_from_pubkey(&self.public)
    }
}

/// Derive an X25519 public key from a 32-byte private scalar via snow's
/// own DH resolver. Avoids pulling in `x25519-dalek` directly and stays in
/// lockstep with whatever Curve25519 implementation `snow` is using.
fn derive_public(private: &[u8]) -> Vec<u8> {
    use snow::params::DHChoice;
    use snow::resolvers::{CryptoResolver, DefaultResolver};
    let resolver = DefaultResolver::default();
    let mut dh = resolver
        .resolve_dh(&DHChoice::Curve25519)
        .expect("Curve25519 DH must be available in snow's default resolver");
    dh.set(private);
    dh.pubkey().to_vec()
}

/// Compute the canonical PeerId for an X25519 static public key.
pub fn peer_id_from_pubkey(public_key: &[u8]) -> PeerId {
    let h = keccak256(public_key);
    PeerId(h.0)
}

// ─── Length-prefixed framing for the handshake ─────────────────────────────

async fn write_frame(stream: &mut TcpStream, buf: &[u8]) -> Result<(), NoiseError> {
    if buf.len() > NOISE_MAX_FRAME {
        return Err(NoiseError::FrameTooLarge(buf.len()));
    }
    let len = (buf.len() as u16).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(buf).await?;
    Ok(())
}

async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, NoiseError> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;
    if len > NOISE_MAX_FRAME {
        return Err(NoiseError::FrameTooLarge(len));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Result of a completed Noise handshake: an authenticated transport plus
/// the cryptographic identity of the remote peer.
pub struct NoiseSession {
    pub transport:     Arc<Mutex<TransportState>>,
    pub remote_static: [u8; 32],
    pub peer_id:       PeerId,
}

/// Drive a Noise XX handshake as the **initiator** (we dialled the peer).
/// Pattern:  -> e   ;   <- e, ee, s, es   ;   -> s, se
pub async fn handshake_initiator(
    stream:       &mut TcpStream,
    local_static: &NoiseStaticKey,
) -> Result<NoiseSession, NoiseError> {
    let mut hs: HandshakeState = Builder::new(NOISE_PARAMS.parse()?)
        .local_private_key(&local_static.private)
        .build_initiator()?;
    handshake_drive(stream, &mut hs, true).await?;
    finish(hs)
}

/// Drive a Noise XX handshake as the **responder** (peer dialled us).
pub async fn handshake_responder(
    stream:       &mut TcpStream,
    local_static: &NoiseStaticKey,
) -> Result<NoiseSession, NoiseError> {
    let mut hs: HandshakeState = Builder::new(NOISE_PARAMS.parse()?)
        .local_private_key(&local_static.private)
        .build_responder()?;
    handshake_drive(stream, &mut hs, false).await?;
    finish(hs)
}

async fn handshake_drive(
    stream:    &mut TcpStream,
    hs:        &mut HandshakeState,
    initiator: bool,
) -> Result<(), NoiseError> {
    // XX is exactly 3 messages: initiator, responder, initiator.
    let mut buf = vec![0u8; NOISE_MAX_FRAME];
    // Step 1
    if initiator {
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(stream, &buf[..n]).await?;
    } else {
        let frame = read_frame(stream).await?;
        let _ = hs.read_message(&frame, &mut buf)?;
    }
    // Step 2
    if initiator {
        let frame = read_frame(stream).await?;
        let _ = hs.read_message(&frame, &mut buf)?;
    } else {
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(stream, &buf[..n]).await?;
    }
    // Step 3
    if initiator {
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(stream, &buf[..n]).await?;
    } else {
        let frame = read_frame(stream).await?;
        let _ = hs.read_message(&frame, &mut buf)?;
    }
    Ok(())
}

fn finish(hs: HandshakeState) -> Result<NoiseSession, NoiseError> {
    let remote = hs
        .get_remote_static()
        .ok_or_else(|| NoiseError::Snow(snow::Error::State(snow::error::StateProblem::HandshakeNotFinished)))?;
    let mut remote_static = [0u8; 32];
    remote_static.copy_from_slice(remote);
    let peer_id = peer_id_from_pubkey(&remote_static);
    let transport = hs.into_transport_mode()?;
    Ok(NoiseSession {
        transport: Arc::new(Mutex::new(transport)),
        remote_static,
        peer_id,
    })
}

// ─── Encrypted message I/O over an established Noise transport ─────────────

/// Encrypt a message and write it as one or more Noise frames, each prefixed
/// by a 2-byte big-endian length.
pub async fn send_encrypted<W>(
    writer:    &mut W,
    transport: &Arc<Mutex<TransportState>>,
    plaintext: &[u8],
) -> Result<(), NoiseError>
where
    W: AsyncWriteExt + Unpin,
{
    // Wire format: u32 BE total cleartext length, then N noise frames.
    let total = (plaintext.len() as u32).to_be_bytes();
    writer.write_all(&total).await?;

    let mut offset = 0;
    let mut frame = vec![0u8; NOISE_MAX_FRAME];
    while offset < plaintext.len() {
        let chunk = (plaintext.len() - offset).min(NOISE_MAX_PLAINTEXT);
        let n = {
            let mut t = transport.lock();
            t.write_message(&plaintext[offset..offset + chunk], &mut frame)?
        };
        let frame_len = (n as u16).to_be_bytes();
        writer.write_all(&frame_len).await?;
        writer.write_all(&frame[..n]).await?;
        offset += chunk;
    }
    Ok(())
}

/// Read one logical message: a u32 cleartext-length header followed by N
/// 2-byte-prefixed Noise frames whose decrypted bytes concatenate to that
/// length.
pub async fn recv_encrypted<R>(
    reader:    &mut R,
    transport: &Arc<Mutex<TransportState>>,
) -> Result<Vec<u8>, NoiseError>
where
    R: AsyncReadExt + Unpin,
{
    let mut hdr = [0u8; 4];
    reader.read_exact(&mut hdr).await?;
    let total = u32::from_be_bytes(hdr) as usize;
    // Sanity cap to match the existing MAX_MSG_BYTES (16 MiB).
    if total > 16 * 1024 * 1024 {
        return Err(NoiseError::FrameTooLarge(total));
    }
    let mut out = Vec::with_capacity(total);
    let mut scratch = vec![0u8; NOISE_MAX_FRAME];
    while out.len() < total {
        let mut len_buf = [0u8; 2];
        reader.read_exact(&mut len_buf).await?;
        let flen = u16::from_be_bytes(len_buf) as usize;
        if flen > NOISE_MAX_FRAME {
            return Err(NoiseError::FrameTooLarge(flen));
        }
        let mut frame = vec![0u8; flen];
        reader.read_exact(&mut frame).await?;
        let n = {
            let mut t = transport.lock();
            t.read_message(&frame, &mut scratch)?
        };
        out.extend_from_slice(&scratch[..n]);
    }
    Ok(out)
}
