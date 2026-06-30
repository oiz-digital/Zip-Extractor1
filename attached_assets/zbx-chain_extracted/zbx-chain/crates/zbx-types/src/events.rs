//! Centralized chain-event registry — closes Item #10 (no canonical
//! catalogue of every event a Zebvix node can emit).
//!
//! Today, events are scattered: zbx-evm emits Solidity logs (typed by
//! 32-byte topic0), zbx-consensus emits state-transition events
//! ad-hoc, zbx-bundler emits AA-flow events, zbx-staking emits
//! reward/slash events. Without a single registry it is impossible
//! to:
//! - audit "what can this chain emit" in one place,
//! - guarantee event-schema compatibility across upgrades, or
//! - generate explorer/indexer types from a single source of truth.
//!
//! This module provides:
//! - `EventDomain` (consensus | execution | governance | defi | …)
//! - `EventSeverity` (info | warn | critical)
//! - `EventDescriptor` (id, name, domain, severity, schema_version)
//! - `EventRegistry` (BTreeMap<EventId, EventDescriptor> with lookup
//!   helpers, RLP codec, and an `installed_default()` constructor that
//!   returns the canonical list of every event the chain currently emits)
//!
//! Discipline:
//! - All collections `BTreeMap` for canonical RLP.
//! - `validate()` runs in BOTH constructor AND `Decodable::decode`.
//! - Newtype `Encodable` impls use direct delegation per LESSON #11
//!   strict form (see `network.rs::PeerId` rationale).

use std::collections::BTreeMap;

use rlp::{Decodable, DecoderError, Encodable, Rlp, RlpStream};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// EventId — stable u32 identifier. Tag layout:
//   high byte  = EventDomain tag
//   low 24 bit = sequence within domain
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct EventId(pub u32);

impl EventId {
    pub const fn new(domain_tag: u8, seq: u32) -> Self {
        // Truncate seq to 24 bits.
        Self(((domain_tag as u32) << 24) | (seq & 0x00FF_FFFF))
    }
    pub fn domain_tag(self) -> u8 { (self.0 >> 24) as u8 }
    pub fn sequence(self)   -> u32 { self.0 & 0x00FF_FFFF }
}

impl Encodable for EventId {
    fn rlp_append(&self, s: &mut RlpStream) { self.0.rlp_append(s); }
}
impl Decodable for EventId {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> { Ok(Self(rlp.as_val()?)) }
}

// ---------------------------------------------------------------------------
// EventDomain — broad category. Tag bytes are stable on disk.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum EventDomain {
    Consensus  = 1,
    Execution  = 2,
    Governance = 3,
    Defi       = 4,
    Staking    = 5,
    Bundler    = 6,
    Bridge     = 7,
    Oracle     = 8,
    Slashing   = 9,
    Network    = 10,
}

impl EventDomain {
    pub fn tag(self) -> u8 { self as u8 }

    pub fn from_tag(t: u8) -> Result<Self, DecoderError> {
        match t {
            1  => Ok(Self::Consensus),
            2  => Ok(Self::Execution),
            3  => Ok(Self::Governance),
            4  => Ok(Self::Defi),
            5  => Ok(Self::Staking),
            6  => Ok(Self::Bundler),
            7  => Ok(Self::Bridge),
            8  => Ok(Self::Oracle),
            9  => Ok(Self::Slashing),
            10 => Ok(Self::Network),
            _  => Err(DecoderError::Custom("invalid EventDomain tag")),
        }
    }
}

impl Encodable for EventDomain {
    fn rlp_append(&self, s: &mut RlpStream) { self.tag().rlp_append(s); }
}
impl Decodable for EventDomain {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> { Self::from_tag(rlp.as_val()?) }
}

// ---------------------------------------------------------------------------
// EventSeverity — operational urgency. Indexers can filter on this.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum EventSeverity {
    Info     = 0,
    Warn     = 1,
    Critical = 2,
}

