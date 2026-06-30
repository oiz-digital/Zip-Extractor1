//! Smart contract interaction: ABI encoding, call, send, deploy, events.

use crate::{
    error::SdkError,
    provider::Provider,
    wallet::Wallet,
    transaction::TransactionRequest,
    filter::{FilterBuilder, LogFilter},
    abi::{AbiFunction, AbiParam, encode_call, decode_output},
};
use zbx_types::{Address, U256, H256};
use serde_json::Value;

/// A deployed contract instance.
///
/// ```rust,no_run
/// use zbx_sdk::{Provider, Wallet, Contract};
/// use zbx_sdk::abi::Token;
///
/// let abi_json = include_str!("../../../contracts/ZbxStaking.abi.json");
/// let contract = Contract::new(
///     "0xStakingAddress",
///     abi_json,
///     provider.clone(),
/// );
/// let result = contract.call("totalStaked", vec![], None).await?;
/// ```
pub struct Contract {
    address:  Address,
    abi:      Vec<AbiFunction>,
    provider: Provider,
}

impl Contract {
    /// Create a new contract handle from an address and ABI JSON string.
    pub fn new(
        address:  impl Into<String>,
        abi_json: impl Into<String>,
        provider: Provider,
    ) -> Result<Self, SdkError> {
        let addr = parse_addr(address.into())?;
        let abi  = parse_abi(abi_json.into())?;
        Ok(Self { address: addr, abi, provider })
    }

    /// Deploy a contract and return the deployed `Contract` handle.
    pub async fn deploy(
        bytecode:    Vec<u8>,
        abi_json:    impl Into<String>,
        constructor: Vec<Token>,
        provider:    Provider,
        wallet:      &Wallet,
    ) -> Result<Self, SdkError> {
        let abi     = parse_abi(abi_json.into())?;
        // Append ABI-encoded constructor args.
        let mut data = bytecode;
        if !constructor.is_empty() {
            data.extend_from_slice(&encode_constructor(&constructor));
        }
        let tx = TransactionRequest::deploy(data);
        let receipt = provider.send(tx, wallet).await?
            .wait_confirmations(1).await?;
        let addr_hex = receipt["contractAddress"].as_str()
            .ok_or_else(|| SdkError::Other("no contractAddress in deploy receipt".into()))?;
        let address = parse_addr(addr_hex.into())?;
        Ok(Self { address, abi, provider })
    }

    pub fn address(&self) -> Address { self.address }

    // ── Read calls ────────────────────────────────────────────────────────────

    /// Call a `view`/`pure` function and decode the output.
    pub async fn call(
        &self,
        function: &str,
        args:     Vec<Token>,
        block:    Option<u64>,
    ) -> Result<Vec<Token>, SdkError> {
        let func     = self.find_function(function)?;
        let calldata = encode_call(&func.selector(), &args, &func.inputs)?;
        let tx = TransactionRequest::call(self.address, calldata);
        let raw_output = self.provider.call(&tx).await?;
        decode_output(&raw_output, &func.outputs)
    }

    /// Call a function and return the first decoded output token.
    pub async fn call_one(
        &self,
        function: &str,
        args:     Vec<Token>,
    ) -> Result<Token, SdkError> {
        let mut tokens = self.call(function, args, None).await?;
        tokens.into_iter().next()
            .ok_or_else(|| SdkError::Abi("function returned no values".into()))
    }

    // ── Write transactions ────────────────────────────────────────────────────

    /// Send a state-changing transaction.  Fills gas and nonce automatically.
    pub async fn send(
        &self,
        function: &str,
        args:     Vec<Token>,
        value:    Option<U256>,
        wallet:   &Wallet,
    ) -> Result<H256, SdkError> {
        let func     = self.find_function(function)?;
        let calldata = encode_call(&func.selector(), &args, &func.inputs)?;
        let tx = TransactionRequest::call(self.address, calldata)
            .value(value.unwrap_or_default())
            .eip1559();
        let signed_hash = self.provider.send(tx, wallet).await?.hash;
        Ok(signed_hash)
    }

