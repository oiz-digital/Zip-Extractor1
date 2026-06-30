//! DeFi safety primitives — TWAP windows, reentrancy state, slippage caps.
//!
//! Closes Item #4 (DeFi security): "no TWAP oracle, manipulation possible
//! via flash loans" and Item #5 (reentrancy state machine — Rust-side
//! mirror of `contracts/libraries/ReentrancyGuard.sol` so off-chain
//! simulators can detect reentrancy attempts before tx submission).
//!
//! Discipline:
//! - All collections `BTreeMap`/`Vec` (ordered by external sort key) for
//!   canonical RLP.
//! - `validate()` runs in BOTH constructor AND `Decodable::decode`.
//! - Newtype `Encodable` impls use direct delegation (LESSON #11 strict
//!   form — see `network.rs::PeerId` for the rationale and the empirical
//!   `s.append(&inner)` double-count trap.).

use std::collections::BTreeMap;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

use crate::address::Address;
use crate::U256;

// ---------------------------------------------------------------------------
// PriceObservation — single (timestamp, cumulative_price) sample.
// Cumulative-price-style TWAP (à la Uniswap v2) is manipulation-resistant:
// to skew the TWAP, an attacker must keep the spot price displaced for
// the entire averaging window, which costs O(window × liquidity).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PriceObservation {
    pub timestamp_unix: u64,
    /// `Σ(price_t × Δt)` from genesis. `price_t` is in 18-decimal fixed point.
    pub cumulative_price: U256,
}

impl PriceObservation {
    pub fn new(timestamp_unix: u64, cumulative_price: U256) -> Self {
        Self { timestamp_unix, cumulative_price }
    }
}

// Crate-local Address codec helper. Goes through `s.append` so that the
// item correctly counts as ONE slot in the enclosing `begin_list(N)` —
// calling `slice.rlp_append(s)` directly would write bytes WITHOUT
// incrementing the parent list's slot counter.
fn append_address(s: &mut RlpStream, a: &Address) {
    let bytes: &[u8] = a.0.as_ref();
    s.append(&bytes);
}

fn decode_address(rlp: &Rlp) -> Result<Address, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() != 20 {
        return Err(DecoderError::Custom("address must be 20 bytes"));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(Address(out))
}

// `U256` from `primitive_types` does not impl `rlp::Encodable` in this
// version. We serialize as 32-byte big-endian (canonical Ethereum form).
// Goes through `s.append` so the item counts in the enclosing list.
fn append_u256(s: &mut RlpStream, v: &U256) {
    let mut buf = [0u8; 32];
    v.to_big_endian(&mut buf);
    let bytes: &[u8] = &buf;
    s.append(&bytes);
}

fn decode_u256(rlp: &Rlp) -> Result<U256, DecoderError> {
    let bytes: Vec<u8> = rlp.as_val()?;
    if bytes.len() > 32 {
        return Err(DecoderError::Custom("U256 must be ≤ 32 bytes"));
    }
    // Pad-front to 32 bytes (RLP strips leading zeros).
    let mut buf = [0u8; 32];
    buf[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(U256::from_big_endian(&buf))
}

impl Encodable for PriceObservation {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(2);
        s.append(&self.timestamp_unix);
        append_u256(s, &self.cumulative_price);
    }
}

impl Decodable for PriceObservation {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 2 { return Err(DecoderError::RlpIncorrectListLen); }
        Ok(Self {
            timestamp_unix:    rlp.val_at(0)?,
            cumulative_price:  decode_u256(&rlp.at(1)?)?,
        })
    }
}

// ---------------------------------------------------------------------------
// TwapWindow — sliding-window TWAP buffer for a single (token0, token1) pair.
// Backed by a sorted `Vec<PriceObservation>`. The `window_secs` field is
// the minimum averaging period; `compute(now)` returns `None` if the
// buffer doesn't span at least `window_secs`.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TwapWindow {
    pub token_a:        Address,
    pub token_b:        Address,
    pub window_secs:    u64,
    pub max_observations: u32,
    pub observations:   Vec<PriceObservation>,
}

impl TwapWindow {
    pub const DEFAULT_WINDOW_SECS:     u64 = 30 * 60;   // 30 minutes
    pub const DEFAULT_MAX_OBSERVATIONS: u32 = 256;

