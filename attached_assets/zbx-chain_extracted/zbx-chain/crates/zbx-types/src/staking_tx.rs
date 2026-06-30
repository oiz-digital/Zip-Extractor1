//! On-chain staking transaction format.
//!
//! `StakingTx` is carried in the `data` field of a normal `SignedTransaction`
//! whose `to` is `STAKING_PRECOMPILE_ADDR`. The block executor recognises this
//! destination and dispatches to `zbx-staking::tx_handler` instead of running
//! EVM bytecode.
//!
//! Wire format: canonical RLP. Each variant is encoded as a list whose first
//! element is a single-byte tag and whose remaining elements are the variant
//! fields in declaration order. Fixed-size byte arrays (pubkey/sig) are
//! encoded as RLP byte strings of fixed length so the decoder can reject
//! malformed payloads at admission.

use crate::address::Address;
use crate::H256;
use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use thiserror::Error;

/// Magic destination address for staking calls: `0x...0888`.
pub const STAKING_PRECOMPILE_ADDR: Address = Address([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0x08, 0x88,
]);

#[inline]
pub fn is_staking_call(addr: &Address) -> bool {
    addr == &STAKING_PRECOMPILE_ADDR
}

/// Unbonding period in blocks (~21 days at 2 s/block).
pub const UNBONDING_PERIOD_BLOCKS: u64 = 907_200;

const TAG_REGISTER: u8 = 0;
const TAG_DELEGATE: u8 = 1;
const TAG_UNDELEGATE: u8 = 2;
const TAG_WITHDRAW: u8 = 3;
const TAG_CLAIM_REWARDS: u8 = 4;
const TAG_CLAIM_DELEGATOR_REWARDS: u8 = 5;
const TAG_FILE_APPEAL: u8 = 6;
const TAG_PROPOSE_UPGRADE: u8 = 7;
const TAG_CAST_VOTE: u8 = 8;

/// Appeal bond escrowed on `FileAppeal`: 1000 ZBX in wei.
/// Refunded on successful overturn; forfeited (burnt) if the appeal
/// is rejected. Larger than the whistleblower bond to discourage
/// frivolous appeals from validators trying to delay finalization.
pub const APPEAL_BOND_WEI: u128 = 1_000 * 1_000_000_000_000_000_000u128;

