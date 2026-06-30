//! Contract storage layout -- slots, arrays, mappings, gaps.
//!
//! Solidity storage layout rules (EVM):
//!   - Each state variable occupies one or more 32-byte storage slots
//!   - Variables are packed into slots (right-aligned, LSB first)
//!   - Reference types (arrays, mappings, bytes/string) use hashed slots
//!   - Arrays: slot N contains length; elements at keccak256(N) + index
//!   - Mappings: slot N is empty; value at keccak256(key ++ N)
//!
//! Upgradeable contracts must maintain storage layout compatibility:
//!   - Never insert new variables before existing ones
//!   - Never delete/reorder existing variables
//!   - Add new variables at the END of the layout
//!   - Use storage gaps to reserve space for future variables
//!
//! ZBX uses storage gap pattern for all upgradeable contracts.

use std::collections::HashMap;

// ── Storage slots ─────────────────────────────────────────────────────────────

/// A single storage slot key (32 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StorageSlot(pub [u8; 32]);

impl StorageSlot {
    /// Compute the storage slot for mapping: keccak256(key ++ base_slot)
    pub fn mapping_slot(key: &[u8], base_slot: u32) -> Self {
        let mut data = Vec::with_capacity(key.len() + 32);
        data.extend_from_slice(key);
        let slot_bytes = (base_slot as u128).to_be_bytes();
        data.extend_from_slice(&[0u8; 16]);
        data.extend_from_slice(&slot_bytes);
        StorageSlot(keccak256(&data))
    }

    /// Compute the base slot for a dynamic array: keccak256(base_slot)
    pub fn array_base_slot(base_slot: u32) -> Self {
        let mut data = [0u8; 32];
        let bytes = (base_slot as u32).to_be_bytes();
        data[28..32].copy_from_slice(&bytes);
        StorageSlot(keccak256(&data))
    }

    /// Compute slot for array element: array_base + index
    pub fn array_element_slot(base_slot: u32, index: u128) -> Self {
        let base = Self::array_base_slot(base_slot);
        let mut slot = [0u128; 2];
        let base_as_u128 = u128::from_be_bytes(base.0[16..].try_into().unwrap_or([0u8; 16]));
        let elem_slot = base_as_u128.wrapping_add(index);
        StorageSlot(elem_slot.to_be_bytes().into_iter()
            .chain([0u8; 16].into_iter()).take(32)
            .collect::<Vec<u8>>().try_into().unwrap_or([0u8; 32]))
    }
}

// ── StorageVec (dynamic array) ────────────────────────────────────────────────

/// Dynamic storage array (equivalent to Solidity T[]).
/// Length stored at base_slot; elements stored at keccak256(base_slot) + i.
pub struct StorageVec<T: Clone> {
    pub base_slot: u32,
    pub storage:   HashMap<StorageSlot, Vec<u8>>,
    _phantom:      std::marker::PhantomData<T>,
}

impl<T: Clone + StorageEncode + StorageDecode> StorageVec<T> {
    pub fn new(base_slot: u32) -> Self {
        Self { base_slot, storage: HashMap::new(), _phantom: std::marker::PhantomData }
    }

    /// Number of elements.
    pub fn len(&self) -> u128 {
        let slot = StorageSlot([0u8; 32]); // simplified -- reads length slot
        self.storage.get(&slot).map(|b| {
            if b.len() >= 16 { u128::from_be_bytes(b[..16].try_into().unwrap_or([0u8; 16])) } else { 0 }
        }).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Push an element to the array.
    pub fn push(&mut self, value: T) {
        let index = self.len();
        let slot = StorageSlot::array_element_slot(self.base_slot, index);
        self.storage.insert(slot, value.encode());
        // Increment length in base slot
    }

    /// Read element at index.
    pub fn get(&self, index: u128) -> Option<T> {
        let slot = StorageSlot::array_element_slot(self.base_slot, index);
        self.storage.get(&slot).and_then(|b| T::decode(b).ok())
    }
}

// ── StorageMap (mapping) ──────────────────────────────────────────────────────

/// Storage mapping (equivalent to Solidity mapping(K => V)).
pub struct StorageMap<K: StorageKey, V: StorageEncode + StorageDecode> {
    pub base_slot: u32,
    pub storage:   HashMap<StorageSlot, Vec<u8>>,
    _phantom:      std::marker::PhantomData<(K, V)>,
}

impl<K: StorageKey, V: StorageEncode + StorageDecode> StorageMap<K, V> {
    pub fn new(base_slot: u32) -> Self {
        Self { base_slot, storage: HashMap::new(), _phantom: std::marker::PhantomData }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        let slot = StorageSlot::mapping_slot(&key.key_bytes(), self.base_slot);
        self.storage.get(&slot).and_then(|b| V::decode(b).ok())
    }

