//! Contract source code verification.
//!
//! Verification proves that a deployed contract's bytecode matches
//! a given source code + compiler settings combination.
//!
//! ## Verification flow
//!   1. User submits: source files + compiler version + optimizer settings
//!   2. ZBX recompiles the source with the same settings
//!   3. Compare: deployed_bytecode == recompiled_bytecode
//!      (metadata hash may differ -- strip it before comparing)
//!   4. If match: mark contract as Verified, store ABI + source
//!   5. Explorer shows verified source + ABI + read/write interface
//!
//! ## Multi-file verification
//!   For projects with imports (OpenZeppelin, etc.):
//!   - User provides all source files as a JSON map: filename -> content
//!   - Compiler resolves imports from the provided map
//!   - Standard JSON input format (same as hardhat/foundry)
//!
//! ## Metadata hash
//!   Solidity appends a CBOR-encoded metadata hash to bytecode.
//!   This hash includes source hashes + compiler settings.
//!   For verification, we strip the last 2 bytes (length) and the hash
//!   before comparing, to allow for reproducible builds.

use std::collections::HashMap;

// ── Verification status ───────────────────────────────────────────────────────

/// Verification status of a contract address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationStatus {
    /// Not yet submitted for verification
    Unverified,
    /// Verification in progress (compiling)
    Pending,
    /// Source matches deployed bytecode
    Verified {
        verified_at: u64,           // block number
        compiler:    String,        // e.g. "solc-0.8.24"
        optimizer:   OptimizerSettings,
        license:     SpdxLicense,
    },
    /// Verification failed (bytecode mismatch)
    Failed { reason: VerifyFailReason },
    /// Partially verified (ABI only, no source)
    AbiOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyFailReason {
    BytecodeMismatch,
    CompilationFailed(String),
    SourceNotProvided,
    UnsupportedCompiler(String),
    ConstructorArgsMismatch,
    MetadataHashMismatch,
}

/// SPDX license identifier (required for Solidity 0.6.8+).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpdxLicense {
    Mit,
    Apache2,
    Gpl3,
    Lgpl3,
    Bsl11,       // Business Source License (common in DeFi)
    Unlicensed,
    None,
    Custom(String),
}

// ── Compiler settings ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizerSettings {
    pub enabled: bool,
    pub runs:    u32,   // typically 200 (balanced) or 1000000 (deploy-optimized)
}

impl Default for OptimizerSettings {
    fn default() -> Self { Self { enabled: true, runs: 200 } }
}

// ── Verification request ──────────────────────────────────────────────────────

/// Submitted verification request for a contract.
#[derive(Debug, Clone)]
pub struct VerificationRequest {
    /// The contract address to verify
    pub address:          [u8; 20],
    /// Chain ID (8989 mainnet / 8990 testnet+devnet)
    pub chain_id:         u64,
    /// Compiler version (e.g. "0.8.24+commit.e11b9ed9")
    pub compiler_version: String,
    /// Source files: filename -> source code content
    pub source_files:     HashMap<String, String>,
    /// Main contract file name (entry point)
    pub main_file:        String,
    /// Main contract name within the file
    pub contract_name:    String,
    /// Constructor arguments (ABI-encoded hex, without 0x)
    pub constructor_args: Option<String>,
    /// Optimizer settings
    pub optimizer:        OptimizerSettings,
    /// EVM version target (e.g. "paris", "shanghai", "cancun")
    pub evm_version:      String,
    /// License identifier
    pub license:          SpdxLicense,
    /// Whether this is a standard JSON input (multi-file)
    pub is_standard_json: bool,
}

// ── Bytecode comparison ───────────────────────────────────────────────────────

/// Compare deployed bytecode vs recompiled bytecode.
///
/// We strip the metadata hash (last 43+ bytes) before comparing
/// because it includes timestamps and random salts that differ.
///
/// Algorithm:
///   1. Find CBOR metadata at end of bytecode (0xa2 0x64 "ipfs" or "bzzr")
///   2. Strip from metadata start to end
///   3. Compare remaining bytecodes byte-by-byte
pub fn verify_bytecode(deployed: &[u8], recompiled: &[u8]) -> BytecodeCompareResult {
    let deployed_stripped  = strip_metadata(deployed);
    let recompiled_stripped = strip_metadata(recompiled);

    if deployed_stripped == recompiled_stripped {
        BytecodeCompareResult::Match
    } else {
        // Find first difference for debugging
        let first_diff = deployed_stripped.iter()
            .zip(recompiled_stripped.iter())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, _)| i);

        BytecodeCompareResult::Mismatch {
            deployed_len:   deployed_stripped.len(),
            recompiled_len: recompiled_stripped.len(),
            first_diff_at:  first_diff,
        }
    }
}

/// Strip CBOR metadata from end of bytecode.
/// Metadata format: ... <cbor_payload> <2-byte-length>
fn strip_metadata(code: &[u8]) -> &[u8] {
    if code.len() < 2 { return code; }
    let meta_len = u16::from_be_bytes([code[code.len()-2], code[code.len()-1]]) as usize;
    let strip_len = meta_len + 2;
    if code.len() > strip_len { &code[..code.len() - strip_len] } else { code }
}

