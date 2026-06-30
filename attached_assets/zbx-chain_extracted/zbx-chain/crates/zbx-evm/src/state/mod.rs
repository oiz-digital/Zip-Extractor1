//! Per-transaction state journal types used by the EVM interpreter.
//!
//! Currently re-exports the access-list / snapshot scaffolding from
//! `snapshot.rs`. The interpreter's runtime journal is driven through the
//! `Host` trait (see `crate::host`); types in this module are kept around
//! for the higher-level `zbx-execution` block executor that composes
//! frame-level snapshots into transaction-level journals.

pub mod snapshot;

pub use snapshot::{
    AccessListState, AccountRevert, DirtyAccount, StateSnapshot, TxLog, ZvmState,
    SHANGHAI_BLOCK,
};
