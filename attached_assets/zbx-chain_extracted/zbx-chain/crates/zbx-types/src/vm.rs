//! ZVM sandbox + gas types — opcode-cost table, sandbox limits, host-call
//! whitelist, and the canonical fault-discriminant for execution failures.
//!
//! Type-and-codec layer. The actual interpreter lives in `zbx-vm` and consumes
//! these types verbatim.
//!
//! Discipline (matches sibling modules):
//! - `BTreeMap` for canonical RLP. `validate()` runs in BOTH constructor AND
//!   `Decodable::decode`.
//! - `s.append(&inner)` inside `begin_list(N)` — never the naked
//!   `inner.rlp_append(s)`, which silently skips the parent counter
//!   (LESSON #11 inverse).
//! - Newtype `Encodable` impls use `self.inner.rlp_append(s)` for direct
//!   delegation (LESSON #11).
//! - `item_count() != N` field-count gate at the top of every `decode`.

use std::collections::BTreeMap;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// OpcodeKind — opaque tag identifying a class of VM instructions.
//
// The interpreter assigns concrete u16 discriminants; this type only
// carries the discriminant + a strict-decode invariant (must match a
// known kind).
// ---------------------------------------------------------------------------

/// Opcode-class discriminant. The interpreter owns the opcode table; here we
/// only enforce that the discriminant is in the documented closed range.
///
/// Layout: high byte = group (arith / mem / control / crypto / host),
/// low byte = sub-opcode. Reserved ranges produce `IllegalOpcode` at decode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct OpcodeKind(pub u16);

impl OpcodeKind {
    pub const GROUP_ARITH: u8 = 0x00;
    pub const GROUP_MEM: u8 = 0x01;
    pub const GROUP_CONTROL: u8 = 0x02;
    pub const GROUP_CRYPTO: u8 = 0x03;
    pub const GROUP_HOST: u8 = 0x04;

    pub const fn group(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub const fn sub(self) -> u8 {
        (self.0 & 0xff) as u8
    }

    /// Group must be one of the 5 documented values, else `IllegalOpcode`.
    pub fn validate(self) -> Result<(), DecoderError> {
        match self.group() {
            Self::GROUP_ARITH
            | Self::GROUP_MEM
            | Self::GROUP_CONTROL
            | Self::GROUP_CRYPTO
            | Self::GROUP_HOST => Ok(()),
            _ => Err(DecoderError::Custom("OpcodeKind: unknown group")),
        }
    }
}

impl Encodable for OpcodeKind {
    fn rlp_append(&self, s: &mut RlpStream) {
        // LESSON #11: direct delegation, not s.append(&u16).
        self.0.rlp_append(s);
    }
}

impl Decodable for OpcodeKind {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let v: u16 = rlp.as_val()?;
        let k = Self(v);
        k.validate()?;
        Ok(k)
    }
}

// ---------------------------------------------------------------------------
// HostCallId — whitelist of permitted host calls. Anything outside the
// whitelist is rejected with `HostCallDenied`.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub enum HostCallId {
    /// Read account balance.
    GetBalance = 0,
    /// Read nonce of an address.
    GetNonce = 1,
    /// Read storage slot.
    StorageRead = 2,
    /// Write storage slot (gas-metered, journaled).
    StorageWrite = 3,
    /// Emit event log.
    EmitLog = 4,
    /// Hash bytes via keccak256.
    Keccak256 = 5,
    /// Verify ECDSA signature.
    EcdsaVerify = 6,
    /// Read current block height (deterministic; never wall-clock).
    GetBlockHeight = 7,
    /// Read current block timestamp (deterministic; coordinator-set).
    GetBlockTimestamp = 8,
    /// Read chain id.
    GetChainId = 9,
    /// Cross-contract call (gas-metered, depth-bounded).
    Call = 10,
    /// Self-destruct (governance-gated).
    SelfDestruct = 11,
}

