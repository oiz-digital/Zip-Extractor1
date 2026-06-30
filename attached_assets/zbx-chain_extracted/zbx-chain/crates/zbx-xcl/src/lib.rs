//! zbx-xcl — Native Cross-Chain Layer for Zebvix Chain.
//!
//! # What is this?
//!
//! A **trustless, bridge-free** cross-chain interoperability protocol.
//! Unlike a traditional bridge (which requires trusting off-chain relayers or
//! a multisig committee), the XCL uses *cryptographic proofs* to transfer
//! assets and messages between ZBX-compatible chains without any privileged
//! intermediaries.
//!
//! # How it works
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     ZBX Chain (Chain A)                         │
//! │                                                                 │
//! │  ① send_packet(channel, amount, receiver, timeout)             │
//! │     → escrow funds                                              │
//! │     → write keccak256(packet) to state trie                    │
//! │     → emit PacketSent event                                     │
//! │                                                                 │
//! │  ④ ack_packet(packet, ack_proof)          [relayed by anyone]  │
//! │     → verify ack Merkle proof vs. Chain B light client         │
//! │     → on success: commitment cleared                            │
//! │     → on error:   refund sender                                 │
//! └─────────────────────────────────────────────────────────────────┘
//!          │ ② anyone submits commitment proof       ▲
//!          │    (no special permission needed)        │ ③ relayer
//!          ▼                                          │    relays ack
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     Foreign Chain (Chain B)                     │
//! │                                                                 │
//! │  ② recv_packet(packet, commitment_proof)   [relayed by anyone] │
//! │     → verify Chain A header QC (BLS12-381 pairing)             │
//! │     → verify Merkle proof against Chain A state root           │
//! │     → release/credit funds to receiver                         │
//! │     → write receipt + ack to state trie                        │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Key properties
//!
//! - **No bridge operators** — relayers have zero authority; bad proofs are rejected.
//! - **No wrapped tokens** — native ZBX is locked on source and released on destination.
//! - **Total supply conserved** — sum of ZBX across all chains equals 150 million cap.
//! - **Permissionless relay** — anyone (validators, users, bots) can relay packets.
//! - **BLS12-381 light clients** — foreign headers verified via real pairing check.
//! - **MPT state proofs** — packet commitments verified via Merkle Patricia Trie proof.
//! - **IBC-compatible packet format** — interoperable with any chain implementing XCL.
//!
//! # Module layout
//!
//! | Module       | Responsibility                                         |
//! |--------------|--------------------------------------------------------|
//! | `error`      | `XclError` enum                                        |
//! | `packet`     | `XclPacket`, commitment hashing, trie key derivation   |
//! | `channel`    | Channel state machine (Init → TryOpen → Open → Closed) |
//! | `client`     | Foreign chain light client (BLS QC + MPT proof verify) |
//! | `commitment` | On-chain commitment / receipt / ack store              |
//! | `transfer`   | FT-1: fungible token transfer protocol & denom logic   |
//! | `handler`    | `send`, `recv`, `ack`, `timeout` packet handlers       |
//! | `relay`      | Permissionless relayer job queue & state machine       |
//! | `precompile` | EVM precompile 0x0b for contract-initiated sends       |

pub mod channel;
pub mod client;
pub mod commitment;
pub mod error;
pub mod handler;
pub mod message;
pub mod packet;
pub mod precompile;
pub mod relay;
pub mod transfer;

pub use channel::{Channel, ChannelRegistry, ChannelState, Ordering};
pub use client::{ClientRegistry, ForeignClient, ForeignHeader};
pub use commitment::CommitmentStore;
pub use error::XclError;
pub use handler::{HandlerResult, StateChange, XclEvent, XclHandler};
pub use message::{MsgPacketData, PacketApp, detect_app, MSG_APP_ID};
pub use packet::{ChannelId, ClientId, PacketAck, XclPacket};
pub use precompile::{XCL_PRECOMPILE_ADDR, parse_xcl_send, encode_xcl_send_output};
pub use relay::{RelayJob, RelayStage, XclRelayer};
pub use transfer::{Denom, DenomAction, FtPacketData};

// ── High-level runner (ZEP-026 node wiring) ───────────────────────────────────
pub mod service;
pub use service::XclGateway;
