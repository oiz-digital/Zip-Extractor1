//! File-based INT8 weight loader for ZBX AI precompile models.
//!
//! # Format
//!
//! Each model's weights are stored as a single binary file:
//! `<model_dir>/<model_name>.zbxw`
//!
//! File layout (little-endian):
//! ```text
//! [magic: 4 bytes]  = 0x5A42_5857  ("ZBXW")
//! [version: 1 byte] = 0x01
//! [model_id: 1 byte]
//! [in_size:  2 bytes LE u16]
//! [hidden:   2 bytes LE u16]
//! [out_size: 2 bytes LE u16]
//! [da_hash:  32 bytes SHA3-256 of (layer1_weights || layer2_weights)]
//! [layer1_weights: in_size * hidden bytes i8, row-major]
//! [layer1_biases:  hidden * 4 bytes i32 LE]
//! [layer2_weights: hidden * out_size bytes i8, row-major]
//! [layer2_biases:  out_size * 4 bytes i32 LE]
//! ```
//!
//! # Production workflow
//!
//! 1. Train model offline, quantize to INT8.
//! 2. Export with `zbx-model-export` CLI (ships with the training repo).
//! 3. Compute SHA3-256 of raw weight bytes, publish hash on DA layer.
//! 4. Place `.zbxw` files in `ZBX_MODEL_DIR` (default `/etc/zbx/models/`).
//! 5. Node loads weights at startup via `WeightLoader::load_all()`.
//!
//! # Devnet fallback
//!
//! If a weight file is absent or fails validation, `WeightLoader` falls
//! back to `stub_network()` and logs a warning. On `ZBX_CHAIN_ENV=mainnet`
//! the node panics instead of falling back.

use crate::{
    engine::{Int8Linear, Int8Network, stub_network},
    error::AiError,
    model::{ModelId, ModelMeta},
};
use sha3::{Digest, Sha3_256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Magic bytes at the start of every `.zbxw` file.
const MAGIC: [u8; 4] = [0x5A, 0x42, 0x58, 0x57]; // "ZBXW"
const FORMAT_VERSION: u8 = 0x01;

/// Fixed header size before the weight payload begins.
///   4 (magic) + 1 (version) + 1 (model_id) + 2 (in) + 2 (hidden) + 2 (out) + 32 (hash) = 44
const HEADER_SIZE: usize = 44;

/// Default directory where weight files are expected on a production node.
const DEFAULT_MODEL_DIR: &str = "/etc/zbx/models";

// ── WeightLoadError ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum WeightLoadError {
    FileNotFound(String),
    TooShort { path: String, got: usize },
    BadMagic([u8; 4]),
    BadVersion(u8),
    ModelIdMismatch { file: u8, expected: u8 },
    SizeMismatch { field: &'static str, file: usize, expected: usize },
    HashMismatch { computed: String, stored: String },
    InvalidWeightRow { layer: usize, row: usize },
    Io(String),
}

impl std::fmt::Display for WeightLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileNotFound(p)     => write!(f, "weight file not found: {p}"),
            Self::TooShort { path, got } => write!(f, "{path}: file too short ({got} bytes)"),
            Self::BadMagic(m)         => write!(f, "bad magic {:?}", m),
            Self::BadVersion(v)       => write!(f, "unsupported weight format version {v}"),
            Self::ModelIdMismatch { file, expected } =>
                write!(f, "model_id mismatch: file={file:#04x} expected={expected:#04x}"),
            Self::SizeMismatch { field, file, expected } =>
                write!(f, "{field} mismatch: file={file} expected={expected}"),
            Self::HashMismatch { computed, stored } =>
                write!(f, "weight hash mismatch: computed={computed} stored={stored}"),
            Self::InvalidWeightRow { layer, row } =>
                write!(f, "invalid weight row: layer={layer} row={row}"),
            Self::Io(e)               => write!(f, "io error: {e}"),
        }
    }
}

// ── WeightLoader ─────────────────────────────────────────────────────────────

/// Loads INT8 weight files from disk for all 12 ZBX AI models.
pub struct WeightLoader {
    model_dir: PathBuf,
}

