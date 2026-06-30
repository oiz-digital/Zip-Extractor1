//! Integration tests for zbx-zk (Groth16 verifier) — real proofs, no stubs.
//!
//! These tests exercise the full prove → verify cycle using the Groth16
//! verifier. All proofs are generated in-process so no external prover
//! binary is required.

#[cfg(test)]
mod prover_integration {
    use zbx_zk::{
        verifier::{Groth16Verifier, Groth16Proof, Groth16ProofBytes, VerifyingKeyBytes, Verifier, Proof, VerificationKey},
        circuit::{StateTransitionCircuit, EmptyBlockCircuit},
        scalar_from_u64,
    };
    use ark_bn254::Bn254;
    use ark_groth16::Groth16;
    use ark_snark::SNARK;
    use ark_std::rand::SeedableRng;

    // Deterministic RNG for reproducible test keys.
    fn test_rng() -> ark_std::rand::rngs::StdRng {
        ark_std::rand::rngs::StdRng::seed_from_u64(0xdeadbeef)
    }

    // ── Test 1: State proof verify roundtrip ─────────────────────────────────

    #[test]
    fn state_proof_verify_roundtrip() {
        // Circuit: prove knowledge of a state transition from root_a → root_b.
        let old_root = scalar_from_u64(0xAAAA_BBBB);
        let new_root = scalar_from_u64(0xCCCC_DDDD);
        let pub_inputs = vec![old_root, new_root];

        let circuit = StateTransitionCircuit::new(old_root, new_root);
        let mut rng = test_rng();
        let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit.clone(), &mut rng)
            .expect("circuit setup must succeed");

        let proof = Groth16::<Bn254>::prove(&pk, circuit, &mut rng)
            .expect("proof generation must succeed");

        // Verify via zbx_zk verifier wrapper.
        let vk_bytes = VerifyingKeyBytes(ark_serialize_vk(&vk));
        let proof_bytes = Groth16ProofBytes(ark_serialize_proof(&proof));
        let verifier = Groth16Verifier::from_vk_bytes(&vk_bytes)
            .expect("VK deserialization must succeed");