/// On-chain staking call payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StakingTx {
    /// Register the sender as a new validator. The carrying tx's `value`
    /// MUST equal `self_stake`. Registration goes through
    /// `ValidatorSet::register_with_pop`.
    RegisterValidator {
        /// Compressed secp256k1 public key of the validator (33 bytes).
        #[serde(with = "BigArray")]
        pubkey: [u8; 33],
        #[serde(with = "BigArray")]
        bls_pubkey: [u8; 48],
        #[serde(with = "BigArray")]
        bls_pop: [u8; 96],
        self_stake: u128,
        commission_bps: u16,
    },

    /// Delegate `amount` wei to `validator`. The carrying tx's `value`
    /// MUST equal `amount`.
    Delegate {
        validator: Address,
        amount: u128,
    },

    /// Begin un-delegating `amount` wei from `validator`. Funds enter the
    /// unbonding queue keyed by `(unlock_height, delegator, validator)`.
    Undelegate {
        validator: Address,
        amount: u128,
    },

    /// Withdraw matured unbonding entries for the sender from `validator`.
    Withdraw {
        validator: Address,
    },

    /// Claim accumulated rewards for `validator`. Sender must equal
    /// `validator` (validators claim their own rewards).
    ClaimRewards {
        validator: Address,
    },

    /// Claim accumulated delegator rewards from `validator`'s pool.
    ///
    /// The sender is the delegator. The claimed amount is proportional to
    /// the sender's current delegation divided by the validator's total
    /// `delegated_stake`, after the validator's commission has been deducted
    /// at reward distribution time (not at claim time).
    ///
    /// The carrying transaction's `value` MUST be 0.
    ClaimDelegatorRewards {
        validator: Address,
    },

    /// File an on-chain appeal against a pending slash record.
    ///
    /// Sender MUST equal the slash record's offender address. The
    /// carrying transaction's `value` MUST equal `APPEAL_BOND_WEI`
    /// (escrowed at `STAKING_PRECOMPILE_ADDR`); refunded on successful
    /// overturn, forfeited on rejection. Filing is only valid while
    /// the record is in `Pending` status and before its `appeal_deadline`.
    FileAppeal {
        evidence_id: H256,
    },

    /// Submit an on-chain governance upgrade proposal.
    ///
    /// Only registered validators may propose. The proposal is recorded
    /// in `ProposalRegistry` keyed by `proposal_id` (monotonically
    /// assigned by the block executor). Proposals targeting module
    /// `module_name` to be promoted to `new_version` at block
    /// `activation_height` enter `ProposalStatus::Pending` until they
    /// pass quorum or expire.
    ///
    /// The carrying transaction's `value` MUST be 0.
    ProposeUpgrade {
        /// Lowercase crate name (e.g. `"evm"`, `"consensus"`).
        module_name: Vec<u8>,
        /// Target semantic version (encoded as major * 1_000_000 + minor * 1_000 + patch).
        new_version: u32,
        /// Block height at which the upgrade activates if quorum is reached.
        activation_height: u64,
    },

    /// Cast a yes/no vote on an existing governance upgrade proposal.
    ///
    /// Only registered validators may vote. Each validator may vote at
    /// most once per `proposal_id`. The block executor tallies votes and
    /// transitions `ProposalStatus::Pending → Scheduled` when quorum
    /// (2/3 of staked weight) is met, or `→ Expired` when
    /// `activation_height` passes with insufficient votes.
    ///
    /// The carrying transaction's `value` MUST be 0.
    CastVote {
        proposal_id: u64,
        /// `true` = vote yes, `false` = vote no.
        approve: bool,
    },
}

#[derive(Debug, Error)]
pub enum StakingTxCodecError {
    #[error("rlp encode: {0}")]
    Encode(String),
    #[error("rlp decode: {0}")]
    Decode(String),
}

impl Encodable for StakingTx {
    fn rlp_append(&self, s: &mut RlpStream) {
        match self {
            StakingTx::RegisterValidator {
                pubkey, bls_pubkey, bls_pop, self_stake, commission_bps,
            } => {
                s.begin_list(6);
                s.append(&TAG_REGISTER);
                s.append(&pubkey.as_slice());
                s.append(&bls_pubkey.as_slice());
                s.append(&bls_pop.as_slice());
                s.append(&u128_be(*self_stake).as_slice());
                s.append(commission_bps);
            }
            StakingTx::Delegate { validator, amount } => {
                s.begin_list(3);
                s.append(&TAG_DELEGATE);
                s.append(&&validator.0[..]);
                s.append(&u128_be(*amount).as_slice());
            }
            StakingTx::Undelegate { validator, amount } => {
                s.begin_list(3);
                s.append(&TAG_UNDELEGATE);
                s.append(&&validator.0[..]);
                s.append(&u128_be(*amount).as_slice());
            }
            StakingTx::Withdraw { validator } => {
                s.begin_list(2);
                s.append(&TAG_WITHDRAW);
                s.append(&&validator.0[..]);
            }
            StakingTx::ClaimRewards { validator } => {
                s.begin_list(2);
                s.append(&TAG_CLAIM_REWARDS);
                s.append(&&validator.0[..]);
            }
            StakingTx::ClaimDelegatorRewards { validator } => {
                s.begin_list(2);
                s.append(&TAG_CLAIM_DELEGATOR_REWARDS);
                s.append(&&validator.0[..]);
            }
            StakingTx::FileAppeal { evidence_id } => {
                s.begin_list(2);
                s.append(&TAG_FILE_APPEAL);
                s.append(&&evidence_id.0[..]);
            }
            StakingTx::ProposeUpgrade { module_name, new_version, activation_height } => {
                s.begin_list(4);
                s.append(&TAG_PROPOSE_UPGRADE);
                s.append(&module_name.as_slice());
                s.append(new_version);
                s.append(activation_height);
            }
            StakingTx::CastVote { proposal_id, approve } => {
                s.begin_list(3);
                s.append(&TAG_CAST_VOTE);
                s.append(proposal_id);
                s.append(&(*approve as u8));
            }
        }
    }
}

