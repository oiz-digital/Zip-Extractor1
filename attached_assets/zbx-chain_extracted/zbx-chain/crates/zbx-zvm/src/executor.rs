//! ZVM executor — top-level entry point for contract execution.

use crate::{
    interpreter::ZvmInterpreter,
    context::{ZvmContext, ZvmResult, ExecutionStatus},
    host::ZvmHost,
    precompiles,
};
use tracing::info;

pub struct ZvmExecutor;

impl ZvmExecutor {
    /// Execute a contract call in the ZVM.
    pub fn execute<H: ZvmHost>(ctx: &ZvmContext, host: &mut H) -> ZvmResult {
        info!(
            caller = hex::encode(ctx.caller),
            contract = hex::encode(ctx.contract),
            gas = ctx.gas_limit,
            "ZVM execute"
        );

        // EXEC-01 FIX (HIGH): route precompile calls through the host-aware
        // dispatcher. Pre-fix, stateful precompiles 0x0A (PayID), 0x0C (Price
        // Oracle), and 0x0F (ZUSD Vault) were dispatched via the stateless
        // `call_precompile` function which has no host access and fail-closes
        // for those addresses. Any top-level transaction that directly targets
        // one of these precompiles would unconditionally fail. Now we pass the
        // host so stateful precompiles can read chain state just as they do
        // inside the interpreter's `do_precompile_call`.
        if is_precompile(&ctx.contract) {
            return Self::execute_precompile_with_host(ctx, host);
        }

        // Check for ZVM magic prefix (optional — enables ZVM-specific tooling)
        let is_zvm_native = ctx.bytecode.starts_with(&crate::ZVM_MAGIC);
        if is_zvm_native {
            info!("ZVM-native contract detected (magic prefix)");
        }

        let mut interpreter = ZvmInterpreter::new(ctx, host);
        interpreter.run()
    }

    /// Dispatch a top-level call to a precompile address (0x01..=0x0F).
    ///
    /// Stateful precompiles (0x0A PayID, 0x0C Price Oracle, 0x0F ZUSD Vault)
    /// are routed through host adapters that bridge `ZvmHost` to the trait
    /// each precompile body requires. Stateless precompiles fall through to
    /// the shared `call_precompile` dispatcher.
    fn execute_precompile_with_host<H: ZvmHost>(ctx: &ZvmContext, host: &mut H) -> ZvmResult {
        let id   = ctx.contract[19];
        let gas  = ctx.gas_limit;
        let data = &ctx.calldata;

        let result: Result<(Vec<u8>, u64), crate::error::ZvmError> = match id {
            // 0x0A — PayID forward/reverse resolver (stateful, needs host).
            0x0A => {
                struct HostAdapter<'a, H: ZvmHost + ?Sized>(&'a H);
                impl<H: ZvmHost + ?Sized> precompiles::PayIdLookup for HostAdapter<'_, H> {
                    fn resolve(&self, name: &[u8]) -> Option<[u8; 20]> {
                        self.0.resolve_pay_id_bytes(name)
                    }
                    fn reverse(&self, addr: &[u8; 20]) -> Option<Vec<u8>> {
                        self.0.reverse_pay_id(addr)
                    }
                }
                precompiles::payid_resolve_with(data, gas, &HostAdapter(&*host))
            }

            // 0x0C — Price oracle reader (stateful, needs storage via host).
            0x0C => {
                struct OracleAdapter<'a, H: ZvmHost + ?Sized>(&'a H);
                impl<H: ZvmHost + ?Sized> zbx_crypto::oracle_state::OracleStateReader
                    for OracleAdapter<'_, H>
                {
                    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
                        self.0.storage_load(addr, slot)
                    }
                }
                precompiles::price_oracle_with(data, gas, &OracleAdapter(&*host))
            }

            // 0x0F — ZUSD vault state reader (stateful, needs storage + timestamp).
            0x0F => {
                struct VaultAdapter<'a, H: ZvmHost + ?Sized> {
                    host: &'a H,
                    ts:   u64,
                }
                impl<H: ZvmHost + ?Sized> zbx_crypto::vault_state::VaultStateReader
                    for VaultAdapter<'_, H>
                {
                    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
                        self.host.storage_load(addr, slot)
                    }
                    fn current_timestamp(&self) -> u64 { self.ts }
                }
                precompiles::zusd_vault_with(
                    data, gas,
                    &VaultAdapter { host: &*host, ts: ctx.block_timestamp },
                )
            }

            // All other precompile IDs (0x01..=0x09, 0x0B, 0x0D, 0x0E) are
            // stateless — delegate to the shared dispatcher.
            _ => precompiles::call_precompile(&ctx.contract, data, gas),
        };

        match result {
            Ok((output, gas_used)) => ZvmResult {
                status:        ExecutionStatus::Success,
                return_data:   output,
                gas_remaining: ctx.gas_limit.saturating_sub(gas_used),
                gas_used,
                logs:          vec![],
                zvm_logs:      vec![],
            },
            Err(e) => ZvmResult {
                status:        ExecutionStatus::ZvmError(e.to_string()),
                return_data:   vec![],
                gas_remaining: 0,
                gas_used:      ctx.gas_limit,
                logs:          vec![],
                zvm_logs:      vec![],
            },
        }
    }
}

fn is_precompile(addr: &[u8; 20]) -> bool {
    // All bytes 0–18 must be zero, byte 19 in range 0x01–0x0F
    addr[..19].iter().all(|b| *b == 0) && (0x01..=0x0F).contains(&addr[19])
}