impl WeightLoader {
    /// Create a loader pointing at the default model directory.
    ///
    /// Respects the `ZBX_MODEL_DIR` environment variable.
    pub fn new() -> Self {
        let dir = std::env::var("ZBX_MODEL_DIR")
            .unwrap_or_else(|_| DEFAULT_MODEL_DIR.to_owned());
        Self { model_dir: PathBuf::from(dir) }
    }

    /// Create a loader pointing at a specific directory (useful for tests).
    pub fn from_dir<P: AsRef<Path>>(dir: P) -> Self {
        Self { model_dir: dir.as_ref().to_path_buf() }
    }

    /// Load a single model's weights from its `.zbxw` file.
    ///
    /// Returns `Ok(Int8Network)` on success, or `Err(WeightLoadError)` if the
    /// file is missing, malformed, or fails the hash check.
    pub fn load_model(&self, meta: &ModelMeta) -> Result<Int8Network, WeightLoadError> {
        let filename = format!("{}.zbxw", meta.name);
        let path = self.model_dir.join(&filename);

        let bytes = std::fs::read(&path)
            .map_err(|e| WeightLoadError::FileNotFound(format!("{}: {e}", path.display())))?;

        self.parse_weight_file(&bytes, meta)
    }

    /// Load all models. Missing files fall back to `stub_network` when
    /// `allow_stubs = true`; on mainnet pass `false` to panic instead.
    pub fn load_all(
        &self,
        metas: &[ModelMeta],
        allow_stubs: bool,
    ) -> HashMap<ModelId, Int8Network> {
        let mut map = HashMap::new();
        for meta in metas {
            match self.load_model(meta) {
                Ok(net) => {
                    tracing::info!(
                        model = meta.name,
                        "AI: loaded real INT8 weights from disk"
                    );
                    map.insert(meta.id, net);
                }
                Err(e) => {
                    if !allow_stubs {
                        panic!(
                            "SECURITY: model '{}' weight file missing/invalid on mainnet: {}. \
                             Place the .zbxw file in {:?} or set ZBX_MODEL_DIR.",
                            meta.name, e, self.model_dir
                        );
                    }
                    tracing::warn!(
                        model = meta.name,
                        error = %e,
                        "AI: weight file missing — using stub_network (devnet only)"
                    );
                    let stub = stub_network(
                        meta.id as u8,
                        meta.input_size,
                        meta.hidden_size,
                        meta.num_classes,
                    )
                    .expect("stub_network parameters are always valid");
                    map.insert(meta.id, stub);
                }
            }
        }
        map
    }

    // ── Parse ─────────────────────────────────────────────────────────────────