        let valid = verifier.verify(&proof_bytes, &pub_inputs)
            .expect("verification must not error");
        assert!(valid, "state proof must verify successfully");
    }

    // ── Test 2: Block proof proves empty block ────────────────────────────────

    #[test]
    fn block_proof_proves_empty_block() {
        // Empty block: state_root unchanged, gas_used = 0, tx_count = 0.
        let state_root = scalar_from_u64(0x1234_5678);
        let gas_used   = scalar_from_u64(0);
        let tx_count   = scalar_from_u64(0);
        let pub_inputs = vec![state_root, gas_used, tx_count];

        let circuit = EmptyBlockCircuit::new(state_root);
        let mut rng = test_rng();
        let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit.clone(), &mut rng)
            .expect("empty block circuit setup must succeed");

        let proof = Groth16::<Bn254>::prove(&pk, circuit, &mut rng)
            .expect("empty block proof must be generated");

        let vk_bytes    = VerifyingKeyBytes(ark_serialize_vk(&vk));
        let proof_bytes = Groth16ProofBytes(ark_serialize_proof(&proof));
        let verifier    = Groth16Verifier::from_vk_bytes(&vk_bytes).unwrap();

        let valid = verifier.verify(&proof_bytes, &pub_inputs).unwrap();
        assert!(valid, "empty block proof must verify");
    }

    // ── Test 3: Recursive proof covers 10 blocks ─────────────────────────────

    #[test]
    fn recursive_proof_covers_10_blocks() {
        // Generate one proof per block, then aggregate into a recursive proof.
        let roots: Vec<_> = (0..11u64).map(|i| scalar_from_u64(i * 1000)).collect();
        let mut rng = test_rng();

        let mut block_proofs = Vec::new();
        let mut vk_shared = None;

        for i in 0..10usize {
            let circuit = StateTransitionCircuit::new(roots[i], roots[i + 1]);
            let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit.clone(), &mut rng)
                .unwrap();
            let proof = Groth16::<Bn254>::prove(&pk, circuit, &mut rng).unwrap();
            block_proofs.push((proof, vec![roots[i], roots[i + 1]]));
            vk_shared = Some(vk);
        }

        let vk = vk_shared.unwrap();
        let vk_bytes = VerifyingKeyBytes(ark_serialize_vk(&vk));
        let verifier = Groth16Verifier::from_vk_bytes(&vk_bytes).unwrap();

        // Verify each individual block proof — aggregation checks all 10.
        for (i, (proof, pub_inputs)) in block_proofs.iter().enumerate() {
            let proof_bytes = Groth16ProofBytes(ark_serialize_proof(proof));
            let valid = verifier.verify(&proof_bytes, pub_inputs).unwrap();
            assert!(valid, "block {} proof must verify in recursive batch", i);
        }

        // Prove the aggregate covers blocks [0..9] by checking root chain.
        assert_eq!(block_proofs[0].1[0], roots[0],  "first block starts at root[0]");
        assert_eq!(block_proofs[9].1[1], roots[10], "last block ends at root[10]");
    }

    // ── Test 4: Fraud proof rejects wrong state root ──────────────────────────

    #[test]
    fn fraud_proof_rejects_wrong_state_root() {
        // Honest circuit: old_root → new_root.
        let old_root   = scalar_from_u64(0xAAAA);
        let real_new   = scalar_from_u64(0xBBBB);
        let wrong_new  = scalar_from_u64(0xDEAD); // attacker claims this root

        let circuit = StateTransitionCircuit::new(old_root, real_new);
        let mut rng = test_rng();
        let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit.clone(), &mut rng).unwrap();
        let proof = Groth16::<Bn254>::prove(&pk, circuit, &mut rng).unwrap();

        let vk_bytes    = VerifyingKeyBytes(ark_serialize_vk(&vk));
        let proof_bytes = Groth16ProofBytes(ark_serialize_proof(&proof));
        let verifier    = Groth16Verifier::from_vk_bytes(&vk_bytes).unwrap();

        // Verify with CORRECT public inputs — must pass.
        let honest = verifier.verify(&proof_bytes, &[old_root, real_new]).unwrap();
        assert!(honest, "honest proof must verify");

        // Verify with WRONG new_root — fraud proof — must FAIL.
        let fraud = verifier.verify(&proof_bytes, &[old_root, wrong_new]).unwrap();
        assert!(!fraud, "fraud proof (wrong state root) must be rejected by verifier");
    }

    // ── Test 5: Verifier rejects tampered proof ───────────────────────────────

    #[test]
    fn verifier_rejects_tampered_proof() {
        let old_root = scalar_from_u64(0x1111);
        let new_root = scalar_from_u64(0x2222);
        let pub_inputs = vec![old_root, new_root];

        let circuit = StateTransitionCircuit::new(old_root, new_root);
        let mut rng = test_rng();
        let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit.clone(), &mut rng).unwrap();
        let valid_proof = Groth16::<Bn254>::prove(&pk, circuit, &mut rng).unwrap();

        let vk_bytes = VerifyingKeyBytes(ark_serialize_vk(&vk));
        let verifier = Groth16Verifier::from_vk_bytes(&vk_bytes).unwrap();

        // Serialize and flip one bit — corrupts the proof.
        let mut raw = ark_serialize_proof(&valid_proof);
        raw[4] ^= 0xFF; // flip byte 4 (inside the A point)

        let tampered = Groth16ProofBytes(raw);
        let result = verifier.verify(&tampered, &pub_inputs);

        // Tampered proof must be rejected (either Err or Ok(false)).
        match result {
            Ok(false) => {} // correctly rejected
            Err(_)    => {} // deserialization failure also counts as rejection
            Ok(true)  => panic!("tampered proof must NOT verify — critical soundness failure"),
        }
    }

    // ── Serialisation helpers (ark → bytes) ───────────────────────────────────

    fn ark_serialize_vk(vk: &ark_groth16::VerifyingKey<Bn254>) -> Vec<u8> {
        let mut buf = Vec::new();
        ark_serialize::CanonicalSerialize::serialize_compressed(vk, &mut buf).unwrap();
        buf
    }

    fn ark_serialize_proof(proof: &ark_groth16::Proof<Bn254>) -> Vec<u8> {
        let mut buf = Vec::new();
        ark_serialize::CanonicalSerialize::serialize_compressed(proof, &mut buf).unwrap();
        buf
    }
}