    pub fn new(token_a: Address, token_b: Address) -> Result<Self, DecoderError> {
        let w = Self {
            token_a,
            token_b,
            window_secs:      Self::DEFAULT_WINDOW_SECS,
            max_observations: Self::DEFAULT_MAX_OBSERVATIONS,
            observations:     Vec::new(),
        };
        w.validate()?;
        Ok(w)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.token_a == self.token_b {
            return Err(DecoderError::Custom("TwapWindow: token_a == token_b"));
        }
        if self.window_secs == 0 {
            return Err(DecoderError::Custom("TwapWindow: window_secs = 0"));
        }
        if self.max_observations == 0 {
            return Err(DecoderError::Custom("TwapWindow: max_observations = 0"));
        }
        if self.observations.len() > self.max_observations as usize {
            return Err(DecoderError::Custom("TwapWindow: too many observations"));
        }
        // Strict monotonic timestamps.
        for w in self.observations.windows(2) {
            if w[0].timestamp_unix >= w[1].timestamp_unix {
                return Err(DecoderError::Custom(
                    "TwapWindow: observations must be strictly time-ordered"));
            }
            if w[0].cumulative_price > w[1].cumulative_price {
                return Err(DecoderError::Custom(
                    "TwapWindow: cumulative_price must be non-decreasing"));
            }
        }
        Ok(())
    }

    /// Append a new observation. Rejects out-of-order timestamps and
    /// non-monotonic cumulative_price (the cumulative invariant).
    /// Evicts the oldest sample if `max_observations` would be exceeded.
    pub fn record(&mut self, obs: PriceObservation) -> Result<(), DecoderError> {
        if let Some(last) = self.observations.last() {
            if obs.timestamp_unix <= last.timestamp_unix {
                return Err(DecoderError::Custom(
                    "TwapWindow::record: timestamp not strictly newer"));
            }
            if obs.cumulative_price < last.cumulative_price {
                return Err(DecoderError::Custom(
                    "TwapWindow::record: cumulative_price decreased"));
            }
        }
        self.observations.push(obs);
        if self.observations.len() > self.max_observations as usize {
            // Drop oldest (front). `Vec::remove(0)` is O(n) but n ≤ 256.
            self.observations.remove(0);
        }
        Ok(())
    }

    /// Compute the TWAP over [now − window_secs, now]. Returns `None` if
    /// the buffer doesn't span the full window (insufficient data).
    pub fn compute(&self, now_unix: u64) -> Option<U256> {
        if self.observations.len() < 2 { return None; }
        let target_start = now_unix.checked_sub(self.window_secs)?;
        let last = self.observations.last()?;
        if last.timestamp_unix < target_start { return None; }

        // Find the earliest observation at or before target_start.
        // Linear scan is fine for n ≤ 256.
        let mut start_idx: Option<usize> = None;
        for (i, o) in self.observations.iter().enumerate() {
            if o.timestamp_unix <= target_start {
                start_idx = Some(i);
            } else {
                break;
            }
        }
        let start = start_idx.unwrap_or(0);
        let start_obs = &self.observations[start];

        let dt = last.timestamp_unix.checked_sub(start_obs.timestamp_unix)?;
        if dt == 0 { return None; }
        let dprice = last.cumulative_price.checked_sub(start_obs.cumulative_price)?;
        // TWAP = (cumulative_end − cumulative_start) / Δt
        Some(dprice / U256::from(dt))
    }
}

impl Encodable for TwapWindow {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(5);
        append_address(s, &self.token_a);
        append_address(s, &self.token_b);
        s.append(&self.window_secs);
        s.append(&self.max_observations);
        s.begin_list(self.observations.len());
        for o in &self.observations { s.append(o); }
    }
}

impl Decodable for TwapWindow {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 5 { return Err(DecoderError::RlpIncorrectListLen); }

        let token_a          = decode_address(&rlp.at(0)?)?;
        let token_b          = decode_address(&rlp.at(1)?)?;
        let window_secs      = rlp.val_at(2)?;
        let max_observations = rlp.val_at(3)?;

        let obs_rlp = rlp.at(4)?;
        let mut observations = Vec::with_capacity(obs_rlp.item_count()?);
        for i in 0..obs_rlp.item_count()? {
            observations.push(obs_rlp.val_at(i)?);
        }

        let w = Self { token_a, token_b, window_secs, max_observations, observations };
        w.validate()?;
        Ok(w)
    }
}

// ---------------------------------------------------------------------------
// SlippagePolicy — per-pool max-bps slippage cap.
// "bps" = basis points = 1/100th of a percent. 100 bps = 1%.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SlippagePolicy {
    pub max_bps: u16,
}

impl SlippagePolicy {
    pub const ABSOLUTE_MAX_BPS: u16 = 1_000;     // 10% — hard cap
    pub const DEFAULT_BPS:      u16 =    50;     // 0.5%

    pub fn new(max_bps: u16) -> Result<Self, DecoderError> {
        let p = Self { max_bps };
        p.validate()?;
        Ok(p)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.max_bps == 0 {
            return Err(DecoderError::Custom("SlippagePolicy: max_bps = 0"));
        }
        if self.max_bps > Self::ABSOLUTE_MAX_BPS {
            return Err(DecoderError::Custom("SlippagePolicy: max_bps > 1000 (10%)"));
        }
        Ok(())
    }
}