    fn parse_weight_file(&self, bytes: &[u8], meta: &ModelMeta) -> Result<Int8Network, WeightLoadError> {
        if bytes.len() < HEADER_SIZE {
            return Err(WeightLoadError::TooShort {
                path: meta.name.to_owned(),
                got: bytes.len(),
            });
        }

        // Magic
        if bytes[0..4] != MAGIC {
            return Err(WeightLoadError::BadMagic(bytes[0..4].try_into().unwrap()));
        }

        // Version
        let version = bytes[4];
        if version != FORMAT_VERSION {
            return Err(WeightLoadError::BadVersion(version));
        }

        // Model ID
        let file_model_id = bytes[5];
        if file_model_id != meta.id as u8 {
            return Err(WeightLoadError::ModelIdMismatch {
                file:     file_model_id,
                expected: meta.id as u8,
            });
        }

        // Sizes
        let in_size  = u16::from_le_bytes([bytes[6],  bytes[7]])  as usize;
        let hidden   = u16::from_le_bytes([bytes[8],  bytes[9]])  as usize;
        let out_size = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;

        if in_size != meta.input_size {
            return Err(WeightLoadError::SizeMismatch {
                field: "in_size", file: in_size, expected: meta.input_size,
            });
        }
        if out_size != meta.num_classes {
            return Err(WeightLoadError::SizeMismatch {
                field: "out_size", file: out_size, expected: meta.num_classes,
            });
        }

        // Stored DA hash (bytes 12..44)
        let stored_hash: [u8; 32] = bytes[12..44].try_into().unwrap();

        // Weight payload layout:
        //   L1 weights: hidden * in_size  bytes (i8)
        //   L1 biases:  hidden * 4        bytes (i32 LE)
        //   L2 weights: out_size * hidden bytes (i8)
        //   L2 biases:  out_size * 4      bytes (i32 LE)
        let l1w_len  = hidden * in_size;
        let l1b_len  = hidden * 4;
        let l2w_len  = out_size * hidden;
        let l2b_len  = out_size * 4;
        let payload_len = l1w_len + l1b_len + l2w_len + l2b_len;
        let expected_total = HEADER_SIZE + payload_len;

        if bytes.len() < expected_total {
            return Err(WeightLoadError::TooShort {
                path: meta.name.to_owned(),
                got: bytes.len(),
            });
        }

        let payload = &bytes[HEADER_SIZE..HEADER_SIZE + payload_len];

        // Hash check — SHA3-256 of raw payload must match stored header hash.
        let computed_hash: [u8; 32] = Sha3_256::digest(payload).into();
        if computed_hash != stored_hash {
            return Err(WeightLoadError::HashMismatch {
                computed: hex::encode(computed_hash),
                stored:   hex::encode(stored_hash),
            });
        }

        // Parse layer 1 weights
        let mut offset = 0usize;
        let l1_weights: Vec<Vec<i8>> = (0..hidden)
            .map(|row| {
                let slice = &payload[offset + row * in_size..offset + (row + 1) * in_size];
                slice.iter().map(|&b| b as i8).collect()
            })
            .collect();
        offset += l1w_len;

        // Parse layer 1 biases
        let l1_biases: Vec<i32> = (0..hidden)
            .map(|i| {
                let o = offset + i * 4;
                i32::from_le_bytes([payload[o], payload[o+1], payload[o+2], payload[o+3]])
            })
            .collect();
        offset += l1b_len;

        // Parse layer 2 weights
        let l2_weights: Vec<Vec<i8>> = (0..out_size)
            .map(|row| {
                let slice = &payload[offset + row * hidden..offset + (row + 1) * hidden];
                slice.iter().map(|&b| b as i8).collect()
            })
            .collect();
        offset += l2w_len;

        // Parse layer 2 biases
        let l2_biases: Vec<i32> = (0..out_size)
            .map(|i| {
                let o = offset + i * 4;
                i32::from_le_bytes([payload[o], payload[o+1], payload[o+2], payload[o+3]])
            })
            .collect();

        let l1 = Int8Linear::new(l1_weights, l1_biases)
            .map_err(|e| WeightLoadError::Io(e.to_string()))?;
        let l2 = Int8Linear::new(l2_weights, l2_biases)
            .map_err(|e| WeightLoadError::Io(e.to_string()))?;

        Ok(Int8Network::new(l1, l2))
    }

    // ── Writer (for tooling / export CLI) ────────────────────────────────────

    /// Serialize an `Int8Network` into the `.zbxw` binary format.
    ///
    /// Used by the `zbx-model-export` CLI. Returns the raw bytes; the caller
    /// is responsible for writing them to `<model_dir>/<model_name>.zbxw`.
    pub fn serialize(meta: &ModelMeta, net: &Int8Network) -> Vec<u8> {
        let in_size  = net.layer1.in_size;
        let hidden   = net.layer1.out_size;
        let out_size = net.layer2.out_size;

        // Build payload first so we can hash it.
        let mut payload = Vec::new();

        // L1 weights (row-major i8)
        for row in &net.layer1.weights {
            payload.extend(row.iter().map(|&b| b as u8));
        }
        // L1 biases (i32 LE)
        for &b in &net.layer1.biases {
            payload.extend_from_slice(&b.to_le_bytes());
        }
        // L2 weights
        for row in &net.layer2.weights {
            payload.extend(row.iter().map(|&b| b as u8));
        }
        // L2 biases
        for &b in &net.layer2.biases {
            payload.extend_from_slice(&b.to_le_bytes());
        }

        let da_hash: [u8; 32] = Sha3_256::digest(&payload).into();

        let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(FORMAT_VERSION);
        out.push(meta.id as u8);
        out.extend_from_slice(&(in_size  as u16).to_le_bytes());
        out.extend_from_slice(&(hidden   as u16).to_le_bytes());
        out.extend_from_slice(&(out_size as u16).to_le_bytes());
        out.extend_from_slice(&da_hash);
        out.extend_from_slice(&payload);
        out
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::stub_network,
        model::{ModelId, ModelMeta},
    };
    use tempfile::TempDir;

