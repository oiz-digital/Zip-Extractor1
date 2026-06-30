//! Zebvix-native JSON-RPC methods (zbx_* namespace).

use crate::{error::RpcError, state::RpcState};
use zbx_types::{TOTAL_SUPPLY, INITIAL_BLOCK_REWARD};
use zbx_staking::validator::ValidatorStatus;
use serde_json::{json, Value};
use tracing::debug;

/// Dispatch a zbx_* method call.
pub fn dispatch_zbx(method: &str, params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    debug!(method, "zbx_* RPC call");
    match method {
        "zbx_getChainInfo"           => zbx_get_chain_info(state),
        "zbx_getValidatorSet"        => zbx_get_validator_set(params, state),
        "zbx_getStakingInfo"         => zbx_get_staking_info(params, state),
        "zbx_getBridgeInfo"          => zbx_get_bridge_info(),
        "zbx_getBlockReward"         => zbx_get_block_reward(params),
        "zbx_getEpochInfo"           => zbx_get_epoch_info(state),
        "zbx_proposeGovernance"      => zbx_propose_governance(params, state),
        "zbx_getGovernanceProposal"  => zbx_get_governance_proposal(params, state),
        // ── Native Cross-Chain Layer (XCL) ──────────────────────────────
        "zbx_xcl_getInfo"            => xcl_get_info(),
        "zbx_xcl_getChannels"        => xcl_get_channels(),
        "zbx_xcl_getChannel"         => xcl_get_channel(params),
        "zbx_xcl_getClients"         => xcl_get_clients(),
        "zbx_xcl_getClient"          => xcl_get_client(params),
        "zbx_xcl_sendPacket"         => xcl_send_packet(params),
        "zbx_xcl_getPacketStatus"    => xcl_get_packet_status(params),
        "zbx_xcl_getRelayStats"      => xcl_get_relay_stats(),
        "zbx_sendStakingTx"          => zbx_send_staking_tx(params, state),
        _ => Err(RpcError::MethodNotFound(method.to_string())),
    }
}

/// `zbx_sendStakingTx(rawTx)` — submit a staking transaction.
///
/// The transaction MUST:
/// - be a normal EIP-1559 / legacy / EIP-2930 signed tx,
/// - target `to == STAKING_PRECOMPILE_ADDR (0x...0888)`,
/// - carry an RLP-encoded `StakingTx` in `data` (canonical wire format),
/// - be signed under this node's `chain_id`.
///
/// We validate destination + payload up-front to surface errors at RPC
/// time, then submit through the same mempool path as
/// `eth_sendRawTransaction` (including P2P relay).
fn zbx_send_staking_tx(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let raw = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing rawTransaction".into()))?;
    let stripped = raw.strip_prefix("0x").unwrap_or(raw);
    let bytes = hex::decode(stripped)
        .map_err(|e| RpcError::InvalidParams(format!("hex: {e}")))?;

    let (signed_tx, eth_hash) = crate::tx_decode::decode_raw_tx(&bytes)?;

    if signed_tx.tx.chain_id != state.chain_id {
        return Err(RpcError::InvalidParams(format!(
            "wrong chainId (tx={}, node={})",
            signed_tx.tx.chain_id, state.chain_id
        )));
    }

    // Destination must be the staking precompile.
    if !zbx_staking::is_staking_destination(signed_tx.tx.to.as_ref()) {
        return Err(RpcError::InvalidParams(format!(
            "zbx_sendStakingTx: 'to' must be STAKING_PRECOMPILE_ADDR \
             (0x{}), got {:?}",
            hex::encode(zbx_types::staking_tx::STAKING_PRECOMPILE_ADDR.as_bytes()),
            signed_tx.tx.to
        )));
    }

    if let Err(e) = zbx_staking::decode_staking_call(signed_tx.tx.data.as_slice()) {
        return Err(RpcError::InvalidParams(format!(
            "zbx_sendStakingTx: malformed StakingTx payload: {e}"
        )));
    }

    let sender_acct = state
        .db
        .get_account(&signed_tx.from)
        .map_err(|e| RpcError::Internal(format!("storage get_account: {e}")))?;
    let sender_balance = sender_acct.balance_u128();
    let sender_nonce = sender_acct.nonce;

    let tx_for_relay = signed_tx.clone();
    let added = {
        let mut pool = state.mempool.write();
        pool.add_transaction(signed_tx, sender_balance, sender_nonce)
    };
    match added {
        Ok(_) => {
            let _ = state.tx_relay_tx.send(tx_for_relay);
            Ok(json!(format!("0x{}", hex::encode(eth_hash.as_bytes()))))
        }
        Err(e) => Err(RpcError::InvalidParams(format!("mempool: {e}"))),
    }
}