impl EventSeverity {
    pub fn tag(self) -> u8 { self as u8 }
    pub fn from_tag(t: u8) -> Result<Self, DecoderError> {
        match t {
            0 => Ok(Self::Info),
            1 => Ok(Self::Warn),
            2 => Ok(Self::Critical),
            _ => Err(DecoderError::Custom("invalid EventSeverity tag")),
        }
    }
}

impl Encodable for EventSeverity {
    fn rlp_append(&self, s: &mut RlpStream) { self.tag().rlp_append(s); }
}
impl Decodable for EventSeverity {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> { Self::from_tag(rlp.as_val()?) }
}

// ---------------------------------------------------------------------------
// EventDescriptor — schema metadata for one event.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventDescriptor {
    pub id:             EventId,
    pub name:           String,         // snake_case stable
    pub domain:         EventDomain,
    pub severity:       EventSeverity,
    pub schema_version: u32,
}

impl EventDescriptor {
    pub const MAX_NAME_LEN: usize = 64;

    pub fn new(
        id: EventId,
        name: impl Into<String>,
        domain: EventDomain,
        severity: EventSeverity,
        schema_version: u32,
    ) -> Result<Self, DecoderError> {
        let d = Self { id, name: name.into(), domain, severity, schema_version };
        d.validate()?;
        Ok(d)
    }

    pub fn validate(&self) -> Result<(), DecoderError> {
        if self.name.is_empty() {
            return Err(DecoderError::Custom("EventDescriptor: empty name"));
        }
        if self.name.len() > Self::MAX_NAME_LEN {
            return Err(DecoderError::Custom("EventDescriptor: name > 64 chars"));
        }
        if !self.name.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(DecoderError::Custom(
                "EventDescriptor: name must be ascii [a-z0-9_]"));
        }
        if self.id.domain_tag() != self.domain.tag() {
            return Err(DecoderError::Custom(
                "EventDescriptor: id.domain_tag != domain.tag"));
        }
        if self.schema_version == 0 {
            return Err(DecoderError::Custom(
                "EventDescriptor: schema_version starts at 1"));
        }
        Ok(())
    }
}

impl Encodable for EventDescriptor {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(5);
        s.append(&self.id);
        s.append(&self.name);
        s.append(&self.domain);
        s.append(&self.severity);
        s.append(&self.schema_version);
    }
}

impl Decodable for EventDescriptor {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        if rlp.item_count()? != 5 { return Err(DecoderError::RlpIncorrectListLen); }
        let d = Self {
            id:             rlp.val_at(0)?,
            name:           rlp.val_at(1)?,
            domain:         rlp.val_at(2)?,
            severity:       rlp.val_at(3)?,
            schema_version: rlp.val_at(4)?,
        };
        d.validate()?;
        Ok(d)
    }
}

// ---------------------------------------------------------------------------
// EventRegistry — canonical catalogue.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventRegistry {
    pub entries: BTreeMap<EventId, EventDescriptor>,
}

impl Default for EventRegistry {
    fn default() -> Self { Self::installed_default() }
}

impl EventRegistry {
    pub fn new() -> Self { Self { entries: BTreeMap::new() } }

    pub fn validate(&self) -> Result<(), DecoderError> {
        for (id, d) in &self.entries {
            if &d.id != id {
                return Err(DecoderError::Custom("EventRegistry: id/key mismatch"));
            }
            d.validate()?;
        }
        Ok(())
    }

    /// Insert a descriptor. Rejects duplicates and name collisions.
    pub fn register(&mut self, d: EventDescriptor) -> Result<(), DecoderError> {
        d.validate()?;
        if self.entries.contains_key(&d.id) {
            return Err(DecoderError::Custom("EventRegistry: id already registered"));
        }
        if self.entries.values().any(|e| e.name == d.name) {
            return Err(DecoderError::Custom("EventRegistry: name already registered"));
        }
        self.entries.insert(d.id, d);
        Ok(())
    }

    pub fn get(&self, id: &EventId) -> Option<&EventDescriptor> {
        self.entries.get(id)
    }

    pub fn by_name(&self, name: &str) -> Option<&EventDescriptor> {
        self.entries.values().find(|d| d.name == name)
    }

    pub fn count_by_domain(&self, domain: EventDomain) -> usize {
        self.entries.values().filter(|d| d.domain == domain).count()
    }

