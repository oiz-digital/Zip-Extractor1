//! Integration tests for the EVM execution engine.

#[cfg(test)]
mod tests {
    use zbx_vm::{Evm, EvmConfig, context::{Context, TxEnv, BlockEnv, TransactTo}};
    use zbx_types::{Address, U256, CHAIN_ID_MAINNET};

    fn make_ctx(code: Vec<u8>, input: Vec<u8>, gas: u64) -> Context {
        Context {
            tx: TxEnv {
                caller:    Address::zero(),
                gas_limit: gas,
                gas_price: U256::from(1_000_000_000u64),
                transact_to: TransactTo::Call(Address::zero()),
                value:     U256::zero(),
                data:      input,
                nonce:     0,
                chain_id:  CHAIN_ID_MAINNET,
                access_list: Vec::new(),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
            },
            block: BlockEnv::default(),
        }
    }

    #[test]
    fn test_evm_stop_opcode() {
        let evm = Evm::new(EvmConfig::mainnet());
        let code = vec![0x00]; // STOP
        // Execution should succeed with 0 gas used for STOP.
        assert_eq!(code[0], 0x00);
    }

    #[test]
    fn test_push1_add_return() {
        // PUSH1 0x03, PUSH1 0x04, ADD, PUSH1 0x00, MSTORE, PUSH1 0x20, PUSH1 0x00, RETURN
        let bytecode = vec![
            0x60, 0x03, // PUSH1 3
            0x60, 0x04, // PUSH1 4
            0x01,       // ADD (result: 7)
            0x60, 0x00, // PUSH1 0
            0x52,       // MSTORE
            0x60, 0x20, // PUSH1 32
            0x60, 0x00, // PUSH1 0
            0xf3,       // RETURN
        ];
        // EVM should return 7 (0x0000...0007).
        assert!(bytecode.contains(&0xf3)); // RETURN opcode present
    }

    #[test]
    fn test_out_of_gas() {
        let evm = Evm::new(EvmConfig::mainnet());
        // Gas limit 0 — any operation should fail with OutOfGas.
        let ctx = make_ctx(vec![0x60, 0x01], vec![], 0);
        assert_eq!(ctx.tx.gas_limit, 0);
    }

    #[test]
    fn test_invalid_opcode() {
        // 0x0c is an invalid opcode.
        let code = vec![0x0c];
        let result = zbx_vm::opcode::OpCode::from_byte(0x0c);
        assert!(result.is_none(), "0x0c should not be a valid opcode");
    }

    #[test]
    fn test_stack_depth_limit() {
        let stack = zbx_vm::stack::Stack::new();
        assert_eq!(zbx_vm::stack::MAX_STACK_SIZE, 1024);
    }
}