impl HostCallId {
    pub const ALL: &'static [HostCallId] = &[
        Self::GetBalance,
        Self::GetNonce,
        Self::StorageRead,
        Self::StorageWrite,
        Self::EmitLog,
        Self::Keccak256,
        Self::EcdsaVerify,
        Self::GetBlockHeight,
        Self::GetBlockTimestamp,
        Self::GetChainId,
        Self::Call,
        Self::SelfDestruct,
    ];

    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(b: u8) -> Result<Self, DecoderError> {
        match b {
            0 => Ok(Self::GetBalance),
            1 => Ok(Self::GetNonce),
            2 => Ok(Self::StorageRead),
            3 => Ok(Self::StorageWrite),
            4 => Ok(Self::EmitLog),
            5 => Ok(Self::Keccak256),
            6 => Ok(Self::EcdsaVerify),
            7 => Ok(Self::GetBlockHeight),
            8 => Ok(Self::GetBlockTimestamp),
            9 => Ok(Self::GetChainId),
            10 => Ok(Self::Call),
            11 => Ok(Self::SelfDestruct),
            _ => Err(DecoderError::Custom("HostCallId: unknown discriminant")),
        }
    }
}

impl Encodable for HostCallId {
    fn rlp_append(&self, s: &mut RlpStream) {
        self.to_u8().rlp_append(s);
    }
}

impl Decodable for HostCallId {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let b: u8 = rlp.as_val()?;
        Self::from_u8(b)
    }
}

// ---------------------------------------------------------------------------
// OpcodeCost — gas charge for a single opcode invocation.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpcodeCost {
    /// Base cost paid before execution (entry tax).
    pub base: u64,
    /// Per-byte cost (for memory or copy ops). Zero for non-byte-scaling ops.
    pub per_byte: u64,
}

impl OpcodeCost {
    pub fn new(base: u64, per_byte: u64) -> Result<Self, DecoderError> {
        let c = Self { base, per_byte };
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.base == 0 {
            return Err(DecoderError::Custom("OpcodeCost.base must be > 0"));
        }
        Ok(())
    }

    /// Gas for an op consuming `n_bytes` of payload. Saturating.
    pub fn gas_for(&self, n_bytes: u64) -> u64 {
        self.base
            .saturating_add(self.per_byte.saturating_mul(n_bytes))
    }
}

impl Encodable for OpcodeCost {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(2);
        s.append(&self.base);
        s.append(&self.per_byte);
    }
}

impl Decodable for OpcodeCost {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 2 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let c = Self {
            base: rlp.val_at(0)?,
            per_byte: rlp.val_at(1)?,
        };
        c.validate()?;
        Ok(c)
    }
}

// ---------------------------------------------------------------------------
// SandboxLimits — hard caps the interpreter MUST enforce per-tx.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxLimits {
    /// Maximum gas a single transaction may consume (also the block-default).
    pub max_gas: u64,
    /// Maximum instruction count per tx (DoS guard independent of gas).
    pub max_instructions: u64,
    /// Maximum bytes of linear memory the contract may allocate.
    pub max_memory_bytes: u32,
    /// Maximum value-stack depth at any moment.
    pub max_stack_depth: u32,
    /// Maximum nested-call depth (cross-contract).
    pub max_call_depth: u16,
    /// Maximum bytes per single storage slot.
    pub max_storage_value_bytes: u32,
}

impl SandboxLimits {
    pub fn mainnet_default() -> Self {
        Self {
            max_gas: 30_000_000,
            max_instructions: 10_000_000,
            max_memory_bytes: 16 * 1024 * 1024, // 16 MiB
            max_stack_depth: 1024,
            max_call_depth: 64,
            max_storage_value_bytes: 32 * 1024, // 32 KiB
        }
    }

    pub fn testnet_default() -> Self {
        Self {
            max_gas: 100_000_000,
            max_instructions: 50_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
            max_stack_depth: 4096,
            max_call_depth: 128,
            max_storage_value_bytes: 64 * 1024,
        }
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.max_gas == 0 {
            return Err(DecoderError::Custom("max_gas must be > 0"));
        }
        if self.max_instructions == 0 {
            return Err(DecoderError::Custom("max_instructions must be > 0"));
        }
        if self.max_memory_bytes == 0 {
            return Err(DecoderError::Custom("max_memory_bytes must be > 0"));
        }
        if self.max_stack_depth == 0 {
            return Err(DecoderError::Custom("max_stack_depth must be > 0"));
        }
        if self.max_call_depth == 0 {
            return Err(DecoderError::Custom("max_call_depth must be > 0"));
        }
        if self.max_storage_value_bytes == 0 {
            return Err(DecoderError::Custom(
                "max_storage_value_bytes must be > 0",
            ));
        }
        Ok(())
    }
}