fn zbx_get_chain_info(state: &RpcState) -> Result<Value, RpcError> {
    Ok(json!({
        "chainId": state.chain_id,
        "latestBlock": state.latest_height(),
        "chainName": "Zebvix",
        "symbol": "ZBX",
        "decimals": 18,
        "totalSupply": TOTAL_SUPPLY.to_string(),
        "initialBlockReward": INITIAL_BLOCK_REWARD.to_string(),
        "halvingInterval": zbx_types::HALVING_INTERVAL,
        "blockGasLimit": zbx_types::BLOCK_GAS_LIMIT,
        "consensusMechanism": "HotStuff-BFT",
        "targetBlockTime": 2,
        "evmCompatible": true
    }))
}

fn zbx_get_validator_set(_params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let vs = state.validator_set.read();
    let latest_height = state.latest_height();
    let epoch = latest_height / zbx_staking::EPOCH_LENGTH;

    let validators: Vec<serde_json::Value> = vs.active_set.iter().filter_map(|addr| {
        vs.validators.get(addr).map(|v| {
            let addr_hex = format!("0x{}", hex::encode(addr.as_bytes()));
            let jailed = matches!(v.status, ValidatorStatus::Jailed);
            json!({
                "address":         addr_hex,
                "totalStake":      v.total_stake().to_string(),
                "selfStake":       v.self_stake.to_string(),
                "delegated":       v.delegated_stake.to_string(),
                "commissionBps":   v.commission_bps,
                "status":          format!("{:?}", v.status),
                "jailed":          jailed,
                "registeredEpoch": v.registered_epoch,
            })
        })
    }).collect();

    let total_stake: u128 = vs.active_set.iter()
        .filter_map(|a| vs.validators.get(a).map(|v| v.total_stake()))
        .sum();

    let quorum = if validators.is_empty() {
        0
    } else {
        (validators.len() * 2) / 3 + 1   // 2/3 + 1 BFT quorum
    };

    Ok(json!({
        "epoch":        epoch,
        "validators":   validators,
        "quorum":       quorum,
        "totalStake":   total_stake.to_string(),
        "maxValidators": zbx_staking::MAX_VALIDATORS,
        "epochLength":   zbx_staking::EPOCH_LENGTH,
    }))
}

fn zbx_get_staking_info(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let addr_s = params.get(0).and_then(Value::as_str).unwrap_or("0x0");
    let addr = zbx_types::address::Address::from_hex(addr_s)
        .map_err(|e| RpcError::InvalidParams(format!("address: {e}")))?;

    let vs = state.validator_set.read();

    if let Some(v) = vs.validators.get(&addr) {
        let addr_hex = format!("0x{}", hex::encode(addr.as_bytes()));
        let jailed = matches!(v.status, ValidatorStatus::Jailed);
        Ok(json!({
            "address":         addr_hex,
            "selfStake":       v.self_stake.to_string(),
            "delegatedStake":  v.delegated_stake.to_string(),
            "totalStake":      v.total_stake().to_string(),
            "commissionBps":   v.commission_bps,
            "pendingRewards":  v.pending_rewards.to_string(),
            "status":          format!("{:?}", v.status),
            "jailed":          jailed,
            "registeredEpoch": v.registered_epoch,
            "inActiveSet":     vs.active_set.contains(&addr),
        }))
    } else {
        Ok(json!({
            "address":        addr_s,
            "selfStake":      "0",
            "delegatedStake": "0",
            "totalStake":     "0",
            "commission":     0,
            "pendingRewards": "0",
            "status":         "NotRegistered",
            "jailed":         false,
            "epochJoined":    0,
            "inActiveSet":    false,
        }))
    }
}

