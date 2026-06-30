//! zbx-keygen — Zebvix validator key generator
//!
//! Generates a fresh BLS-12-381 keypair (for consensus signing) and an
//! secp256k1 keypair (for node identity / EVM address).
//!
//! Usage:
//!   zbx-keygen [--count N] [--output json|text]
//!
//! Output (text, default):
//!   === Validator Keypair #1 ===
//!   EVM Address  : 0x...
//!   BLS PubKey   : 0x...  (48 bytes, put in genesis validators[] + node.toml)
//!   BLS PrivKey  : 0x...  (32 bytes, set as VALIDATOR_KEY env var — KEEP SECRET)
//!   Node PrivKey : 0x...  (32 bytes, secp256k1 — for P2P identity — KEEP SECRET)
//!
//! Output (json):
//!   [{"evm_address":"0x...","bls_pubkey":"0x...","bls_privkey":"0x...","node_privkey":"0x..."}]
//!
//! SECURITY: Private keys are printed to stdout only. Never log or commit them.

use clap::Parser;
use rand::rngs::OsRng;
use zbx_crypto::{
    bls::BlsPrivKey,
    secp256k1::PrivKey as Secp256k1PrivKey,
};

#[derive(Debug, Parser)]
#[command(
    name = "zbx-keygen",
    version = "0.2.0",
    about = "Generate Zebvix validator BLS + secp256k1 keypairs"
)]
struct Cli {
    /// Number of keypairs to generate.
    #[arg(long, short = 'n', default_value = "1")]
    count: usize,

    /// Output format: `text` (human-readable) or `json`.
    #[arg(long, short = 'o', default_value = "text")]
    output: String,
}

struct KeySet {
    evm_address:  String,
    bls_pubkey:   String,
    bls_privkey:  String,
    node_privkey: String,
}

fn generate_keyset() -> KeySet {
    let mut rng = OsRng;

    // ── BLS-12-381 keypair (consensus signing) ────────────────────────────
    let bls_priv = BlsPrivKey::generate(&mut rng);
    let bls_pub  = bls_priv.to_pubkey();

    // ── secp256k1 keypair (node identity + EVM address) ──────────────────
    let node_priv = Secp256k1PrivKey::random();
    let evm_addr  = node_priv.to_address();

    KeySet {
        evm_address:  format!("{evm_addr}"),
        bls_pubkey:   format!("0x{}", hex::encode(bls_pub.as_bytes())),
        bls_privkey:  format!("0x{}", hex::encode(bls_priv.as_bytes())),
        node_privkey: format!("0x{}", hex::encode(node_priv.as_bytes())),
    }
}

fn main() {
    let cli = Cli::parse();
    let count = cli.count.max(1);
    let sets: Vec<KeySet> = (0..count).map(|_| generate_keyset()).collect();

    match cli.output.as_str() {
        "json" => {
            let entries: Vec<String> = sets
                .iter()
                .map(|k| {
                    format!(
                        r#"{{"evm_address":"{evm}","bls_pubkey":"{bpk}","bls_privkey":"{bsk}","node_privkey":"{nsk}"}}"#,
                        evm = k.evm_address,
                        bpk = k.bls_pubkey,
                        bsk = k.bls_privkey,
                        nsk = k.node_privkey,
                    )
                })
                .collect();
            println!("[{}]", entries.join(",\n "));
        }
        _ => {
            for (i, k) in sets.iter().enumerate() {
                println!();
                println!("=== Validator Keypair #{} ===", i + 1);
                println!();
                println!("  EVM Address   : {}", k.evm_address);
                println!("  BLS PubKey    : {}", k.bls_pubkey);
                println!();
                println!("  !! KEEP THESE PRIVATE !! — never commit or log:");
                println!("  BLS PrivKey   : {}", k.bls_privkey);
                println!("  Node PrivKey  : {}", k.node_privkey);
                println!();
                println!("  --- What to do with these values ---");
                println!("  1. Set VALIDATOR_KEY={} on this validator's VPS.", k.bls_privkey);
                println!("  2. Add to genesis validators[] (address only):");
                println!("       \"{}\"", k.evm_address);
                println!("  3. For OTHER validators' node.toml [[chain.extra_validators]]:");
                println!("       address    = \"{}\"", k.evm_address);
                println!("       bls_pubkey = \"{}\"", k.bls_pubkey);
            }
            println!();
            println!("--- Genesis alloc snippet (paste into testnet-genesis.json) ---");
            for k in &sets {
                println!(
                    r#"  {{"address": "{}", "balance": "10000000000000000000000", "nonce": 0}},"#,
                    k.evm_address
                );
            }
            println!();
            println!("--- testnet-genesis.json validators[] snippet ---");
            println!("  \"validators\": [");
            for (i, k) in sets.iter().enumerate() {
                let comma = if i + 1 < sets.len() { "," } else { "" };
                println!("    \"{}\"{}",  k.evm_address, comma);
            }
            println!("  ]");
        }
    }
}