impl Encodable for SandboxLimits {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(6);
        s.append(&self.max_gas);
        s.append(&self.max_instructions);
        s.append(&self.max_memory_bytes);
        s.append(&self.max_stack_depth);
        s.append(&self.max_call_depth);
        s.append(&self.max_storage_value_bytes);
    }
}

impl Decodable for SandboxLimits {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 6 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let l = Self {
            max_gas: rlp.val_at(0)?,
            max_instructions: rlp.val_at(1)?,
            max_memory_bytes: rlp.val_at(2)?,
            max_stack_depth: rlp.val_at(3)?,
            max_call_depth: rlp.val_at(4)?,
            max_storage_value_bytes: rlp.val_at(5)?,
        };
        l.validate()?;
        Ok(l)
    }
}

// ---------------------------------------------------------------------------
// VmError — canonical fault discriminants the interpreter MUST report.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum VmError {
    OutOfGas { requested: u64, remaining: u64 },
    OutOfInstructions { used: u64, max: u64 },
    OutOfMemory { requested: u32, max: u32 },
    StackOverflow { depth: u32, max: u32 },
    StackUnderflow,
    CallDepthExceeded { depth: u16, max: u16 },
    IllegalOpcode { opcode: OpcodeKind },
    HostCallDenied { id: u8 },
    InvalidJump { target: u32 },
    InvalidMemoryAccess { offset: u32, size: u32 },
    StorageValueTooLarge { actual: u32, max: u32 },
    /// Contract reverted explicitly via `revert(reason)`.
    Reverted { reason_len: u32 },
    /// Runtime panic that isn't a normal revert (interpreter MUST NOT trust it).
    InterpreterFault,
}

impl VmError {
    pub fn tag(&self) -> u8 {
        match self {
            Self::OutOfGas { .. } => 0,
            Self::OutOfInstructions { .. } => 1,
            Self::OutOfMemory { .. } => 2,
            Self::StackOverflow { .. } => 3,
            Self::StackUnderflow => 4,
            Self::CallDepthExceeded { .. } => 5,
            Self::IllegalOpcode { .. } => 6,
            Self::HostCallDenied { .. } => 7,
            Self::InvalidJump { .. } => 8,
            Self::InvalidMemoryAccess { .. } => 9,
            Self::StorageValueTooLarge { .. } => 10,
            Self::Reverted { .. } => 11,
            Self::InterpreterFault => 12,
        }
    }
}

impl Encodable for VmError {
    fn rlp_append(&self, s: &mut RlpStream) {
        match self {
            Self::OutOfGas { requested, remaining } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(requested);
                s.append(remaining);
            }
            Self::OutOfInstructions { used, max } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(used);
                s.append(max);
            }
            Self::OutOfMemory { requested, max } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(requested);
                s.append(max);
            }
            Self::StackOverflow { depth, max } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(depth);
                s.append(max);
            }
            Self::StackUnderflow => {
                s.begin_list(1);
                s.append(&self.tag());
            }
            Self::CallDepthExceeded { depth, max } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(depth);
                s.append(max);
            }
            Self::IllegalOpcode { opcode } => {
                s.begin_list(2);
                s.append(&self.tag());
                s.append(opcode);
            }
            Self::HostCallDenied { id } => {
                s.begin_list(2);
                s.append(&self.tag());
                s.append(id);
            }
            Self::InvalidJump { target } => {
                s.begin_list(2);
                s.append(&self.tag());
                s.append(target);
            }
            Self::InvalidMemoryAccess { offset, size } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(offset);
                s.append(size);
            }
            Self::StorageValueTooLarge { actual, max } => {
                s.begin_list(3);
                s.append(&self.tag());
                s.append(actual);
                s.append(max);
            }
            Self::Reverted { reason_len } => {
                s.begin_list(2);
                s.append(&self.tag());
                s.append(reason_len);
            }
            Self::InterpreterFault => {
                s.begin_list(1);
                s.append(&self.tag());
            }
        }
    }
}