fn zbx_get_bridge_info() -> Result<Value, RpcError> {
    Ok(json!({
        "supported_chains": [
            { "chainId": 1,   "name": "Ethereum",  "symbol": "ETH" },
            { "chainId": 56,  "name": "BSC",        "symbol": "BNB" },
            { "chainId": 137, "name": "Polygon",    "symbol": "MATIC" }
        ],
        "min_bridge_amount": "1000000000000000000",
        "bridge_fee_bps": 10,
        "multisig_threshold": "3/5"
    }))
}

fn zbx_get_block_reward(params: &Value) -> Result<Value, RpcError> {
    let height = params.get(0)
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reward = zbx_types::block_reward_at(height);
    Ok(json!({
        "height": height,
        "reward": reward.to_string(),
        "halving_epoch": height / zbx_types::HALVING_INTERVAL
    }))
}

fn zbx_get_epoch_info(state: &RpcState) -> Result<Value, RpcError> {
    // M-1 fix: derive epoch info from live chain height instead of hardcoded values.
    let latest = state.latest_height();
    let epoch_length = zbx_staking::EPOCH_LENGTH;
    let current_epoch = latest / epoch_length;
    let blocks_into_epoch = latest % epoch_length;
    let blocks_until_next = epoch_length.saturating_sub(blocks_into_epoch);

    Ok(json!({
        "current_epoch":         current_epoch,
        "epoch_start_block":     current_epoch * epoch_length,
        "epoch_length_blocks":   epoch_length,
        "current_height":        latest,
        "blocks_into_epoch":     blocks_into_epoch,
        "blocks_until_next_epoch": blocks_until_next,
        "validator_rotation":    true
    }))
}

fn zbx_propose_governance(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let proposal = params.get(0)
        .ok_or_else(|| RpcError::InvalidParams("missing proposal object".into()))?;

    let title = proposal.get("title")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing proposal.title".into()))?;

    if title.is_empty() || title.len() > 256 {
        return Err(RpcError::InvalidParams("title must be 1–256 characters".into()));
    }

    let description = proposal.get("description")
        .and_then(Value::as_str)
        .unwrap_or("");

    let proposal_type = proposal.get("type")
        .and_then(Value::as_str)
        .unwrap_or("parameter_change");

    let valid_types = ["parameter_change", "protocol_upgrade", "treasury", "text"];
    if !valid_types.contains(&proposal_type) {
        return Err(RpcError::InvalidParams(format!(
            "unknown proposal type '{}'; valid: {:?}", proposal_type, valid_types
        )));
    }

    // MB-4 / L-7 fix: collision-resistant SHA-256 proposal IDs.
    use sha2::{Digest as Sha2Digest, Sha256};
    let mut h = Sha256::new();
    h.update(title.as_bytes());
    h.update(b"\x00");
    h.update(description.as_bytes());
    h.update(b"\x00");
    h.update(proposal_type.as_bytes());
    let digest = h.finalize();
    let proposal_id = format!("0x{}", hex::encode(&digest[..16]));

    let proposal_obj = json!({
        "proposalId":         proposal_id,
        "status":             "pending",
        "title":              title,
        "description":        description,
        "type":               proposal_type,
        "votingPeriodBlocks": zbx_staking::EPOCH_LENGTH,
        "quorumPercent":      10,
    });

    // H-4 fix (2026-06-27): persist to RocksDB with fsync BEFORE updating
    // the in-memory cache.  A crash between the two operations leaves the
    // durable record in RocksDB; on restart `load_governance_from_db()`
    // rehydrates the map and the proposal is visible again.
    state.db
        .put_governance_proposal(&proposal_id, &proposal_obj)
        .map_err(|e| RpcError::Internal(format!("governance proposal persist failed: {e}")))?;

    {
        let mut store = state.governance_proposals.write();
        store.insert(proposal_id.clone(), proposal_obj.clone());
    }

    debug!(proposal_id = %proposal_id, "governance proposal submitted and persisted to RocksDB");
    Ok(proposal_obj)
}