impl Decodable for StakingTx {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let n = rlp.item_count()?;
        if n == 0 {
            return Err(DecoderError::RlpIncorrectListLen);
        }
        let tag: u8 = rlp.val_at(0)?;
        match tag {
            TAG_REGISTER if n == 6 => {
                let pubkey = fixed_bytes::<33>(&rlp.at(1)?)?;
                let bls_pubkey = fixed_bytes::<48>(&rlp.at(2)?)?;
                let bls_pop = fixed_bytes::<96>(&rlp.at(3)?)?;
                let self_stake = u128_from_be(&rlp.at(4)?)?;
                let commission_bps: u16 = rlp.val_at(5)?;
                Ok(StakingTx::RegisterValidator {
                    pubkey, bls_pubkey, bls_pop, self_stake, commission_bps,
                })
            }
            TAG_DELEGATE if n == 3 => Ok(StakingTx::Delegate {
                validator: address_at(&rlp.at(1)?)?,
                amount: u128_from_be(&rlp.at(2)?)?,
            }),
            TAG_UNDELEGATE if n == 3 => Ok(StakingTx::Undelegate {
                validator: address_at(&rlp.at(1)?)?,
                amount: u128_from_be(&rlp.at(2)?)?,
            }),
            TAG_WITHDRAW if n == 2 => Ok(StakingTx::Withdraw {
                validator: address_at(&rlp.at(1)?)?,
            }),
            TAG_CLAIM_REWARDS if n == 2 => Ok(StakingTx::ClaimRewards {
                validator: address_at(&rlp.at(1)?)?,
            }),
            TAG_CLAIM_DELEGATOR_REWARDS if n == 2 => Ok(StakingTx::ClaimDelegatorRewards {
                validator: address_at(&rlp.at(1)?)?,
            }),
            TAG_FILE_APPEAL if n == 2 => Ok(StakingTx::FileAppeal {
                evidence_id: H256(fixed_bytes::<32>(&rlp.at(1)?)?),
            }),
            TAG_PROPOSE_UPGRADE if n == 4 => {
                let module_name: Vec<u8> = rlp.at(1)?.data()?.to_vec();
                let new_version: u32 = rlp.val_at(2)?;
                let activation_height: u64 = rlp.val_at(3)?;
                Ok(StakingTx::ProposeUpgrade { module_name, new_version, activation_height })
            }
            TAG_CAST_VOTE if n == 3 => {
                let proposal_id: u64 = rlp.val_at(1)?;
                let approve_byte: u8 = rlp.val_at(2)?;
                Ok(StakingTx::CastVote { proposal_id, approve: approve_byte != 0 })
            }
            _ => Err(DecoderError::Custom("StakingTx: unknown tag or arity")),
        }
    }
}

fn u128_be(v: u128) -> [u8; 16] { v.to_be_bytes() }

fn u128_from_be(rlp: &Rlp) -> Result<u128, DecoderError> {
    let b = rlp.data()?;
    if b.len() > 16 {
        return Err(DecoderError::RlpInvalidLength);
    }
    let mut buf = [0u8; 16];
    buf[16 - b.len()..].copy_from_slice(b);
    Ok(u128::from_be_bytes(buf))
}

fn fixed_bytes<const N: usize>(rlp: &Rlp) -> Result<[u8; N], DecoderError> {
    let b = rlp.data()?;
    if b.len() != N {
        return Err(DecoderError::RlpInvalidLength);
    }
    let mut out = [0u8; N];
    out.copy_from_slice(b);
    Ok(out)
}

fn address_at(rlp: &Rlp) -> Result<Address, DecoderError> {
    Ok(Address(fixed_bytes::<20>(rlp)?))
}