    /// Build the canonical chain-event catalogue. New events MUST be
    /// added here, not registered ad-hoc by individual crates.
    pub fn installed_default() -> Self {
        // Helper to register-or-panic — these constants are static, so a
        // panic here would only fire during unit tests if the catalogue
        // is broken (duplicate id, bad name, etc.).
        fn add(reg: &mut EventRegistry,
               domain: EventDomain, seq: u32, name: &str, sev: EventSeverity) {
            let id = EventId::new(domain.tag(), seq);
            let d  = EventDescriptor::new(id, name, domain, sev, 1)
                .expect("static EventDescriptor invalid (catalogue bug)");
            reg.register(d).expect("static EventDescriptor duplicate (catalogue bug)");
        }

        let mut reg = Self::new();

        // ---- Consensus (1) ----
        add(&mut reg, EventDomain::Consensus, 1, "block_proposed",   EventSeverity::Info);
        add(&mut reg, EventDomain::Consensus, 2, "block_finalized",  EventSeverity::Info);
        add(&mut reg, EventDomain::Consensus, 3, "view_change",      EventSeverity::Warn);
        add(&mut reg, EventDomain::Consensus, 4, "validator_jailed", EventSeverity::Critical);
        add(&mut reg, EventDomain::Consensus, 5, "validator_unjailed", EventSeverity::Info);
        add(&mut reg, EventDomain::Consensus, 6, "epoch_rotated",    EventSeverity::Info);

        // ---- Execution (2) ----
        add(&mut reg, EventDomain::Execution, 1, "tx_executed",      EventSeverity::Info);
        add(&mut reg, EventDomain::Execution, 2, "tx_reverted",      EventSeverity::Warn);
        add(&mut reg, EventDomain::Execution, 3, "out_of_gas",       EventSeverity::Warn);
        add(&mut reg, EventDomain::Execution, 4, "evm_log_emitted",  EventSeverity::Info);

        // ---- Governance (3) ----
        add(&mut reg, EventDomain::Governance, 1, "proposal_submitted", EventSeverity::Info);
        add(&mut reg, EventDomain::Governance, 2, "vote_cast",          EventSeverity::Info);
        add(&mut reg, EventDomain::Governance, 3, "proposal_finalized", EventSeverity::Info);
        add(&mut reg, EventDomain::Governance, 4, "upgrade_executed",   EventSeverity::Critical);

        // ---- DeFi (4) ----
        add(&mut reg, EventDomain::Defi, 1, "swap",                 EventSeverity::Info);
        add(&mut reg, EventDomain::Defi, 2, "liquidity_added",      EventSeverity::Info);
        add(&mut reg, EventDomain::Defi, 3, "liquidity_removed",    EventSeverity::Info);
        add(&mut reg, EventDomain::Defi, 4, "twap_observation",     EventSeverity::Info);
        add(&mut reg, EventDomain::Defi, 5, "slippage_exceeded",    EventSeverity::Warn);

        // ---- Staking (5) ----
        add(&mut reg, EventDomain::Staking, 1, "stake_deposited",   EventSeverity::Info);
        add(&mut reg, EventDomain::Staking, 2, "stake_withdrawn",   EventSeverity::Info);
        add(&mut reg, EventDomain::Staking, 3, "reward_claimed",    EventSeverity::Info);
        add(&mut reg, EventDomain::Staking, 4, "validator_slashed", EventSeverity::Critical);

        // ---- Bundler (6) ----
        add(&mut reg, EventDomain::Bundler, 1, "userop_submitted",  EventSeverity::Info);
        add(&mut reg, EventDomain::Bundler, 2, "userop_included",   EventSeverity::Info);
        add(&mut reg, EventDomain::Bundler, 3, "userop_failed",     EventSeverity::Warn);

        // ---- Bridge (7) ----
        add(&mut reg, EventDomain::Bridge, 1, "bridge_lock",        EventSeverity::Info);
        add(&mut reg, EventDomain::Bridge, 2, "bridge_release",     EventSeverity::Info);
        add(&mut reg, EventDomain::Bridge, 3, "bridge_paused",      EventSeverity::Critical);

        // ---- Oracle (8) ----
        add(&mut reg, EventDomain::Oracle, 1, "feed_updated",       EventSeverity::Info);
        add(&mut reg, EventDomain::Oracle, 2, "feed_stale",         EventSeverity::Warn);
        add(&mut reg, EventDomain::Oracle, 3, "feed_paused",        EventSeverity::Critical);

        // ---- Slashing (9) ----
        add(&mut reg, EventDomain::Slashing, 1, "evidence_submitted", EventSeverity::Warn);
        add(&mut reg, EventDomain::Slashing, 2, "penalty_applied",    EventSeverity::Critical);

        // ---- Network (10) ----
        add(&mut reg, EventDomain::Network, 1, "peer_connected",      EventSeverity::Info);
        add(&mut reg, EventDomain::Network, 2, "peer_disconnected",   EventSeverity::Info);
        add(&mut reg, EventDomain::Network, 3, "peer_banned",         EventSeverity::Warn);
        add(&mut reg, EventDomain::Network, 4, "eclipse_suspected",   EventSeverity::Critical);

        reg
    }
}

