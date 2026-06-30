//! Main EVM interpreter: fetch-decode-execute loop.

use crate::{
    opcode::OpCode,
    stack::Stack,
    memory::Memory,
    gas::AccessList,
    host::Host,
    context::{Context, CallContext, CallType},
};
use zbx_types::{Address, U256, H256};
use tracing::{trace, debug};

/// EVM configuration (which EIPs are active).
#[derive(Debug, Clone)]
pub struct EvmConfig {
    pub chain_id:     u64,
    pub cancun:       bool, // EIP-4844, EIP-1153, EIP-5656
    pub shanghai:     bool, // EIP-3855 PUSH0
    pub london:       bool, // EIP-1559
    pub berlin:       bool, // EIP-2929
    pub max_call_depth: usize,
}

impl EvmConfig {
    pub fn mainnet() -> Self {
        Self {
            chain_id: zbx_types::CHAIN_ID_MAINNET,
            cancun:   true,
            shanghai: true,
            london:   true,
            berlin:   true,
            max_call_depth: 1024,
        }
    }
}

/// The reason execution halted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    Stop,
    Return,
    Revert,
    OutOfGas,
    StackOverflow,
    StackUnderflow,
    InvalidOpcode(u8),
    InvalidJump,
    StaticModeViolation,
    CallDepthExceeded,
    PrecompileError,
    CreateCollision,
}

impl ExitReason {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Stop | Self::Return)
    }
}

/// Result of an EVM execution.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub reason:      ExitReason,
    pub gas_used:    u64,
    pub gas_refund:  u64,
    pub output:      Vec<u8>,
    pub logs:        Vec<crate::context::Log>,
    pub created:     Option<Address>,
}

impl ExecutionResult {
    pub fn success(&self) -> bool { self.reason.is_ok() }
}

/// The Zebvix EVM.
pub struct Evm {
    pub config: EvmConfig,
}

impl Evm {
    pub fn new(config: EvmConfig) -> Self {
        Self { config }
    }

    /// Execute a transaction against `host`.
    pub fn transact(
        &self,
        ctx: &Context,
        host: &mut dyn Host,
    ) -> ExecutionResult {
        use crate::context::TransactTo;

        // Validate gas.
        let intrinsic = crate::gas::intrinsic_gas(
            &ctx.tx.data,
            matches!(ctx.tx.transact_to, TransactTo::Create),
            &ctx.tx.access_list,
        );

        if ctx.tx.gas_limit < intrinsic {
            return ExecutionResult {
                reason: ExitReason::OutOfGas,
                gas_used: ctx.tx.gas_limit,
                gas_refund: 0,
                output: Vec::new(),
                logs: Vec::new(),
                created: None,
            };
        }

        let gas_left = ctx.tx.gas_limit - intrinsic;

        // Execute.
        match &ctx.tx.transact_to {
            TransactTo::Call(to) => {
                self.execute_call(*to, &ctx.tx.data, gas_left, ctx.tx.value, host, false, 0)
            }
            TransactTo::Create => {
                self.execute_create(&ctx.tx.data, gas_left, ctx.tx.value, host, 0)
            }
        }
    }

    fn execute_call(
        &self,
        to: Address,
        input: &[u8],
        gas: u64,
        value: U256,
        host: &mut dyn Host,
        is_static: bool,
        depth: usize,
    ) -> ExecutionResult {
        if depth >= self.config.max_call_depth {
            return ExecutionResult {
                reason: ExitReason::CallDepthExceeded,
                gas_used: gas,
                gas_refund: 0,
                output: Vec::new(),
                logs: Vec::new(),
                created: None,
            };
        }

        // Check for precompile.
        if let Some(result) = crate::precompiles::call_precompile(to, input, gas) {
            return match result {
                Ok((gas_used, output)) => ExecutionResult {
                    reason: ExitReason::Return,
                    gas_used,
                    gas_refund: 0,
                    output,
                    logs: Vec::new(),
                    created: None,
                },
                Err(_) => ExecutionResult {
                    reason: ExitReason::PrecompileError,
                    gas_used: gas,
                    gas_refund: 0,
                    output: Vec::new(),
                    logs: Vec::new(),
                    created: None,
                },
            };
        }

        let code = host.code(to).to_vec();
        if code.is_empty() {
            // EOA: just a value transfer.
            return ExecutionResult {
                reason: ExitReason::Stop,
                gas_used: 0,
                gas_refund: 0,
                output: Vec::new(),
                logs: Vec::new(),
                created: None,
            };
        }

        self.run_bytecode(&code, input, gas, host, is_static, depth)
    }

