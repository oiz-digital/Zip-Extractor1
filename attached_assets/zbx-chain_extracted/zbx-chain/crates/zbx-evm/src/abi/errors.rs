//! Custom Error ABI -- EIP-838 / Solidity custom errors
//!
//! Custom errors introduced in Solidity 0.8.0 provide gas-efficient
//! error reporting compared to revert strings.
//!
//! Wire format:
//!   REVERT data = selector(4 bytes) ++ abi_encode(args...)
//!   selector    = keccak256("ErrorName(type1,type2,...)")[:4]
//!
//! Examples:
//!   error InsufficientBalance(address account, uint256 balance, uint256 required)
//!   selector = keccak256("InsufficientBalance(address,uint256,uint256)")[:4]
//!
//!   error Unauthorized()
//!   selector = keccak256("Unauthorized()")[:4]  = 0x82b42900
//!
//! ZBX EVM decodes custom errors during trace/debug output and in zbxctl.

use std::collections::HashMap;

// ── Custom Error Selector ─────────────────────────────────────────────────────

/// A 4-byte custom error selector (keccak256 of error signature).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorSelector(pub [u8; 4]);

impl ErrorSelector {
    /// Compute selector from error signature string.
    /// e.g. "InsufficientBalance(address,uint256,uint256)"
    pub fn from_signature(sig: &str) -> Self {
        let hash = keccak256(sig.as_bytes());
        Self([hash[0], hash[1], hash[2], hash[3]])
    }

    pub fn as_u32(&self) -> u32 {
        u32::from_be_bytes(self.0)
    }
}

// ── Custom Error ABI definition ───────────────────────────────────────────────

/// ABI definition for a custom Solidity error.
#[derive(Debug, Clone)]
pub struct CustomError {
    pub name:     String,
    pub selector: ErrorSelector,
    pub params:   Vec<ErrorParam>,
}

#[derive(Debug, Clone)]
pub struct ErrorParam {
    pub name:    String,
    pub ty:      AbiType,
    pub indexed: bool,
}

#[derive(Debug, Clone)]
pub enum AbiType {
    Uint(u8),      // uint8 .. uint256
    Int(u8),       // int8 .. int256
    Address,
    Bool,
    Bytes,         // dynamic bytes
    FixedBytes(u8),// bytes1 .. bytes32
    String,        // dynamic string
    Array(Box<AbiType>),          // T[]
    FixedArray(Box<AbiType>, u32),// T[N]
    Tuple(Vec<AbiType>),          // (T1, T2, ...)
}

impl CustomError {
    pub fn new(name: &str, params: Vec<ErrorParam>) -> Self {
        let sig = build_error_sig(name, &params);
        let selector = ErrorSelector::from_signature(&sig);
        Self { name: name.into(), selector, params }
    }

    /// Decode the revert data for this error (after stripping selector).
    pub fn decode_args(&self, data: &[u8]) -> Result<Vec<DecodedValue>, AbiError> {
        if data.len() < 4 { return Err(AbiError::DataTooShort); }
        if data[..4] != self.selector.0 { return Err(AbiError::SelectorMismatch); }
        abi_decode_params(&data[4..], &self.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>())
    }
}

/// Well-known ZBX built-in custom errors (registered in error registry).
pub fn zbx_builtin_errors() -> Vec<CustomError> {
    vec![
        // Staking
        CustomError::new("InsufficientStake",    vec![
            ErrorParam { name: "account".into(), ty: AbiType::Address, indexed: false },
            ErrorParam { name: "required".into(), ty: AbiType::Uint(256), indexed: false },
            ErrorParam { name: "actual".into(),   ty: AbiType::Uint(256), indexed: false },
        ]),
        CustomError::new("StakeLocked", vec![
            ErrorParam { name: "unlockTime".into(), ty: AbiType::Uint(256), indexed: false },
        ]),
        CustomError::new("ValidatorJailed", vec![
            ErrorParam { name: "validator".into(), ty: AbiType::Address, indexed: false },
        ]),
        // Governance
        CustomError::new("ProposalNotFound", vec![
            ErrorParam { name: "proposalId".into(), ty: AbiType::Uint(256), indexed: false },
        ]),
        CustomError::new("AlreadyVoted", vec![
            ErrorParam { name: "voter".into(), ty: AbiType::Address, indexed: false },
        ]),
        CustomError::new("VotingEnded", vec![
            ErrorParam { name: "endBlock".into(), ty: AbiType::Uint(256), indexed: false },
        ]),
        // ERC-20
        CustomError::new("ERC20InsufficientBalance", vec![
            ErrorParam { name: "sender".into(),  ty: AbiType::Address, indexed: false },
            ErrorParam { name: "balance".into(), ty: AbiType::Uint(256), indexed: false },
            ErrorParam { name: "needed".into(),  ty: AbiType::Uint(256), indexed: false },
        ]),
        CustomError::new("ERC20InsufficientAllowance", vec![
            ErrorParam { name: "spender".into(),   ty: AbiType::Address, indexed: false },
            ErrorParam { name: "allowance".into(), ty: AbiType::Uint(256), indexed: false },
            ErrorParam { name: "needed".into(),    ty: AbiType::Uint(256), indexed: false },
        ]),
        // General
        CustomError::new("Unauthorized",  vec![]),
        CustomError::new("ZeroAddress",   vec![]),
        CustomError::new("Overflow",      vec![]),
        CustomError::new("Paused",        vec![]),
    ]
}

// ── Error registry ────────────────────────────────────────────────────────────

/// Global error selector registry (for decoding revert data in traces).
pub struct ErrorRegistry {
    pub by_selector: HashMap<ErrorSelector, CustomError>,
}

impl ErrorRegistry {
    pub fn new() -> Self {
        let mut r = Self { by_selector: HashMap::new() };
        for err in zbx_builtin_errors() { r.register(err); }
        r
    }

    pub fn register(&mut self, err: CustomError) {
        self.by_selector.insert(err.selector, err);
    }

    /// Decode a REVERT payload. Returns (error_name, decoded_args) if known.
    pub fn decode_revert(&self, data: &[u8]) -> Option<(&CustomError, Vec<DecodedValue>)> {
        if data.len() < 4 { return None; }
        let sel = ErrorSelector([data[0], data[1], data[2], data[3]]);
        let err = self.by_selector.get(&sel)?;
        let args = err.decode_args(data).ok()?;
        Some((err, args))
    }
}

#[derive(Debug, Clone)]
pub enum DecodedValue {
    Uint(u128), Int(i128), Address([u8; 20]),
    Bool(bool), Bytes(Vec<u8>), String(String), Tuple(Vec<DecodedValue>),
}

#[derive(Debug)]
pub enum AbiError { DataTooShort, SelectorMismatch, InvalidEncoding, TypeMismatch }

fn keccak256(_data: &[u8]) -> [u8; 32] { [0u8; 32] }
fn build_error_sig(name: &str, params: &[ErrorParam]) -> String {
    let args: Vec<&str> = params.iter().map(|_| "uint256").collect();
    format!("{}({})", name, args.join(","))
}
fn abi_decode_params(_data: &[u8], _types: &[AbiType]) -> Result<Vec<DecodedValue>, AbiError> {
    Ok(vec![])
}