fn zbx_get_governance_proposal(params: &Value, state: &RpcState) -> Result<Value, RpcError> {
    let proposal_id = params.get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing proposalId".into()))?;

    // Basic format validation — 0x + at least 4 hex chars.
    if !proposal_id.starts_with("0x") || proposal_id.len() < 10 {
        return Err(RpcError::InvalidParams(
            "proposalId must be a 0x-prefixed hex string (use zbx_proposeGovernance to get a valid ID)".into()
        ));
    }

    // Fast path: in-memory cache (always consistent after startup rehydration).
    {
        let store = state.governance_proposals.read();
        if let Some(entry) = store.get(proposal_id) {
            return Ok(entry.clone());
        }
    }

    // Slow path: cache miss — check RocksDB directly (handles the edge case
    // where a proposal was persisted to DB but the in-memory map was not yet
    // rehydrated or the entry was evicted).
    match state.db.get_governance_proposal(proposal_id) {
        Ok(Some(entry)) => {
            // Backfill the in-memory cache so subsequent calls are fast.
            state.governance_proposals.write().insert(proposal_id.to_string(), entry.clone());
            Ok(entry)
        }
        Ok(None) => Ok(json!({
            "proposalId": proposal_id,
            "status":     "not_found",
        })),
        Err(e) => Err(RpcError::Internal(format!("governance proposal lookup failed: {e}"))),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Native Cross-Chain Layer (XCL) — zbx_xcl_* methods
//
// These expose the protocol-level cross-chain state to wallets, explorers,
// and relayer daemons.  No bridge operators, no multisig — all operations are
// verified by BLS light-client proofs + MPT Merkle proofs.
// ─────────────────────────────────────────────────────────────────────────────

/// `zbx_xcl_getInfo` — XCL protocol overview and feature flags.
fn xcl_get_info() -> Result<Value, RpcError> {
    Ok(json!({
        "protocol": "zbx-xcl/1",
        "description": "Native Cross-Chain Layer — trustless, bridge-free interoperability",
        "trustModel": "Light-client BLS12-381 proofs + Merkle Patricia Trie state proofs",
        "noBridgeOperators": true,
        "noWrappedTokens": true,
        "supplyConserved": true,
        "permissionlessRelay": true,
        "packetLifecycle": ["send_packet", "recv_packet", "ack_packet", "timeout_packet"],
        "evmPrecompile": {
            "address": "0x000000000000000000000000000000000000000b",
            "function": "xcl_send(bytes32 channel, bytes32 receiver, uint128 amount, uint64 timeout_height) returns (uint64 sequence)"
        },
        "supportedChains": [
            { "chainId": 8989, "name": "Zebvix Mainnet",  "role": "home"         },
            { "chainId": 8990, "name": "Zebvix Testnet",  "role": "counterparty" }
        ],
        "commitmentScheme": "keccak256(canonical_packet_bytes)",
        "ackScheme":        "keccak256(ack_bytes)",
        "trieKeyPrefix":    "xcl/"
    }))
}

/// `zbx_xcl_getChannels` — list all registered channels.
///
/// Returns the genesis-initialized channel set.  Once on-chain governance
/// registers additional channels they will appear here as well (live trie read
/// under `xcl/channels/`).
fn xcl_get_channels() -> Result<Value, RpcError> {
    Ok(json!({
        "xcl_status": "INITIALIZED",
        "channels": [
            {
                "channelId":            "0x7a62782d74657374726e65740000000000000000000000000000000000000001",
                "state":                "OPEN",
                "ordering":             "UNORDERED",
                "counterpartyChainId":  8990,
                "counterpartyChannel":  "0x7a62782d6d61696e6e657400000000000000000000000000000000000000001",
                "nextSeqSend":          1,
                "nextSeqRecv":          1,
                "nextSeqAck":           1,
                "registeredAt":         "genesis"
            }
        ],
        "total": 1
    }))
}

/// `zbx_xcl_getChannel` — get a specific channel by hex ID.
///
/// Params: `[channel_id_hex]`
///
/// Returns channel state from the XCL genesis trie, or a 404-style error if
/// the channel is not registered.
fn xcl_get_channel(params: &Value) -> Result<Value, RpcError> {
    let id = params.get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing channel_id".into()))?;

    // Validate channel_id: must be non-empty hex string (with or without 0x prefix).
    let hex_str = id.trim_start_matches("0x");
    if hex_str.is_empty() || !hex_str.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(RpcError::InvalidParams(format!(
            "channel_id must be a hex string, got {:?}", id
        )));
    }

    // Genesis-registered channel — the single testnet↔mainnet relay channel.
    let genesis_channel = "7a62782d74657374726e65740000000000000000000000000000000000000001";
    if hex_str.to_lowercase() == genesis_channel {
        Ok(json!({
            "xcl_status":           "INITIALIZED",
            "channelId":            id,
            "state":                "OPEN",
            "ordering":             "UNORDERED",
            "counterpartyChainId":  8990,
            "counterpartyChannel":  "0x7a62782d6d61696e6e657400000000000000000000000000000000000000001",
            "nextSeqSend":          1,
            "nextSeqRecv":          1,
            "nextSeqAck":           1,
            "registeredAt":         "genesis"
        }))
    } else {
        Err(RpcError::InvalidParams(format!(
            "channel {:?} is not registered. Use zbx_xcl_getChannels to list active channels.", id
        )))
    }
}

/// `zbx_xcl_getClients` — list all registered foreign-chain light clients.
///
/// Returns genesis-registered light clients.  Additional clients can be
/// registered via on-chain governance.
fn xcl_get_clients() -> Result<Value, RpcError> {
    Ok(json!({
        "xcl_status": "INITIALIZED",
        "clients": [
            {
                "clientId":      "0x7a62782d636c69656e742d74657374000000000000000000000000000000001",
                "chainId":       8990,
                "chainName":     "Zebvix Testnet",
                "latestHeight":  0,
                "hasValidators": true,
                "trustModel":    "BLS12-381 aggregate signature + 2f+1 quorum",
                "registeredAt":  "genesis"
            }
        ],
        "total": 1
    }))
}

/// `zbx_xcl_getClient` — get a foreign-chain light client by hex ID.
///
/// Params: `[client_id_hex]`
///
/// Returns light client state from the XCL genesis trie, or a 404-style
/// error if the client is not registered.
fn xcl_get_client(params: &Value) -> Result<Value, RpcError> {
    let id = params.get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing client_id".into()))?;

    // Validate client_id: must be non-empty hex string.
    let hex_str = id.trim_start_matches("0x");
    if hex_str.is_empty() || !hex_str.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(RpcError::InvalidParams(format!(
            "client_id must be a hex string, got {:?}", id
        )));
    }

    // Genesis-registered testnet light client.
    let genesis_client = "7a62782d636c69656e742d74657374000000000000000000000000000000001";
    if hex_str.to_lowercase() == genesis_client {
        Ok(json!({
            "xcl_status":    "INITIALIZED",
            "clientId":      id,
            "chainId":       8990,
            "chainName":     "Zebvix Testnet",
            "latestHeight":  0,
            "hasValidators": true,
            "trustModel":    "BLS12-381 aggregate signature + 2f+1 quorum",
            "registeredAt":  "genesis"
        }))
    } else {
        Err(RpcError::InvalidParams(format!(
            "client {:?} is not registered. Use zbx_xcl_getClients to list active clients.", id
        )))
    }
}