impl StakingTx {
    pub fn encode(&self) -> Result<Vec<u8>, StakingTxCodecError> {
        Ok(rlp::encode(self).to_vec())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StakingTxCodecError> {
        rlp::decode::<StakingTx>(bytes).map_err(|e| StakingTxCodecError::Decode(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precompile_addr_is_0x0888() {
        assert_eq!(STAKING_PRECOMPILE_ADDR.0[18], 0x08);
        assert_eq!(STAKING_PRECOMPILE_ADDR.0[19], 0x88);
        assert!(is_staking_call(&STAKING_PRECOMPILE_ADDR));
        assert!(!is_staking_call(&Address([1u8; 20])));
    }

    #[test]
    fn rlp_roundtrip_all_variants() {
        let cases = vec![
            StakingTx::RegisterValidator {
                pubkey: [3u8; 33],
                bls_pubkey: [7u8; 48],
                bls_pop: [9u8; 96],
                self_stake: 100_000 * 10u128.pow(18),
                commission_bps: 500,
            },
            StakingTx::Delegate { validator: Address([1u8; 20]), amount: 5 * 10u128.pow(18) },
            StakingTx::Undelegate {
                validator: Address([2u8; 20]),
                amount: 1_000_000_000_000_000_000,
            },
            StakingTx::Withdraw { validator: Address([3u8; 20]) },
            StakingTx::ClaimRewards { validator: Address([4u8; 20]) },
            StakingTx::ClaimDelegatorRewards { validator: Address([5u8; 20]) },
            StakingTx::ProposeUpgrade {
                module_name: b"zbx-vm".to_vec(),
                new_version: 3,
                activation_height: 1_000_000,
            },
            StakingTx::CastVote { proposal_id: 42, approve: true },
            StakingTx::CastVote { proposal_id: 7, approve: false },
        ];
        for c in &cases {
            let enc = c.encode().unwrap();
            let dec = StakingTx::decode(&enc).unwrap();
            assert_eq!(c, &dec);
        }
    }

    #[test]
    fn rlp_roundtrip_propose_upgrade_fields() {
        let tx = StakingTx::ProposeUpgrade {
            module_name: b"zbx-staking".to_vec(),
            new_version: 99,
            activation_height: 7_777_777,
        };
        let enc = tx.encode().unwrap();
        let dec = StakingTx::decode(&enc).unwrap();
        match dec {
            StakingTx::ProposeUpgrade { module_name, new_version, activation_height } => {
                assert_eq!(module_name, b"zbx-staking");
                assert_eq!(new_version, 99);
                assert_eq!(activation_height, 7_777_777);
            }
            _ => panic!("decoded wrong variant"),
        }
    }

    #[test]
    fn rlp_roundtrip_cast_vote_both_sides() {
        for approve in [true, false] {
            let tx = StakingTx::CastVote { proposal_id: 1234, approve };
            let enc = tx.encode().unwrap();
            let dec = StakingTx::decode(&enc).unwrap();
            match dec {
                StakingTx::CastVote { proposal_id, approve: a } => {
                    assert_eq!(proposal_id, 1234);
                    assert_eq!(a, approve);
                }
                _ => panic!("decoded wrong variant"),
            }
        }
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(StakingTx::decode(&[0xff; 3]).is_err());
        assert!(StakingTx::decode(&[]).is_err());
    }

    #[test]
    fn decode_rejects_wrong_pubkey_len() {
        // Hand-built RLP list with 32-byte (not 33) "pubkey" → must reject.
        let mut s = RlpStream::new_list(6);
        s.append(&TAG_REGISTER);
        s.append(&[0u8; 32].as_slice());
        s.append(&[0u8; 48].as_slice());
        s.append(&[0u8; 96].as_slice());
        s.append(&u128_be(1).as_slice());
        s.append(&500u16);
        assert!(StakingTx::decode(&s.out()).is_err());
    }

    #[test]
    fn unbonding_period_is_21d_at_2s_blocks() {
        assert_eq!(UNBONDING_PERIOD_BLOCKS, 21 * 24 * 60 * 60 / 2);
    }
}
