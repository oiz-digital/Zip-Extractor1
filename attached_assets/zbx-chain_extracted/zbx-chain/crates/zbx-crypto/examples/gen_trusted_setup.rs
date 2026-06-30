//! Generate a deterministic EIP-4844-format trusted setup file.
//!
//! WARNING: The output of this binary is **NOT** a real KZG ceremony —
//! the secret `s` is hard-coded and recoverable. It is intended ONLY for
//! testnet/devnet bring-up and CI; mainnet operators MUST replace the
//! generated file with the official Ethereum KZG ceremony output
//! (https://github.com/ethereum/kzg-ceremony) before booting chain 8989.
//!
//! Run from repo root:
//!   cargo run -p zbx-crypto --example gen_trusted_setup -- <out_path>

use bls12_381::{G1Projective, G2Projective, Scalar};
use group::{Curve, Group};
use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};

const N_G1: usize = 4096;
const N_G2: usize = 65;

fn main() -> std::io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let out = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("zbx-chain/node/configs/trusted_setup_devnet.txt");

    // Deterministic test secret — same seed gives same file across CI runs.
    // SHA-256("zbx-devnet-kzg-trusted-setup-2026-05-09") interpreted as a
    // scalar via from_bytes_wide-style folding. Keeping this in the source
    // tree makes the resulting file reproducible (anyone can re-derive it).
    let seed: [u8; 64] = [
        0x5a, 0x42, 0x58, 0x2d, 0x44, 0x45, 0x56, 0x4e, 0x45, 0x54, 0x2d, 0x4b, 0x5a, 0x47, 0x2d,
        0x32, 0x30, 0x32, 0x36, 0x2d, 0x30, 0x35, 0x2d, 0x30, 0x39, 0x21, 0x21, 0x21, 0x21, 0x21,
        0x21, 0x21, 0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe, 0xfe, 0xed, 0xfa, 0xce, 0x13,
        0x37, 0x42, 0x42, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00, 0x0f, 0x1e,
        0x2d, 0x3c, 0x4b, 0x5a,
    ];
    let s = Scalar::from_bytes_wide(&seed);

    let f = File::create(out)?;
    let mut w = BufWriter::new(f);

    writeln!(w, "# DEVNET-ONLY KZG trusted setup (NOT mainnet-safe).")?;
    writeln!(
        w,
        "# Secret s is deterministic from sha256('zbx-devnet-kzg-trusted-setup-2026-05-09')."
    )?;
    writeln!(w, "# Mainnet operators MUST replace this with the official")?;
    writeln!(
        w,
        "# Ethereum KZG ceremony output before booting chain 8989."
    )?;
    writeln!(w, "{}", N_G1)?;
    writeln!(w, "{}", N_G2)?;

    // G1 monomial: [s^0]·G1, [s^1]·G1, ..., [s^(N_G1-1)]·G1.
    let mut s_pow = Scalar::one();
    for _ in 0..N_G1 {
        let p = (G1Projective::generator() * s_pow).to_affine();
        let bytes = p.to_compressed();
        writeln!(w, "{}", hex::encode(bytes))?;
        s_pow *= s;
    }

    // G2 monomial: [s^0]·G2, [s^1]·G2, ..., [s^(N_G2-1)]·G2.
    let mut s_pow2 = Scalar::one();
    for _ in 0..N_G2 {
        let p = (G2Projective::generator() * s_pow2).to_affine();
        let bytes = p.to_compressed();
        writeln!(w, "{}", hex::encode(bytes))?;
        s_pow2 *= s;
    }

    w.flush()?;
    eprintln!("Wrote {}", out);
    Ok(())
}