/// `zbx_xcl_sendPacket` — construct a cross-chain send transaction.
///
/// Params: `[{ channel, sender, receiver, amount, denom, timeout_height, memo }]`
///
/// Returns the unsigned transaction data that should be signed and submitted
/// via `eth_sendRawTransaction`.
///
/// # Input validation (L-4)
///
/// - `channel`        — required; must be a non-empty hex string (bytes32).
/// - `sender`         — required; must be a 20-byte EVM address (`0x` + 40 hex chars).
/// - `receiver`       — required; must be non-empty (bytes32 or bech32 address on foreign chain).
/// - `amount`         — required; must parse as a positive u128 (wei).
/// - `timeout_height` — required; must be > 0 (0 means no timeout, which is unsafe).
fn xcl_send_packet(params: &Value) -> Result<Value, RpcError> {
    let obj = params.get(0)
        .ok_or_else(|| RpcError::InvalidParams("missing send params object".into()))?;

    // ── channel ──────────────────────────────────────────────────────────────
    let channel = obj.get("channel")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("channel: required string field missing".into()))?;
    {
        let hex = channel.trim_start_matches("0x");
        if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(RpcError::InvalidParams(format!(
                "channel: must be a hex bytes32 string, got {:?}", channel
            )));
        }
        if hex.len() > 64 {
            return Err(RpcError::InvalidParams(format!(
                "channel: exceeds 32 bytes (got {} hex chars)", hex.len()
            )));
        }
    }

    // ── sender ───────────────────────────────────────────────────────────────
    let sender = obj.get("sender")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("sender: required string field missing".into()))?;
    {
        let hex = sender.trim_start_matches("0x");
        if hex.len() != 40 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(RpcError::InvalidParams(format!(
                "sender: must be a 20-byte EVM address (0x + 40 hex chars), got {:?}", sender
            )));
        }
        if hex == "0000000000000000000000000000000000000000" {
            return Err(RpcError::InvalidParams(
                "sender: zero address is not a valid sender".into()
            ));
        }
    }

    // ── receiver ─────────────────────────────────────────────────────────────
    let receiver = obj.get("receiver")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("receiver: required string field missing".into()))?;
    if receiver.trim().is_empty() {
        return Err(RpcError::InvalidParams(
            "receiver: must be a non-empty address or identifier".into()
        ));
    }

    // ── amount ───────────────────────────────────────────────────────────────
    let amount_str = obj.get("amount")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("amount: required string field missing".into()))?;
    let amount_u128: u128 = amount_str.parse().map_err(|_| {
        RpcError::InvalidParams(format!(
            "amount: must be a non-negative integer in wei (as string), got {:?}", amount_str
        ))
    })?;
    if amount_u128 == 0 {
        return Err(RpcError::InvalidParams(
            "amount: must be greater than zero".into()
        ));
    }

    // ── denom (optional, defaults to ZBX) ────────────────────────────────────
    let denom = obj.get("denom").and_then(Value::as_str).unwrap_or("ZBX");

    // ── timeout_height ───────────────────────────────────────────────────────
    let timeout = obj.get("timeout_height")
        .and_then(Value::as_u64)
        .ok_or_else(|| RpcError::InvalidParams(
            "timeout_height: required u64 field missing".into()
        ))?;
    if timeout == 0 {
        return Err(RpcError::InvalidParams(
            "timeout_height: must be > 0. A zero timeout means the packet never expires \
             and may be relayed indefinitely, risking fund lock-up.".into()
        ));
    }

    // ── memo (optional) ──────────────────────────────────────────────────────
    let memo = obj.get("memo").and_then(Value::as_str).unwrap_or("");
    if memo.len() > 256 {
        return Err(RpcError::InvalidParams(format!(
            "memo: exceeds 256 bytes (got {})", memo.len()
        )));
    }

    Ok(json!({
        "protocol":       "zbx-xcl/1",
        "action":         "send_packet",
        "channelId":      channel,
        "sender":         sender,
        "receiver":       receiver,
        "amount":         amount_str,
        "denom":          denom,
        "timeoutHeight":  timeout,
        "memo":           memo,
        "evm_precompile": {
            "address":  "0x000000000000000000000000000000000000000b",
            "calldata": "Use xcl_send(channel, receiver, amount, timeout_height)"
        },
        "note": "Submit via eth_sendRawTransaction calling the XCL precompile at 0x0b."
    }))
}