    /// Send and wait for 1 confirmation.
    pub async fn send_and_wait(
        &self,
        function: &str,
        args:     Vec<Token>,
        value:    Option<U256>,
        wallet:   &Wallet,
    ) -> Result<Value, SdkError> {
        let func     = self.find_function(function)?;
        let calldata = encode_call(&func.selector(), &args, &func.inputs)?;
        let tx = TransactionRequest::call(self.address, calldata)
            .value(value.unwrap_or_default())
            .eip1559();
        self.provider.send(tx, wallet).await?
            .wait_confirmations(1).await
    }

    // ── Event subscription ────────────────────────────────────────────────────

    /// Build a log filter for an event emitted by this contract.
    pub fn events(&self, event_name: &str) -> FilterBuilder {
        let addr: Address = self.address;
        let sig: H256     = keccak_event_sig(event_name);
        let f: FilterBuilder = FilterBuilder::new();
        let f: FilterBuilder = FilterBuilder::address(f, addr);
        FilterBuilder::event_signature(f, sig)
    }

    // ── Utils ─────────────────────────────────────────────────────────────────

    fn find_function(&self, name: &str) -> Result<&AbiFunction, SdkError> {
        self.abi.iter().find(|f| f.name == name)
            .ok_or_else(|| SdkError::FunctionNotFound(name.into()))
    }
}

fn keccak_event_sig(sig: &str) -> H256 {
    use sha3::{Digest, Keccak256};
    let hash = Keccak256::digest(sig.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&hash);
    H256(out)
}

fn parse_addr(s: String) -> Result<Address, SdkError> {
    let clean = s.trim_start_matches("0x");
    let bytes  = hex::decode(clean).map_err(SdkError::Hex)?;
    if bytes.len() != 20 {
        return Err(SdkError::Other("address must be 20 bytes".into()));
    }
    let mut arr = [0u8; 20];
    arr.copy_from_slice(&bytes);
    Ok(Address(arr))
}

/// Parse a Solidity JSON ABI string into a list of `AbiFunction` entries.
///
/// The JSON format follows the Solidity ABI spec:
/// ```json
/// [{"type":"function","name":"transfer","inputs":[…],"outputs":[…],"stateMutability":"nonpayable"}]
/// ```
/// Non-function entries (events, constructor, errors, fallback) are skipped.
fn parse_abi(json: String) -> Result<Vec<AbiFunction>, SdkError> {
    use crate::abi::{AbiParam, ParamType};

    let items: Vec<serde_json::Value> =
        serde_json::from_str(&json).map_err(|e| SdkError::Abi(e.to_string()))?;

    let mut functions = Vec::new();
    for item in &items {
        // Only process function entries
        if item.get("type").and_then(|t| t.as_str()) != Some("function") {
            continue;
        }
        let name = item["name"]
            .as_str()
            .ok_or_else(|| SdkError::Abi("function missing 'name' field".into()))?
            .to_string();
        let state_mutability = item
            .get("stateMutability")
            .and_then(|v| v.as_str())
            .unwrap_or("nonpayable")
            .to_string();
        let inputs  = parse_abi_params(&item["inputs"])?;
        let outputs = parse_abi_params(&item["outputs"])?;
        functions.push(AbiFunction { name, inputs, outputs, state_mutability });
    }
    Ok(functions)
}

fn parse_abi_params(val: &serde_json::Value) -> Result<Vec<crate::abi::AbiParam>, SdkError> {
    use crate::abi::{AbiParam, ParamType};

    let arr = match val.as_array() {
        Some(a) => a,
        None => return Ok(Vec::new()),
    };
    arr.iter().map(|p| {
        let name     = p.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let type_str = p.get("type").and_then(|v| v.as_str()).unwrap_or("bytes");
        let indexed  = p.get("indexed").and_then(|v| v.as_bool()).unwrap_or(false);
        let internal = p.get("internalType").and_then(|v| v.as_str()).map(str::to_string);
        let ty = parse_solidity_type(type_str)
            .map_err(|e| SdkError::Abi(format!("type '{type_str}': {e}")))?;
        Ok(AbiParam { name, ty, indexed, internal })
    }).collect()
}