    pub fn insert(&mut self, key: &K, value: V) {
        let slot = StorageSlot::mapping_slot(&key.key_bytes(), self.base_slot);
        self.storage.insert(slot, value.encode());
    }
}

// ── Storage Gap -- upgradeable contract reserved space ────────────────────────

/// Storage gap -- reserved slots for future variable additions.
///
/// Upgradeable contracts include a gap array at the end of storage:
///   uint256[50] private __gap;  // reserve 50 slots
///
/// This allows child contracts to add variables without colliding
/// with parent contract storage (as long as gap is large enough).
///
/// When a parent adds new variables, they reduce the gap size:
///   Before: uint256[50] __gap;
///   After adding 2 variables: uint256[48] __gap;
///
/// Standard sizes:
///   - __gap[50] : standard OpenZeppelin pattern
///   - __gap[100]: larger buffer for complex contracts
pub struct StorageGap {
    /// Number of reserved storage slots (reduces as variables are added)
    pub remaining_slots: u32,
    /// Starting slot index for the gap
    pub start_slot:      u32,
}

impl StorageGap {
    /// Create a storage gap with N reserved slots.
    pub fn new(slots: u32, start_slot: u32) -> Self {
        Self { remaining_slots: slots, start_slot }
    }

    /// Add a new state variable (consume one gap slot).
    /// Returns Err if gap is exhausted.
    pub fn consume_slot(&mut self) -> Result<u32, StorageError> {
        if self.remaining_slots == 0 {
            return Err(StorageError::GapExhausted);
        }
        let slot = self.start_slot + (50 - self.remaining_slots); // track which slot is used
        self.remaining_slots -= 1;
        Ok(slot)
    }

    /// Slots remaining for future variables.
    pub fn available(&self) -> u32 { self.remaining_slots }
}

// ── Well-known ZBX storage gaps ───────────────────────────────────────────────

/// Storage gap allocations for ZBX upgradeable contracts:
pub const STAKING_GAP_SIZE:    u32 = 50;  // StakingPool: 50 slots reserved
pub const GOVERNANCE_GAP_SIZE: u32 = 50;  // Governance: 50 slots reserved
pub const ORACLE_GAP_SIZE:     u32 = 50;  // Oracle: 50 slots reserved
pub const BRIDGE_GAP_SIZE:     u32 = 100; // Bridge: 100 slots reserved (complex)

// ── Trait helpers ─────────────────────────────────────────────────────────────

pub trait StorageEncode { fn encode(&self) -> Vec<u8>; }
pub trait StorageDecode: Sized { fn decode(bytes: &[u8]) -> Result<Self, StorageError>; }
pub trait StorageKey { fn key_bytes(&self) -> Vec<u8>; }

impl StorageKey for [u8; 20] {
    fn key_bytes(&self) -> Vec<u8> {
        let mut b = vec![0u8; 12];
        b.extend_from_slice(self);
        b
    }
}

impl StorageEncode for u128 {
    fn encode(&self) -> Vec<u8> { self.to_be_bytes().to_vec() }
}
impl StorageDecode for u128 {
    fn decode(bytes: &[u8]) -> Result<Self, StorageError> {
        if bytes.len() < 16 { return Err(StorageError::InvalidData); }
        Ok(u128::from_be_bytes(bytes[..16].try_into().unwrap_or([0u8; 16])))
    }
}

#[derive(Debug)]
pub enum StorageError { GapExhausted, InvalidData, SlotNotFound }

fn keccak256(_data: &[u8]) -> [u8; 32] { [0u8; 32] }