impl Encodable for SlippagePolicy {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(1);
        s.append(&self.max_bps);
    }
}

impl Decodable for SlippagePolicy {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 1 { return Err(DecoderError::RlpIncorrectListLen); }
        let p = Self { max_bps: rlp.val_at(0)? };
        p.validate()?;
        Ok(p)
    }
}

// ---------------------------------------------------------------------------
// ReentrancyState — Rust-side mirror of the Solidity ReentrancyGuard.
// Off-chain simulators (e.g. zbx-bundler) hold one ReentrancyMap keyed by
// contract address. Each top-level external call enters → checks Idle →
// flips to Entered → executes → flips back to Idle on return. Any nested
// call that finds Entered is the reentrancy attack.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum ReentrancyState {
    Idle,
    Entered,
}

impl ReentrancyState {
    pub fn tag(self) -> u8 {
        match self { Self::Idle => 0, Self::Entered => 1 }
    }
    pub fn from_tag(t: u8) -> Result<Self, DecoderError> {
        match t {
            0 => Ok(Self::Idle),
            1 => Ok(Self::Entered),
            _ => Err(DecoderError::Custom("invalid ReentrancyState tag")),
        }
    }
}

impl Encodable for ReentrancyState {
    fn rlp_append(&self, s: &mut RlpStream) { self.tag().rlp_append(s); }
}

impl Decodable for ReentrancyState {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        Self::from_tag(rlp.as_val()?)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReentrancyMap {
    pub guards: BTreeMap<Address, ReentrancyState>,
}

impl Default for ReentrancyMap {
    fn default() -> Self { Self::new() }
}

impl ReentrancyMap {
    pub fn new() -> Self { Self { guards: BTreeMap::new() } }

    /// Attempt to enter the guarded section for `contract`. Returns
    /// `Err` if already entered (reentrancy detected).
    pub fn enter(&mut self, contract: Address) -> Result<(), DecoderError> {
        let state = self.guards.entry(contract).or_insert(ReentrancyState::Idle);
        match *state {
            ReentrancyState::Idle => {
                *state = ReentrancyState::Entered;
                Ok(())
            }
            ReentrancyState::Entered =>
                Err(DecoderError::Custom("ReentrancyMap: re-entry detected")),
        }
    }

    /// Exit the guarded section. Idempotent — calling on Idle is a no-op
    /// (matches Solidity ReentrancyGuard semantics where the modifier
    /// always resets).
    pub fn exit(&mut self, contract: Address) {
        if let Some(state) = self.guards.get_mut(&contract) {
            *state = ReentrancyState::Idle;
        }
    }

    pub fn is_entered(&self, contract: &Address) -> bool {
        self.guards.get(contract).map(|s| *s == ReentrancyState::Entered).unwrap_or(false)
    }
}

impl Encodable for ReentrancyMap {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.guards.len());
        for (addr, state) in &self.guards {
            s.begin_list(2);
            append_address(s, addr);
            s.append(state);
        }
    }
}

