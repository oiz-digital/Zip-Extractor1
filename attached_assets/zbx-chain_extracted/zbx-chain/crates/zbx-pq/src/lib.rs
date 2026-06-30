//! zbx-pq — Post-Quantum Cryptography for ZBX Chain (ZEP-015).
//!
//! Implements NIST FIPS 204 (CRYSTALS-Dilithium-3) for digital signatures
//! and NIST FIPS 203 (CRYSTALS-Kyber-768) for key encapsulation.
//!
//! ## Features
//!
//! - **Dilithium-3**: Post-quantum digital signatures (3309-byte sigs, NIST FIPS 204 ML-DSA-65, Level 3)
//! - **Kyber-768**: Post-quantum KEM for session key encryption (Level 3)
//! - **Hybrid mode**: ECDSA + Dilithium dual signatures for transition period
//!
//! ## Migration Phases (ZEP-015)
//!
//! | Block    | Phase                  | Required              |
//! |----------|------------------------|-----------------------|
//! | 0        | Classical              | ECDSA only            |
//! | 500,000  | HybridEcdsaPrimary     | ECDSA + optional PQ   |
//! | 750,000  | HybridPqPrimary        | Either ECDSA or PQ    |
//! | TBD      | PostQuantumOnly        | Dilithium required    |
//!
//! ## Usage
//!
//! ```rust,no_run
//! use zbx_pq::{dilithium, kyber, hybrid};
//!
//! // Generate a Dilithium-3 keypair from a seed
//! let kp = dilithium::keygen_from_seed(&[42u8; 32]);
//!
//! // Sign a message
//! let sig = dilithium::sign(&kp.private_key, b"my transaction");
//!
//! // Verify the signature
//! dilithium::verify(&kp.public_key, b"my transaction", &sig).unwrap();
//!
//! // Kyber KEM: encapsulate a shared secret
//! let kyber_kp = kyber::kyber_keygen(&[1u8; 32]);
//! let (ciphertext, shared_secret) = kyber::encapsulate(&kyber_kp.public_key, &mut rand::rngs::OsRng).unwrap();
//! let recovered = kyber::decapsulate(&kyber_kp.private_key, &ciphertext).unwrap();
//! assert_eq!(shared_secret.0, recovered.0);
//! ```

pub mod dilithium;
pub mod error;
pub mod hybrid;
pub mod kyber;

pub use dilithium::{
    DilithiumKeyPair, DilithiumPrivateKey, DilithiumPublicKey, DilithiumSignature,
    DILITHIUM3_PK_SIZE, DILITHIUM3_SIG_SIZE, DILITHIUM3_SK_SIZE,
    keygen_from_seed, proof_of_possession, sign as dilithium_sign, verify as dilithium_verify,
};
pub use error::PqError;
pub use hybrid::{
    HybridSignature, HybridVerifyResult, PqPhase,
    dilithium_address, verify_hybrid,
};
pub use kyber::{
    KyberCiphertext, KyberKeyPair, KyberPrivateKey, KyberPublicKey, SharedSecret,
    KYBER768_CT_SIZE, KYBER768_PK_SIZE, KYBER768_SK_SIZE, KYBER_SS_SIZE,
    decapsulate, encapsulate, kyber_keygen,
};
