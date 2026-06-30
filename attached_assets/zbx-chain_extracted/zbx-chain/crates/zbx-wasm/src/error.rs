use thiserror::Error;

#[derive(Debug, Error)]
pub enum WasmError {
    #[error("module compilation failed: {0}")]
    CompilationFailed(String),
    #[error("instantiation failed: {0}")]
    InstantiationFailed(String),
    #[error("execution trapped: {0}")]
    Trap(String),
    #[error("out of gas: limit={limit}, used={used}")]
    OutOfGas { limit: u64, used: u64 },
    #[error("memory access violation at offset {0}")]
    MemoryViolation(u32),
    #[error("host function not found: {0}")]
    HostFnNotFound(String),
    #[error("invalid module: {0}")]
    InvalidModule(String),
    #[error("stack overflow in WASM execution")]
    StackOverflow,
    #[error("reentrancy detected in WASM call")]
    Reentrancy,
    #[error("storage error: {0}")]
    Storage(String),
    #[error("serialisation error: {0}")]
    Serialisation(String),
}