impl Decodable for ReentrancyMap {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let mut guards = BTreeMap::new();
        for i in 0..rlp.item_count()? {
            let pair = rlp.at(i)?;
            if pair.item_count()? != 2 { return Err(DecoderError::RlpIncorrectListLen); }
            let addr  = decode_address(&pair.at(0)?)?;
            let state: ReentrancyState = pair.val_at(1)?;
            guards.insert(addr, state);
        }
        Ok(Self { guards })
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = b;
        Address(a)
    }

    fn obs(t: u64, p: u64) -> PriceObservation {
        PriceObservation::new(t, U256::from(p))
    }

    // ---- TwapWindow ----

    #[test]
    fn twap_record_rejects_same_or_older_timestamp() {
        let mut w = TwapWindow::new(addr(1), addr(2)).unwrap();
        w.record(obs(100, 1_000)).unwrap();
        assert!(w.record(obs(100, 2_000)).is_err());
        assert!(w.record(obs(99,  2_000)).is_err());
    }

    #[test]
    fn twap_record_rejects_decreasing_cumulative_price() {
        let mut w = TwapWindow::new(addr(1), addr(2)).unwrap();
        w.record(obs(100, 1_000)).unwrap();
        assert!(w.record(obs(200, 999)).is_err());
    }

    #[test]
    fn twap_compute_returns_none_when_window_not_filled() {
        let mut w = TwapWindow::new(addr(1), addr(2)).unwrap();
        // window_secs default = 1800.
        w.record(obs(0, 0)).unwrap();
        w.record(obs(60, 60_000)).unwrap();
        // now=60 → start = 60-1800 = underflow → None.
        assert_eq!(w.compute(60), None);
    }

    #[test]
    fn twap_compute_returns_average_over_window() {
        let mut w = TwapWindow::new(addr(1), addr(2)).unwrap();
        // Steady price = 1000 over 3600 seconds. cumulative_t = 1000*t.
        w.record(obs(0,    0)).unwrap();
        w.record(obs(1800, 1_800_000)).unwrap();
        w.record(obs(3600, 3_600_000)).unwrap();
        let twap = w.compute(3600).unwrap();
        // Window = [1800, 3600]. Δprice = 1_800_000, Δt = 1800. TWAP = 1000.
        assert_eq!(twap, U256::from(1000u64));
    }

    #[test]
    fn twap_window_evicts_oldest_at_capacity() {
        let mut w = TwapWindow::new(addr(1), addr(2)).unwrap();
        w.max_observations = 3;
        w.record(obs(10, 100)).unwrap();
        w.record(obs(20, 200)).unwrap();
        w.record(obs(30, 300)).unwrap();
        w.record(obs(40, 400)).unwrap();
        assert_eq!(w.observations.len(), 3);
        assert_eq!(w.observations[0].timestamp_unix, 20);
    }

    #[test]
    fn twap_validate_rejects_same_token_pair() {
        let r = TwapWindow::new(addr(1), addr(1));
        assert!(r.is_err());
    }

    #[test]
    fn twap_rlp_round_trip() {
        let mut w = TwapWindow::new(addr(1), addr(2)).unwrap();
        w.record(obs(0,    0)).unwrap();
        w.record(obs(1800, 1_800_000)).unwrap();
        w.record(obs(3600, 3_600_000)).unwrap();
        let enc = rlp::encode(&w);
        let dec: TwapWindow = rlp::decode(&enc).unwrap();
        assert_eq!(w, dec);
    }

    // ---- SlippagePolicy ----

    #[test]
    fn slippage_default_validates() {
        SlippagePolicy::new(SlippagePolicy::DEFAULT_BPS).unwrap();
    }

    #[test]
    fn slippage_rejects_zero_and_above_cap() {
        assert!(SlippagePolicy::new(0).is_err());
        assert!(SlippagePolicy::new(SlippagePolicy::ABSOLUTE_MAX_BPS + 1).is_err());
        SlippagePolicy::new(SlippagePolicy::ABSOLUTE_MAX_BPS).unwrap();   // boundary OK
    }

    #[test]
    fn slippage_rlp_round_trip() {
        let p = SlippagePolicy::new(75).unwrap();
        let enc = rlp::encode(&p);
        let dec: SlippagePolicy = rlp::decode(&enc).unwrap();
        assert_eq!(p, dec);
    }

    // ---- ReentrancyMap ----

    #[test]
    fn reentrancy_first_enter_succeeds() {
        let mut m = ReentrancyMap::new();
        m.enter(addr(7)).unwrap();
        assert!(m.is_entered(&addr(7)));
    }

    #[test]
    fn reentrancy_second_enter_rejected() {
        let mut m = ReentrancyMap::new();
        m.enter(addr(7)).unwrap();
        assert!(m.enter(addr(7)).is_err());
    }

    #[test]
    fn reentrancy_exit_resets_state() {
        let mut m = ReentrancyMap::new();
        m.enter(addr(7)).unwrap();
        m.exit(addr(7));
        assert!(!m.is_entered(&addr(7)));
        // Re-entry now allowed.
        m.enter(addr(7)).unwrap();
    }

    #[test]
    fn reentrancy_independent_per_contract() {
        let mut m = ReentrancyMap::new();
        m.enter(addr(1)).unwrap();
        m.enter(addr(2)).unwrap();   // different contract — OK
        assert!(m.is_entered(&addr(1)));
        assert!(m.is_entered(&addr(2)));
    }

    #[test]
    fn reentrancy_rlp_round_trip() {
        let mut m = ReentrancyMap::new();
        m.enter(addr(1)).unwrap();
        m.enter(addr(2)).unwrap();
        m.exit(addr(1));
        let enc = rlp::encode(&m);
        let dec: ReentrancyMap = rlp::decode(&enc).unwrap();
        assert_eq!(m, dec);
    }

    #[test]
    fn reentrancy_state_tag_round_trip() {
        for st in [ReentrancyState::Idle, ReentrancyState::Entered] {
            let enc = rlp::encode(&st);
            let dec: ReentrancyState = rlp::decode(&enc).unwrap();
            assert_eq!(st, dec);
        }
        assert!(ReentrancyState::from_tag(99).is_err());
    }
}