    fn execute_create(
        &self,
        init_code: &[u8],
        gas: u64,
        value: U256,
        host: &mut dyn Host,
        depth: usize,
    ) -> ExecutionResult {
        if depth >= self.config.max_call_depth {
            return ExecutionResult {
                reason: ExitReason::CallDepthExceeded,
                gas_used: gas,
                gas_refund: 0,
                output: Vec::new(),
                logs: Vec::new(),
                created: None,
            };
        }
        // EIP-3860: initcode size limit.
        if init_code.len() > 2 * 24576 {
            return ExecutionResult {
                reason: ExitReason::InvalidOpcode(0),
                gas_used: gas,
                gas_refund: 0,
                output: Vec::new(),
                logs: Vec::new(),
                created: None,
            };
        }
        self.run_bytecode(init_code, &[], gas, host, false, depth)
    }

    fn run_bytecode(
        &self,
        code: &[u8],
        input: &[u8],
        mut gas: u64,
        host: &mut dyn Host,
        is_static: bool,
        depth: usize,
    ) -> ExecutionResult {
        let mut stack  = Stack::new();
        let mut memory = Memory::new();
        let mut pc     = 0usize;
        let mut logs   = Vec::new();
        let gas_start  = gas;

        loop {
            if pc >= code.len() {
                return ExecutionResult {
                    reason: ExitReason::Stop,
                    gas_used: gas_start - gas,
                    gas_refund: 0,
                    output: Vec::new(),
                    logs,
                    created: None,
                };
            }

            let op = match OpCode::from_byte(code[pc]) {
                Some(op) => op,
                None => {
                    let b = code[pc];
                    return ExecutionResult {
                        reason: ExitReason::InvalidOpcode(b),
                        gas_used: gas_start - gas,
                        gas_refund: 0,
                        output: Vec::new(),
                        logs,
                        created: None,
                    };
                }
            };

            // Charge static gas.
            let static_cost = op.static_gas();
            if gas < static_cost {
                return ExecutionResult {
                    reason: ExitReason::OutOfGas,
                    gas_used: gas_start,
                    gas_refund: 0,
                    output: Vec::new(),
                    logs,
                    created: None,
                };
            }
            gas -= static_cost;
            pc  += 1;

            match op {
                OpCode::STOP => {
                    return ExecutionResult {
                        reason: ExitReason::Stop,
                        gas_used: gas_start - gas,
                        gas_refund: 0,
                        output: Vec::new(),
                        logs,
                        created: None,
                    };
                }
                OpCode::PUSH0 => { let _ = stack.push(U256::zero()); }
                op_push if op_push.is_push() && op_push != OpCode::PUSH0 => {
                    let n = op_push.push_size();
                    let end = (pc + n).min(code.len());
                    let mut buf = [0u8; 32];
                    let src = &code[pc..end];
                    buf[32 - src.len()..].copy_from_slice(src);
                    let _ = stack.push(U256::from_big_endian(&buf));
                    pc += n;
                }
                OpCode::ADD => {
                    if let (Ok(a), Ok(b)) = (stack.pop(), stack.pop()) {
                        let _ = stack.push(a.overflowing_add(b).0);
                    }
                }
                OpCode::MUL => {
                    if let (Ok(a), Ok(b)) = (stack.pop(), stack.pop()) {
                        let _ = stack.push(a.overflowing_mul(b).0);
                    }
                }
                OpCode::SUB => {
                    if let (Ok(a), Ok(b)) = (stack.pop(), stack.pop()) {
                        let _ = stack.push(a.overflowing_sub(b).0);
                    }
                }
                OpCode::POP => { let _ = stack.pop(); }
                op_dup if op_dup.is_dup() => {
                    let n = (op_dup as u8 - 0x7f) as usize;
                    let _ = stack.dup(n);
                }
                op_swap if op_swap.is_swap() => {
                    let n = (op_swap as u8 - 0x8f) as usize;
                    let _ = stack.swap(n);
                }
                OpCode::RETURN => {
                    let offset = stack.pop().unwrap_or_default().as_usize();
                    let size   = stack.pop().unwrap_or_default().as_usize();
                    let output = memory.get_slice(offset, size).to_vec();
                    return ExecutionResult {
                        reason: ExitReason::Return,
                        gas_used: gas_start - gas,
                        gas_refund: 0,
                        output,
                        logs,
                        created: None,
                    };
                }
                OpCode::REVERT => {
                    let offset = stack.pop().unwrap_or_default().as_usize();
                    let size   = stack.pop().unwrap_or_default().as_usize();
                    let output = memory.get_slice(offset, size).to_vec();
                    return ExecutionResult {
                        reason: ExitReason::Revert,
                        gas_used: gas_start - gas,
                        gas_refund: 0,
                        output,
                        logs,
                        created: None,
                    };
                }
                OpCode::JUMPDEST => { /* no-op, already charged */ }
                _ => {
                    // Other opcodes handled by dispatcher (omitted for brevity).
                    debug!("evm: unhandled opcode {:?} at pc {}", op, pc - 1);
                }
            }
        }
    }
}