/// Parse a Solidity type string such as `"uint256"`, `"address[]"`, `"bytes32[4]"`.
fn parse_solidity_type(s: &str) -> Result<crate::abi::ParamType, String> {
    use crate::abi::ParamType;

    // Array: T[] or T[N]
    if s.ends_with(']') {
        let open = s.rfind('[').ok_or("unmatched ']' in type")?;
        let inner = parse_solidity_type(&s[..open])?;
        let size_str = &s[open + 1..s.len() - 1];
        return if size_str.is_empty() {
            Ok(ParamType::Array(Box::new(inner)))
        } else {
            let n = size_str.parse::<usize>().map_err(|_| "invalid array size")?;
            Ok(ParamType::FixedArray(Box::new(inner), n))
        };
    }

    // Tuple (components are not recursively parsed here; full tuple support
    // requires the nested `components` array in the ABI JSON)
    if s.starts_with("tuple") {
        return Ok(ParamType::Tuple(Vec::new()));
    }

    match s {
        "address"   => Ok(ParamType::Address),
        "bool"      => Ok(ParamType::Bool),
        "bytes"     => Ok(ParamType::Bytes),
        "string"    => Ok(ParamType::String),
        "uint"      => Ok(ParamType::Uint(256)),
        "int"       => Ok(ParamType::Int(256)),
        _ if s.starts_with("uint") => {
            let bits = s[4..].parse::<usize>().unwrap_or(256);
            Ok(ParamType::Uint(bits))
        }
        _ if s.starts_with("int") => {
            let bits = s[3..].parse::<usize>().unwrap_or(256);
            Ok(ParamType::Int(bits))
        }
        _ if s.starts_with("bytes") => {
            let n = s[5..].parse::<usize>().unwrap_or(0);
            if n == 0 || n > 32 { Ok(ParamType::Bytes) } else { Ok(ParamType::FixedBytes(n)) }
        }
        other => Err(format!("unknown Solidity type: {other}")),
    }
}

/// ABI-encode constructor arguments (no 4-byte selector prefix).
///
/// Implements the standard head/tail encoding from the Solidity ABI spec.
fn encode_constructor(args: &[Token]) -> Vec<u8> {
    abi_encode_tuple(args)
}

/// Encode a tuple of tokens using the Solidity ABI head/tail layout.
fn abi_encode_tuple(tokens: &[Token]) -> Vec<u8> {
    let head_size = tokens.len() * 32;
    let mut head = Vec::<u8>::new();
    let mut tail = Vec::<u8>::new();

    for token in tokens {
        if abi_token_is_dynamic(token) {
            // Head: 32-byte offset to this element's data in the tail
            let offset = (head_size + tail.len()) as u64;
            head.extend_from_slice(&pad32_be(offset.to_be_bytes().as_ref()));
            tail.extend(abi_encode_token(token));
        } else {
            head.extend(abi_encode_token(token));
        }
    }

    head.extend(tail);
    head
}

fn abi_token_is_dynamic(token: &Token) -> bool {
    matches!(token, Token::Bytes(_) | Token::String(_) | Token::Array(_))
}

fn abi_encode_token(token: &Token) -> Vec<u8> {
    use zbx_types::U256;
    match token {
        Token::Address(a)     => pad32_be(&a.0),
        Token::Uint(u)        => { let mut b = [0u8; 32]; u.to_big_endian(&mut b); b.to_vec() }
        Token::Int(i)         => { let mut b = [0u8; 32]; if *i < 0 { b.fill(0xff); } b[24..].copy_from_slice(&i.to_be_bytes()); b.to_vec() }
        Token::Bool(b)        => pad32_be(&[if *b { 1 } else { 0 }]),
        Token::FixedBytes(fb) => { let mut b = [0u8; 32]; b[..32].copy_from_slice(fb); b.to_vec() }
        Token::Bytes(bytes)   => {
            let mut out = pad32_be(&(bytes.len() as u64).to_be_bytes());
            out.extend_from_slice(bytes);
            // Pad to next 32-byte boundary
            let rem = bytes.len() % 32;
            if rem != 0 { out.extend(vec![0u8; 32 - rem]); }
            out
        }
        Token::String(s)      => abi_encode_token(&Token::Bytes(s.as_bytes().to_vec())),
        Token::Array(arr)     => {
            let mut out = pad32_be(&(arr.len() as u64).to_be_bytes());
            out.extend(abi_encode_tuple(arr));
            out
        }
        Token::Tuple(items)   => abi_encode_tuple(items),
    }
}

fn pad32_be(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    let start = 32usize.saturating_sub(bytes.len());
    out[start..].copy_from_slice(&bytes[..bytes.len().min(32)]);
    out
}

// Re-export Token for users of this module.
pub use crate::abi::Token;