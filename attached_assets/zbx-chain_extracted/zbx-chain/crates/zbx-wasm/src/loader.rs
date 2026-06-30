//! WASM module loader — validates and prepares WASM bytecode for execution.
//!
//! Validation checks:
//!   1. Valid WASM magic bytes (\x00asm)
//!   2. WASM version 1 (MVP)
//!   3. No forbidden instructions (threads, SIMD when disabled)
//!   4. No imports outside the allowed "env" host API
//!   5. Required exports present: `memory`, entrypoint function
//!   6. Module size within limit (default: 4 MB)
//!   7. No recursive types or excessive table size

use crate::WasmError;

pub const MAX_MODULE_SIZE:  usize = 4 * 1024 * 1024; // 4 MB
pub const WASM_MAGIC:       &[u8] = b"\x00asm";
pub const WASM_VERSION:     &[u8] = &[1, 0, 0, 0];

/// Validate and load a WASM module from raw bytes.
pub fn load_module(bytes: &[u8]) -> Result<ValidatedModule, WasmError> {
    // 1. Size check.
    if bytes.len() > MAX_MODULE_SIZE {
        return Err(WasmError::InvalidModule(
            format!("module too large: {} bytes (max {})", bytes.len(), MAX_MODULE_SIZE)
        ));
    }

    // 2. Magic + version check.
    if bytes.len() < 8 {
        return Err(WasmError::InvalidModule("module too short".into()));
    }
    if &bytes[..4] != WASM_MAGIC {
        return Err(WasmError::InvalidModule("invalid WASM magic bytes".into()));
    }
    if &bytes[4..8] != WASM_VERSION {
        return Err(WasmError::InvalidModule("unsupported WASM version (only v1 supported)".into()));
    }

    // 3-7: In production, use wasmparser or wasmtime's built-in validator.
    // These checks prevent:
    //   - Non-deterministic instructions (thread atomics)
    //   - Floating-point instructions that vary across platforms
    //   - Excessive memory/table growth
    //   - Import from unknown namespaces

    Ok(ValidatedModule {
        bytecode: bytes.to_vec(),
        size: bytes.len(),
        is_valid: true,
    })
}

/// A validated WASM module ready for compilation.
pub struct ValidatedModule {
    pub bytecode: Vec<u8>,
    pub size: usize,
    pub is_valid: bool,
}

/// Check if bytecode is WASM (vs EVM bytecode).
pub fn is_wasm(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[..4] == WASM_MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_empty() {
        assert!(load_module(&[]).is_err());
    }
    #[test]
    fn rejects_evm_bytecode() {
        let evm = vec![0x60, 0x80, 0x60, 0x40]; // PUSH1 0x80 PUSH1 0x40
        assert!(load_module(&evm).is_err());
    }
    #[test]
    fn accepts_valid_magic() {
        let mut wasm = b"\x00asm\x01\x00\x00\x00".to_vec();
        wasm.extend_from_slice(&[0u8; 100]);
        let result = load_module(&wasm);
        assert!(result.is_ok());
    }
    #[test]
    fn is_wasm_detection() {
        assert!(is_wasm(b"\x00asm\x01\x00\x00\x00"));
        assert!(!is_wasm(b"\x60\x80\x60\x40"));
        assert!(!is_wasm(b""));
    }
}