impl Encodable for EventRegistry {
    fn rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(self.entries.len());
        for d in self.entries.values() { s.append(d); }
    }
}

impl Decodable for EventRegistry {
    fn decode(rlp: &Rlp) -> Result<Self, DecoderError> {
        let mut entries = BTreeMap::new();
        for i in 0..rlp.item_count()? {
            let d: EventDescriptor = rlp.val_at(i)?;
            entries.insert(d.id, d);
        }
        let r = Self { entries };
        r.validate()?;
        Ok(r)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_packs_domain_and_seq() {
        let id = EventId::new(EventDomain::Consensus.tag(), 42);
        assert_eq!(id.domain_tag(), 1);
        assert_eq!(id.sequence(), 42);
    }

    #[test]
    fn event_id_truncates_overflow_sequence() {
        let id = EventId::new(EventDomain::Defi.tag(), 0xFFFF_FFFF);
        assert_eq!(id.domain_tag(), 4);
        assert_eq!(id.sequence(), 0x00FF_FFFF);
    }

    #[test]
    fn descriptor_rejects_uppercase_name() {
        let id = EventId::new(EventDomain::Defi.tag(), 1);
        assert!(EventDescriptor::new(id, "Swap", EventDomain::Defi,
                                     EventSeverity::Info, 1).is_err());
    }

    #[test]
    fn descriptor_rejects_id_domain_mismatch() {
        let id = EventId::new(EventDomain::Defi.tag(), 1);
        assert!(EventDescriptor::new(id, "swap", EventDomain::Consensus,
                                     EventSeverity::Info, 1).is_err());
    }

    #[test]
    fn descriptor_rejects_empty_or_oversize_name() {
        let id = EventId::new(EventDomain::Defi.tag(), 1);
        assert!(EventDescriptor::new(id, "", EventDomain::Defi,
                                     EventSeverity::Info, 1).is_err());
        let big = "a".repeat(EventDescriptor::MAX_NAME_LEN + 1);
        assert!(EventDescriptor::new(id, big, EventDomain::Defi,
                                     EventSeverity::Info, 1).is_err());
    }

    #[test]
    fn descriptor_rejects_zero_schema_version() {
        let id = EventId::new(EventDomain::Defi.tag(), 1);
        assert!(EventDescriptor::new(id, "swap", EventDomain::Defi,
                                     EventSeverity::Info, 0).is_err());
    }

    #[test]
    fn registry_register_rejects_duplicate_id() {
        let mut r = EventRegistry::new();
        let id = EventId::new(EventDomain::Defi.tag(), 1);
        let d  = EventDescriptor::new(id, "swap", EventDomain::Defi,
                                      EventSeverity::Info, 1).unwrap();
        r.register(d.clone()).unwrap();
        assert!(r.register(d).is_err());
    }

    #[test]
    fn registry_register_rejects_duplicate_name() {
        let mut r = EventRegistry::new();
        let d1 = EventDescriptor::new(EventId::new(EventDomain::Defi.tag(), 1),
                                      "swap", EventDomain::Defi,
                                      EventSeverity::Info, 1).unwrap();
        let d2 = EventDescriptor::new(EventId::new(EventDomain::Defi.tag(), 2),
                                      "swap", EventDomain::Defi,
                                      EventSeverity::Info, 1).unwrap();
        r.register(d1).unwrap();
        assert!(r.register(d2).is_err());
    }

    #[test]
    fn installed_default_has_canonical_catalogue() {
        let r = EventRegistry::installed_default();
        // Sanity check known events.
        assert!(r.by_name("block_proposed").is_some());
        assert!(r.by_name("validator_slashed").is_some());
        assert!(r.by_name("eclipse_suspected").is_some());
        assert!(r.by_name("twap_observation").is_some());
        assert!(r.by_name("upgrade_executed").is_some());
        // Unknown event should not exist.
        assert!(r.by_name("nope").is_none());
    }

    #[test]
    fn installed_default_per_domain_counts() {
        let r = EventRegistry::installed_default();
        assert_eq!(r.count_by_domain(EventDomain::Consensus),  6);
        assert_eq!(r.count_by_domain(EventDomain::Execution),  4);
        assert_eq!(r.count_by_domain(EventDomain::Governance), 4);
        assert_eq!(r.count_by_domain(EventDomain::Defi),       5);
        assert_eq!(r.count_by_domain(EventDomain::Staking),    4);
        assert_eq!(r.count_by_domain(EventDomain::Bundler),    3);
        assert_eq!(r.count_by_domain(EventDomain::Bridge),     3);
        assert_eq!(r.count_by_domain(EventDomain::Oracle),     3);
        assert_eq!(r.count_by_domain(EventDomain::Slashing),   2);
        assert_eq!(r.count_by_domain(EventDomain::Network),    4);
        // Total should be 38.
        let total: usize = [1,2,3,4,5,6,7,8,9,10].iter()
            .map(|t| r.count_by_domain(EventDomain::from_tag(*t).unwrap()))
            .sum();
        assert_eq!(total, 38);
        assert_eq!(r.entries.len(), 38);
    }

    #[test]
    fn registry_get_by_id_works() {
        let r = EventRegistry::installed_default();
        let id = EventId::new(EventDomain::Defi.tag(), 4);
        assert_eq!(r.get(&id).unwrap().name, "twap_observation");
    }

    #[test]
    fn rlp_round_trip_descriptor() {
        let d = EventDescriptor::new(
            EventId::new(EventDomain::Defi.tag(), 1),
            "swap", EventDomain::Defi, EventSeverity::Info, 7,
        ).unwrap();
        let enc = rlp::encode(&d);
        let dec: EventDescriptor = rlp::decode(&enc).unwrap();
        assert_eq!(d, dec);
    }

    #[test]
    fn rlp_round_trip_full_default_registry() {
        let r = EventRegistry::installed_default();
        let enc = rlp::encode(&r);
        let dec: EventRegistry = rlp::decode(&enc).unwrap();
        assert_eq!(r, dec);
        assert_eq!(dec.entries.len(), 38);
    }

    #[test]
    fn rlp_round_trip_domain_and_severity() {
        for d in [EventDomain::Consensus, EventDomain::Execution,
                  EventDomain::Governance, EventDomain::Defi,
                  EventDomain::Staking, EventDomain::Bundler,
                  EventDomain::Bridge, EventDomain::Oracle,
                  EventDomain::Slashing, EventDomain::Network] {
            let enc = rlp::encode(&d);
            let dec: EventDomain = rlp::decode(&enc).unwrap();
            assert_eq!(d, dec);
        }
        for s in [EventSeverity::Info, EventSeverity::Warn, EventSeverity::Critical] {
            let enc = rlp::encode(&s);
            let dec: EventSeverity = rlp::decode(&enc).unwrap();
            assert_eq!(s, dec);
        }
        assert!(EventDomain::from_tag(99).is_err());
        assert!(EventSeverity::from_tag(99).is_err());
    }
}
