//! Halving schedule — every 25M blocks.

pub const HALVING_INTERVAL: u64 = 25_000_000;
pub const BLOCKS_PER_YEAR:  f64 = 6_307_200.0;

#[derive(Debug, Clone)]
pub struct HalvingEvent {
    pub epoch:             u32,
    pub block_number:      u64,
    pub reward_zbx:        f64,
    pub years_from_genesis: f64,
}

pub struct HalvingSchedule;
impl HalvingSchedule {
    pub fn schedule() -> Vec<HalvingEvent> {
        let mut v = Vec::new();
        let mut r = 3.0f64;
        for epoch in 0u32..64 {
            let bn = epoch as u64 * HALVING_INTERVAL;
            v.push(HalvingEvent { epoch, block_number: bn, reward_zbx: r, years_from_genesis: bn as f64 / BLOCKS_PER_YEAR });
            r /= 2.0;
            if r < 1e-18 { break; }
        }
        v
    }
    pub fn epoch_for_block(b: u64) -> u32 { (b / HALVING_INTERVAL) as u32 }
    pub fn next_halving(b: u64)    -> u64  { ((b / HALVING_INTERVAL) + 1) * HALVING_INTERVAL }
}