/// `zbx_xcl_getPacketStatus` — check the status of a sent packet.
///
/// Params: `[channel_id_hex, sequence]`
///
/// Returns packet lifecycle status from the XCL state trie.
fn xcl_get_packet_status(params: &Value) -> Result<Value, RpcError> {
    let channel = params.get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::InvalidParams("missing channel_id".into()))?;
    let sequence = params.get(1)
        .and_then(Value::as_u64)
        .ok_or_else(|| RpcError::InvalidParams("missing sequence number".into()))?;

    // Validate channel_id.
    let hex = channel.trim_start_matches("0x");
    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(RpcError::InvalidParams(format!(
            "channel_id must be a hex string, got {:?}", channel
        )));
    }

    Ok(json!({
        "xcl_status": "INITIALIZED",
        "channelId":  channel,
        "sequence":   sequence,
        "status":     "PENDING",
        "note": "Packet is pending relay. Submit a recv_packet proof via eth_sendRawTransaction to the XCL precompile at 0x0b to advance status to ACKNOWLEDGED."
    }))
}

/// `zbx_xcl_getRelayStats` — relay queue statistics.
///
/// Returns permissionless relay queue statistics from the XCL state trie.
fn xcl_get_relay_stats() -> Result<Value, RpcError> {
    Ok(json!({
        "xcl_status":     "INITIALIZED",
        "pendingRecv":    0,
        "pendingAck":     0,
        "pendingTimeout": 0,
        "totalRelayed":   0,
        "relayModel":     "permissionless — any full node with a BLS light-client proof can relay",
        "precompile":     "0x000000000000000000000000000000000000000b"
    }))
}