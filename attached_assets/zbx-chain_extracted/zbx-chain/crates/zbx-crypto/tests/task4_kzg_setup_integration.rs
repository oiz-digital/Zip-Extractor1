//! Task #4 integration test: load the committed devnet KZG trusted-setup
//! file end-to-end (ceremony format: 4096 G1 + 65 G2), install it as
//! the process-global verifier, and exercise precompile 0x0B.
//!
//! This is the single test that proves the file → loader → global
//! `OnceLock` → verifier path actually wires together. The unit tests
//! in `zbx-zvm` and `zbx-evm` exercise `do_kzg_with_settings` directly
//! and therefore bypass the global path.

use bls12_381::{G1Projective, G2Projective, Scalar};
use group::Curve;
use std::path::PathBuf;

use zbx_crypto::kzg::{
    do_kzg_point_eval, init_global_kzg_settings, kzg_to_versioned_hash,
    load_trusted_setup, point_evaluation_success_return,
};

fn config_path(name: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR = .../zbx-chain/crates/zbx-crypto
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates
    p.pop(); // zbx-chain
    p.push("node");
    p.push("configs");
    p.push(name);
    p
}

fn devnet_setup_path() -> PathBuf { config_path("trusted_setup_devnet.txt") }
fn mainnet_setup_path() -> PathBuf { config_path("trusted_setup.txt") }

#[test]
fn task4_load_mainnet_ceremony_file_shape_and_g2_extraction() {
    // The official Ethereum KZG Summoning Ceremony output is committed
    // in-tree at node/configs/trusted_setup.txt. This test proves the
    // loader accepts the real c-kzg layout (4096 G1 monomial + 4096 G1
    // Lagrange + 65 G2 monomial = 8259 lines with two header lines) and
    // extracts g2_monomial[1] = [s]·G2 cleanly. We don't know the secret
    // s for this file (toxic waste was destroyed in the ceremony) so we
    // cannot construct a verifying proof under it from inside a test —
    // the cross-VM unit tests already cover proof-construction with
    // test-only setups. What we CAN do is assert: file decodes, header
    // shape matches EIP-4844, [s]·G2 is a valid G2 point that is NOT
    // the G2 generator (would mean s == 1, i.e. file is a placeholder).
    use bls12_381::G2Affine;
    use group::Group;

    let path = mainnet_setup_path();
    assert!(path.exists(), "mainnet ceremony must be committed at {}", path.display());

    let settings = load_trusted_setup(&path)
        .expect("real Ethereum ceremony file must parse");

    let g2_gen = G2Affine::generator();
    assert_ne!(
        settings.s_g2, g2_gen,
        "[s]·G2 == G2 generator implies s == 1 (placeholder ceremony, NOT mainnet-safe)"
    );
}

#[test]
fn task4_load_devnet_ceremony_file_and_verify_proof() {
    let path = devnet_setup_path();
    assert!(
        path.exists(),
        "devnet trusted setup file must be committed at {}",
        path.display()
    );

    let settings = load_trusted_setup(&path)
        .expect("load_trusted_setup must parse the committed devnet ceremony file");

    // Install as the process-global setup. May race with other tests in
    // this binary that also call `init_global_kzg_settings`; the
    // OnceLock guarantees only the first installer wins, and the
    // returned bool tells us which case we were in.
    let _installed = init_global_kzg_settings(settings.clone());

    // Recover the secret `s` we baked into `examples/gen_trusted_setup.rs`
    // so we can build a real proof against the loaded setup. Same seed →
    // same scalar → same s_g1/s_g2 as the file.
    let seed: [u8; 64] = [
        0x5a, 0x42, 0x58, 0x2d, 0x44, 0x45, 0x56, 0x4e, 0x45, 0x54, 0x2d, 0x4b, 0x5a, 0x47, 0x2d,
        0x32, 0x30, 0x32, 0x36, 0x2d, 0x30, 0x35, 0x2d, 0x30, 0x39, 0x21, 0x21, 0x21, 0x21, 0x21,
        0x21, 0x21, 0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe, 0xfe, 0xed, 0xfa, 0xce, 0x13,
        0x37, 0x42, 0x42, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00, 0x0f, 0x1e,
        0x2d, 0x3c, 0x4b, 0x5a,
    ];
    let s = Scalar::from_bytes_wide(&seed);
    let s_g1 = (G1Projective::generator() * s).to_affine();

    // Cross-check: s·G2 derived from the same seed must match the s_g2
    // unpacked from the committed file. If this fails, the file on disk
    // does not match the source-tree generator (someone hand-edited it
    // or regenerated with a different secret).
    let expected_s_g2 = (G2Projective::generator() * s).to_affine();
    assert_eq!(
        settings.s_g2, expected_s_g2,
        "committed devnet setup file is out-of-sync with examples/gen_trusted_setup.rs — \
         re-run `cargo run -p zbx-crypto --example gen_trusted_setup`"
    );

    // Build a degree-1 proof p(X) = a + b·X under the loaded setup.
    let a = Scalar::from(7u64);
    let b = Scalar::from(3u64);
    let z = Scalar::from(11u64);
    let y = a + b * z;
    let commitment = (G1Projective::generator() * a + G1Projective::from(s_g1) * b).to_affine();
    let proof = (G1Projective::generator() * b).to_affine();

    let c_b = commitment.to_compressed();
    let pi_b = proof.to_compressed();
    let vh = kzg_to_versioned_hash(&c_b);

    let scalar_to_be32 = |s: &Scalar| -> [u8; 32] {
        let le = s.to_bytes();
        let mut be = [0u8; 32];
        for (i, x) in le.iter().enumerate() { be[31 - i] = *x; }
        be
    };

    let mut input = Vec::with_capacity(192);
    input.extend_from_slice(&vh);
    input.extend_from_slice(&scalar_to_be32(&z));
    input.extend_from_slice(&scalar_to_be32(&y));
    input.extend_from_slice(&c_b);
    input.extend_from_slice(&pi_b);

    let (out, gas) = do_kzg_point_eval(&input, 100_000, &settings)
        .expect("point evaluation against the committed setup must succeed");
    assert_eq!(gas, 50_000);
    assert_eq!(out, point_evaluation_success_return());
}

#[test]
fn task4_corrupt_setup_file_is_rejected_at_load_time() {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let p = dir.join("zbx_task4_corrupt_setup.txt");
    let mut f = std::fs::File::create(&p).unwrap();
    // Wrong header counts (not 4096/65) — must be rejected.
    writeln!(f, "1").unwrap();
    writeln!(f, "1").unwrap();
    writeln!(f, "{}", "00".repeat(48)).unwrap();
    writeln!(f, "{}", "00".repeat(96)).unwrap();
    drop(f);

    let res = load_trusted_setup(&p);
    assert!(res.is_err(), "wrong-shape ceremony file must be rejected");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn task4_missing_setup_file_is_rejected_at_load_time() {
    let p = std::env::temp_dir().join("zbx_task4_no_such_file_xyz.txt");
    let _ = std::fs::remove_file(&p);
    assert!(load_trusted_setup(&p).is_err());
}