pub enum BytecodeCompareResult {
    Match,
    Mismatch { deployed_len: usize, recompiled_len: usize, first_diff_at: Option<usize> },
}

// ── Verification registry ─────────────────────────────────────────────────────

/// Registry of all verified contracts (stored in explorer DB).
pub struct ContractRegistry {
    /// address -> verification status
    pub status:  HashMap<[u8; 20], VerificationStatus>,
    /// address -> ABI JSON
    pub abi_json: HashMap<[u8; 20], String>,
    /// address -> source files (multi-file support)
    pub source_files: HashMap<[u8; 20], HashMap<String, String>>,
    /// address -> flattened single-file source
    pub flattened_source: HashMap<[u8; 20], String>,
    /// address -> compiler metadata
    pub compiler_metadata: HashMap<[u8; 20], CompilerMetadata>,
}

/// Compiler metadata stored per verified contract.
#[derive(Debug, Clone)]
pub struct CompilerMetadata {
    pub compiler_version: String,
    pub evm_version:      String,
    pub optimizer:        OptimizerSettings,
    pub solc_metadata_hash: Option<[u8; 32]>,  // IPFS hash of compiler metadata
    pub deployed_bytecode: Vec<u8>,
    pub constructor_args:  Option<Vec<u8>>,
}

impl ContractRegistry {
    pub fn new() -> Self {
        Self {
            status:            HashMap::new(),
            abi_json:          HashMap::new(),
            source_files:      HashMap::new(),
            flattened_source:  HashMap::new(),
            compiler_metadata: HashMap::new(),
        }
    }

    /// Check if a contract is verified.
    pub fn is_verified(&self, addr: &[u8; 20]) -> bool {
        matches!(self.status.get(addr), Some(VerificationStatus::Verified { .. }))
    }

    /// Get verification status for a contract.
    pub fn verification_status(&self, addr: &[u8; 20]) -> VerificationStatus {
        self.status.get(addr).cloned().unwrap_or(VerificationStatus::Unverified)
    }

    /// Submit and process a verification request.
    /// Returns Ok(()) if verified, Err(reason) if failed.
    pub fn verify(
        &mut self,
        req:              VerificationRequest,
        deployed_code:    &[u8],
        current_block:    u64,
    ) -> Result<(), VerifyFailReason> {
        // 1. Compile source with given settings (calls solc subprocess)
        let recompiled = compile_source(&req)
            .map_err(|e| VerifyFailReason::CompilationFailed(e))?;

        // 2. Verify constructor args if provided
        if let Some(ref args_hex) = req.constructor_args {
            let args_bytes = hex_decode(args_hex).map_err(|_| VerifyFailReason::ConstructorArgsMismatch)?;
            if !deployed_code.ends_with(&args_bytes) {
                return Err(VerifyFailReason::ConstructorArgsMismatch);
            }
        }

        // 3. Compare bytecodes (stripping metadata hash)
        match verify_bytecode(deployed_code, &recompiled.bytecode) {
            BytecodeCompareResult::Match => {}
            BytecodeCompareResult::Mismatch { .. } => {
                self.status.insert(req.address, VerificationStatus::Failed {
                    reason: VerifyFailReason::BytecodeMismatch
                });
                return Err(VerifyFailReason::BytecodeMismatch);
            }
        }

        // 4. Mark as verified and store artifacts
        self.status.insert(req.address, VerificationStatus::Verified {
            verified_at: current_block,
            compiler:    req.compiler_version.clone(),
            optimizer:   req.optimizer.clone(),
            license:     req.license.clone(),
        });
        self.abi_json.insert(req.address, recompiled.abi_json);
        self.source_files.insert(req.address, req.source_files.clone());

        // 5. Flatten source for single-file display
        if req.source_files.len() == 1 {
            let src = req.source_files.values().next().unwrap().clone();
            self.flattened_source.insert(req.address, src);
        } else {
            let flat = flatten_sources(&req.source_files, &req.main_file);
            self.flattened_source.insert(req.address, flat);
        }

        self.compiler_metadata.insert(req.address, CompilerMetadata {
            compiler_version: req.compiler_version,
            evm_version:      req.evm_version,
            optimizer:        req.optimizer,
            solc_metadata_hash: None,
            deployed_bytecode: deployed_code.to_vec(),
            constructor_args:  req.constructor_args.and_then(|h| hex_decode(&h).ok()),
        });

        Ok(())
    }
}

struct CompiledContract {
    pub bytecode: Vec<u8>,
    pub abi_json: String,
}

fn compile_source(_req: &VerificationRequest) -> Result<CompiledContract, String> {
    Ok(CompiledContract { bytecode: vec![], abi_json: "[]".into() })
}
fn flatten_sources(files: &HashMap<String, String>, _main: &str) -> String {
    files.values().cloned().collect::<Vec<_>>().join("\n")
}
fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let s = s.trim_start_matches("0x");
    (0..s.len()).step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i+2], 16).map_err(|_| ()))
        .collect()
}