    fn meta_for(id: ModelId) -> ModelMeta { ModelMeta::stub(id) }

    fn write_weight_file(dir: &Path, meta: &ModelMeta, net: &Int8Network) {
        let bytes = WeightLoader::serialize(meta, net);
        let path  = dir.join(format!("{}.zbxw", meta.name));
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn roundtrip_serialize_parse() {
        let meta = meta_for(ModelId::SpamClassifier);
        let original = stub_network(
            meta.id as u8, meta.input_size, meta.hidden_size, meta.num_classes,
        ).unwrap();

        let tmp = TempDir::new().unwrap();
        write_weight_file(tmp.path(), &meta, &original);

        let loader = WeightLoader::from_dir(tmp.path());
        let loaded = loader.load_model(&meta).expect("load_model must succeed");

        // Both networks must produce the same output on the same input.
        let input: Vec<i8> = vec![10i8; meta.input_size];
        let r_orig   = original.infer(&input).unwrap();
        let r_loaded = loaded.infer(&input).unwrap();
        assert_eq!(r_orig, r_loaded, "loaded network must match original");
    }

    #[test]
    fn missing_file_returns_error() {
        let meta   = meta_for(ModelId::RiskScorer);
        let tmp    = TempDir::new().unwrap();
        let loader = WeightLoader::from_dir(tmp.path());
        assert!(
            loader.load_model(&meta).is_err(),
            "missing weight file must return Err"
        );
    }

    #[test]
    fn tampered_payload_fails_hash_check() {
        let meta = meta_for(ModelId::NftTagger);
        let net  = stub_network(
            meta.id as u8, meta.input_size, meta.hidden_size, meta.num_classes,
        ).unwrap();

        let tmp  = TempDir::new().unwrap();
        let path = tmp.path().join(format!("{}.zbxw", meta.name));
        let mut bytes = WeightLoader::serialize(&meta, &net);

        // Flip a byte deep in the payload (past the header).
        bytes[HEADER_SIZE + 10] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();

        let loader = WeightLoader::from_dir(tmp.path());
        let result = loader.load_model(&meta);
        assert!(
            matches!(result, Err(WeightLoadError::HashMismatch { .. })),
            "tampered payload must fail hash check, got: {:?}", result
        );
    }

    #[test]
    fn load_all_stubs_when_no_files() {
        let tmp    = TempDir::new().unwrap();
        let loader = WeightLoader::from_dir(tmp.path());
        let metas: Vec<_> = ModelId::all().iter()
            .map(|&id| ModelMeta::stub(id))
            .collect();

        let nets = loader.load_all(&metas, true);
        assert_eq!(nets.len(), 12, "must return networks for all 12 models");
    }

    #[test]
    fn bad_magic_rejected() {
        let meta = meta_for(ModelId::GasOptimizer);
        let net  = stub_network(
            meta.id as u8, meta.input_size, meta.hidden_size, meta.num_classes,
        ).unwrap();
        let mut bytes = WeightLoader::serialize(&meta, &net);
        bytes[0] = 0x00; // corrupt magic

        let tmp  = TempDir::new().unwrap();
        let path = tmp.path().join(format!("{}.zbxw", meta.name));
        std::fs::write(path, bytes).unwrap();

        let loader = WeightLoader::from_dir(tmp.path());
        assert!(
            matches!(loader.load_model(&meta), Err(WeightLoadError::BadMagic(_))),
            "bad magic must be rejected"
        );
    }
}