impl Decodable for VmError {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let n = rlp.item_count()?;
        if n == 0 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let tag: u8 = rlp.val_at(0)?;
        match tag {
            0 if n == 3 => Ok(Self::OutOfGas {
                requested: rlp.val_at(1)?,
                remaining: rlp.val_at(2)?,
            }),
            1 if n == 3 => Ok(Self::OutOfInstructions {
                used: rlp.val_at(1)?,
                max: rlp.val_at(2)?,
            }),
            2 if n == 3 => Ok(Self::OutOfMemory {
                requested: rlp.val_at(1)?,
                max: rlp.val_at(2)?,
            }),
            3 if n == 3 => Ok(Self::StackOverflow {
                depth: rlp.val_at(1)?,
                max: rlp.val_at(2)?,
            }),
            4 if n == 1 => Ok(Self::StackUnderflow),
            5 if n == 3 => Ok(Self::CallDepthExceeded {
                depth: rlp.val_at(1)?,
                max: rlp.val_at(2)?,
            }),
            6 if n == 2 => Ok(Self::IllegalOpcode {
                opcode: rlp.val_at(1)?,
            }),
            7 if n == 2 => Ok(Self::HostCallDenied { id: rlp.val_at(1)? }),
            8 if n == 2 => Ok(Self::InvalidJump {
                target: rlp.val_at(1)?,
            }),
            9 if n == 3 => Ok(Self::InvalidMemoryAccess {
                offset: rlp.val_at(1)?,
                size: rlp.val_at(2)?,
            }),
            10 if n == 3 => Ok(Self::StorageValueTooLarge {
                actual: rlp.val_at(1)?,
                max: rlp.val_at(2)?,
            }),
            11 if n == 2 => Ok(Self::Reverted {
                reason_len: rlp.val_at(1)?,
            }),
            12 if n == 1 => Ok(Self::InterpreterFault),
            _ => Err(DecoderError::Custom(
                "VmError: unknown tag or arity mismatch",
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// VmPolicy — composite of opcode-cost table + sandbox limits + host-call
// whitelist. The interpreter holds a `&VmPolicy` for the lifetime of a tx.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VmPolicy {
    pub limits: SandboxLimits,
    pub default_cost: OpcodeCost,
    /// Per-opcode-kind cost overrides. Missing entries fall back to default.
    pub opcode_costs: BTreeMap<OpcodeKind, OpcodeCost>,
    /// Whitelisted host calls. Anything not in this set returns
    /// `HostCallDenied`.
    pub host_whitelist: BTreeMap<HostCallId, OpcodeCost>,
}

impl VmPolicy {
    pub fn mainnet_default() -> Self {
        let mut host = BTreeMap::new();
        for h in HostCallId::ALL {
            host.insert(*h, OpcodeCost::new(100, 0).expect("non-zero base"));
        }
        Self {
            limits: SandboxLimits::mainnet_default(),
            default_cost: OpcodeCost::new(2, 0).expect("non-zero base"),
            opcode_costs: BTreeMap::new(),
            host_whitelist: host,
        }
    }

    pub fn testnet_default() -> Self {
        let mut host = BTreeMap::new();
        for h in HostCallId::ALL {
            host.insert(*h, OpcodeCost::new(50, 0).expect("non-zero base"));
        }
        Self {
            limits: SandboxLimits::testnet_default(),
            default_cost: OpcodeCost::new(1, 0).expect("non-zero base"),
            opcode_costs: BTreeMap::new(),
            host_whitelist: host,
        }
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        self.limits.validate()?;
        self.default_cost.validate()?;
        for (k, c) in &self.opcode_costs {
            k.validate()?;
            c.validate()?;
        }
        for (_, c) in &self.host_whitelist {
            c.validate()?;
        }
        if self.host_whitelist.is_empty() {
            return Err(DecoderError::Custom(
                "host_whitelist must contain at least one entry",
            ));
        }
        Ok(())
    }

    /// Look up gas for an opcode, falling back to `default_cost` on miss.
    pub fn cost_of(&self, op: OpcodeKind) -> &OpcodeCost {
        self.opcode_costs.get(&op).unwrap_or(&self.default_cost)
    }

    /// True iff `id` is whitelisted; interpreter MUST consult this before
    /// dispatching any host call.
    pub fn is_host_allowed(&self, id: HostCallId) -> bool {
        self.host_whitelist.contains_key(&id)
    }
}

impl Encodable for VmPolicy {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(4);
        s.append(&self.limits);
        s.append(&self.default_cost);
        // opcode_costs as inline list of [OpcodeKind, OpcodeCost] pairs.
        s.begin_list(self.opcode_costs.len());
        for (k, c) in &self.opcode_costs {
            s.begin_list(2);
            s.append(k);
            s.append(c);
        }
        // host_whitelist as inline list of [HostCallId, OpcodeCost] pairs.
        s.begin_list(self.host_whitelist.len());
        for (h, c) in &self.host_whitelist {
            s.begin_list(2);
            s.append(h);
            s.append(c);
        }
    }
}

impl Decodable for VmPolicy {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 4 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let limits: SandboxLimits = rlp.val_at(0)?;
        let default_cost: OpcodeCost = rlp.val_at(1)?;

        let mut opcode_costs = BTreeMap::new();
        let mut prev: Option<OpcodeKind> = None;
        for item in rlp.at(2)?.iter() {
            if item.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let k: OpcodeKind = item.val_at(0)?;
            let c: OpcodeCost = item.val_at(1)?;
            if let Some(p) = prev {
                if k <= p {
                    return Err(DecoderError::Custom(
                        "opcode_costs must be strictly ascending",
                    ));
                }
            }
            prev = Some(k);
            opcode_costs.insert(k, c);
        }

        let mut host_whitelist = BTreeMap::new();
        let mut prev_h: Option<HostCallId> = None;
        for item in rlp.at(3)?.iter() {
            if item.item_count()? != 2 {
                return Err(DecoderError::RlpIncorrectListLen);
            }
            let h: HostCallId = item.val_at(0)?;
            let c: OpcodeCost = item.val_at(1)?;
            if let Some(p) = prev_h {
                if h <= p {
                    return Err(DecoderError::Custom(
                        "host_whitelist must be strictly ascending",
                    ));
                }
            }
            prev_h = Some(h);
            host_whitelist.insert(h, c);
        }

        let p = Self {
            limits,
            default_cost,
            opcode_costs,
            host_whitelist,
        };
        p.validate()?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rlp::{decode, encode};

    // --- OpcodeKind ---
    #[test]
    fn opcode_validate_accepts_known_groups() {
        OpcodeKind(0x0000).validate().unwrap();
        OpcodeKind(0x0401).validate().unwrap();
    }
    #[test]
    fn opcode_validate_rejects_unknown_group() {
        assert!(OpcodeKind(0x9900).validate().is_err());
    }
    #[test]
    fn opcode_rlp_round_trip() {
        let k = OpcodeKind(0x0301);
        let bytes = encode(&k);
        let back: OpcodeKind = decode(&bytes).unwrap();
        assert_eq!(k, back);
    }

    // --- HostCallId ---
    #[test]
    fn host_round_trip_all() {
        for &h in HostCallId::ALL {
            let bytes = encode(&h);
            let back: HostCallId = decode(&bytes).unwrap();
            assert_eq!(h, back);
        }
    }
    #[test]
    fn host_decode_rejects_unknown() {
        let bytes = encode(&99u8);
        let r: Result<HostCallId, _> = decode(&bytes);
        assert!(r.is_err());
    }

    // --- OpcodeCost ---
    #[test]
    fn cost_rejects_zero_base() {
        assert!(OpcodeCost::new(0, 0).is_err());
    }
    #[test]
    fn cost_gas_for_saturates() {
        let c = OpcodeCost::new(10, u64::MAX).unwrap();
        assert_eq!(c.gas_for(2), u64::MAX);
    }
    #[test]
    fn cost_rlp_round_trip() {
        let c = OpcodeCost::new(7, 3).unwrap();
        let bytes = encode(&c);
        let back: OpcodeCost = decode(&bytes).unwrap();
        assert_eq!(c, back);
    }

    // --- SandboxLimits ---
    #[test]
    fn limits_mainnet_validates() {
        SandboxLimits::mainnet_default().validate().unwrap();
    }
    #[test]
    fn limits_testnet_validates() {
        SandboxLimits::testnet_default().validate().unwrap();
    }
    #[test]
    fn limits_rejects_zero_field() {
        let mut l = SandboxLimits::mainnet_default();
        l.max_call_depth = 0;
        assert!(l.validate().is_err());
    }
    #[test]
    fn limits_rlp_round_trip() {
        let l = SandboxLimits::mainnet_default();
        let bytes = encode(&l);
        let back: SandboxLimits = decode(&bytes).unwrap();
        assert_eq!(l, back);
    }

    // --- VmError ---
    #[test]
    fn vm_error_round_trip_all_variants() {
        let cases = vec![
            VmError::OutOfGas { requested: 5, remaining: 1 },
            VmError::OutOfInstructions { used: 100, max: 50 },
            VmError::OutOfMemory { requested: 9, max: 5 },
            VmError::StackOverflow { depth: 2, max: 1 },
            VmError::StackUnderflow,
            VmError::CallDepthExceeded { depth: 5, max: 3 },
            VmError::IllegalOpcode { opcode: OpcodeKind(0x0301) },
            VmError::HostCallDenied { id: 99 },
            VmError::InvalidJump { target: 42 },
            VmError::InvalidMemoryAccess { offset: 1, size: 2 },
            VmError::StorageValueTooLarge { actual: 100, max: 50 },
            VmError::Reverted { reason_len: 16 },
            VmError::InterpreterFault,
        ];
        for c in cases {
            let bytes = encode(&c);
            let back: VmError = decode(&bytes).unwrap();
            assert_eq!(c, back);
        }
    }

    #[test]
    fn vm_error_decode_rejects_arity_mismatch() {
        let mut s = RlpStream::new_list(2);
        s.append(&0u8); // OutOfGas tag
        s.append(&1u64); // missing second u64
        let bytes = s.out();
        let r: Result<VmError, _> = decode(&bytes);
        assert!(r.is_err());
    }

    // --- VmPolicy ---
    #[test]
    fn policy_mainnet_validates() {
        VmPolicy::mainnet_default().validate().unwrap();
    }

    #[test]
    fn policy_cost_of_falls_back_to_default() {
        let p = VmPolicy::mainnet_default();
        let c = p.cost_of(OpcodeKind(0x0001));
        assert_eq!(c, &p.default_cost);
    }

    #[test]
    fn policy_is_host_allowed_whitelist() {
        let p = VmPolicy::mainnet_default();
        assert!(p.is_host_allowed(HostCallId::Keccak256));
    }

    #[test]
    fn policy_rejects_empty_whitelist() {
        let mut p = VmPolicy::mainnet_default();
        p.host_whitelist.clear();
        assert!(p.validate().is_err());
    }

    #[test]
    fn policy_rlp_round_trip() {
        let p = VmPolicy::mainnet_default();
        let bytes = encode(&p);
        let back: VmPolicy = decode(&bytes).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn policy_rlp_with_opcode_overrides() {
        let mut p = VmPolicy::mainnet_default();
        p.opcode_costs.insert(OpcodeKind(0x0301), OpcodeCost::new(30, 6).unwrap());
        p.opcode_costs.insert(OpcodeKind(0x0001), OpcodeCost::new(3, 0).unwrap());
        let bytes = encode(&p);
        let back: VmPolicy = decode(&bytes).unwrap();
        assert_eq!(p, back);
        // Canonical order preserved.
        let keys: Vec<_> = back.opcode_costs.keys().collect();
        assert_eq!(keys, vec![&OpcodeKind(0x0001), &OpcodeKind(0x0301)]);
    }
}
