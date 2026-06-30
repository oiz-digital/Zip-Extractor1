//! Generated protobuf types for ZBX Chain.
//!
//! Proto files live in `proto/` at the repository root.
//! Run `cargo build -p zbx-proto` to regenerate `src/generated/`.
//!
//! Usage:
//! ```rust,ignore
//! use zbx_proto::consensus::v1::VoteMessage;
//! use zbx_proto::da::v1::BlobSubmission;
//! ```

pub mod consensus {
    pub mod v1 {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/zbx.consensus.v1.rs"));
    }
}

pub mod da {
    pub mod v1 {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/zbx.da.v1.rs"));
    }
}

pub mod node {
    pub mod v1 {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/zbx.node.v1.rs"));
    }
}

pub mod prover {
    pub mod v1 {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/zbx.prover.v1.rs"));
    }
}
