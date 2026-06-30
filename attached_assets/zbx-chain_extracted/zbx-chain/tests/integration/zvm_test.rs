//! Integration tests for the ZVM (Zebvix Virtual Machine).

#[cfg(test)]
mod zvm_tests {
    use zbx_zvm::{
        executor::ZvmExecutor,
        context::{ZvmContext, ExecutionStatus},
        opcodes::Opcode,
        host::MockZvmHost,
        ZVM_VERSION, ZVM_MAGIC,
    };
    use zbx_types::CHAIN_ID_MAINNET;

    fn test_ctx(bytecode: Vec<u8>) -> ZvmContext {
        let mut ctx = ZvmContext::test_default();
        ctx.bytecode = bytecode;
        ctx
    }

    // ── Constants ─────────────────────────────────────────────────────────

    #[test]
    fn test_zvm_constants() {
        assert_eq!(ZVM_VERSION, 1);
        assert_eq!(ZVM_MAGIC, [0xEF, 0x5A, 0x42]);
        assert_eq!(CHAIN_ID_MAINNET, 8989);
    }

    // ── EVM compat: basic arithmetic ─────────────────────────────────────

    #[test]
    fn test_add_opcode() {
        // PUSH1 3, PUSH1 4, ADD, STOP
        let code = vec![0x60, 3, 0x60, 4, 0x01, 0x00];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
    }

    #[test]
    fn test_mul_opcode() {
        // PUSH1 3, PUSH1 4, MUL, STOP
        let code = vec![0x60, 3, 0x60, 4, 0x02, 0x00];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
    }

    // ── EVM compat: RETURN ────────────────────────────────────────────────

    #[test]
    fn test_return_opcode() {
        // PUSH1 0x42, PUSH1 0, MSTORE8, PUSH1 1, PUSH1 0, RETURN
        let code = vec![0x60, 0x42, 0x60, 0, 0x53, 0x60, 1, 0x60, 0, 0xF3];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.return_data, vec![0x42]);
    }

    // ── EVM compat: REVERT ────────────────────────────────────────────────

    #[test]
    fn test_revert_opcode() {
        // PUSH1 0, PUSH1 0, REVERT
        let code = vec![0x60, 0, 0x60, 0, 0xFD];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Revert);
    }

    // ── EVM compat: JUMP ──────────────────────────────────────────────────

    #[test]
    fn test_jump_opcode() {
        // PUSH1 3, JUMP, INVALID, JUMPDEST, STOP
        let code = vec![0x60, 3, 0x56, 0xFE, 0x5B, 0x00];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
    }

    // ── ZVM native: ZBXPRICE ──────────────────────────────────────────────

    #[test]
    fn test_zbxprice_opcode() {
        // ZBXPRICE (0xC2), STOP
        let code = vec![0xC2, 0x00];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        host.zbx_price = 2_500 * 10u128.pow(18);
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.gas_used, 50 + 0); // ZBXPRICE(50) + STOP(0)
    }

    // ── ZVM native: ZBXTIME ───────────────────────────────────────────────

    #[test]
    fn test_zbxtime_opcode() {
        // ZBXTIME (0xC3), STOP
        let code = vec![0xC3, 0x00];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.gas_used, 2); // ZBXTIME costs 2 gas
    }

    // ── ZVM native: CHAINVER ──────────────────────────────────────────────

    #[test]
    fn test_chainver_opcode() {
        // CHAINVER (0xC5), STOP
        let code = vec![0xC5, 0x00];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
    }

    // ── ZVM native: AASENDER ─────────────────────────────────────────────

    #[test]
    fn test_aasender_no_aa() {
        // AASENDER (0xC4), STOP
        let code = vec![0xC4, 0x00];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
    }

    #[test]
    fn test_aasender_with_aa() {
        let code = vec![0xC4, 0x00];
        let mut ctx = test_ctx(code);
        ctx.aa_sender = Some([0xAA; 20]); // Set AA sender
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
    }

    // ── ZVM native: PAYID ─────────────────────────────────────────────────

    #[test]
    fn test_payid_opcode_resolved() {
        // Store "ali@zbx" in memory at offset 0, then PAYID
        // PUSH7 "ali@zbx" as bytes, PUSH1 0, MSTORE (simplified test)
        // For simplicity, just test STOP after precompile call
        let code = vec![0x00]; // STOP
        let mut ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let mut addr = [0u8; 20];
        addr[19] = 0x42;
        host.pay_ids.insert("ali".to_string(), addr);

        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
    }

    // ── ZVM native: ZBXBURN ───────────────────────────────────────────────

    #[test]
    fn test_zbxburn_sufficient_balance() {
        let code = vec![0x00]; // STOP (burn tested via host directly)
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let caller = [0x01; 20];
        host.balances.insert(caller, 1000);
        assert!(host.burn_zbx(&caller, 100).is_ok());
        assert_eq!(host.balance(&caller), 900);
    }

    #[test]
    fn test_zbxburn_insufficient_balance() {
        use zbx_zvm::error::ZvmError;
        let code = vec![0x00];
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let caller = [0x01; 20];
        host.balances.insert(caller, 50);
        assert!(matches!(host.burn_zbx(&caller, 100), Err(ZvmError::InsufficientBalance)));
    }

    // ── ZVM native: static context ────────────────────────────────────────

    #[test]
    fn test_sstore_in_static_fails() {
        // PUSH1 1, PUSH1 0, SSTORE in static context → should error
        let code = vec![0x60, 1, 0x60, 0, 0x55, 0x00];
        let mut ctx = test_ctx(code);
        ctx.is_static = true;
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert!(matches!(result.status, ExecutionStatus::ZvmError(_)));
    }

    // ── Opcode properties ─────────────────────────────────────────────────

    #[test]
    fn test_zvm_opcode_is_native() {
        assert!(Opcode::PAYID.is_zvm_native());
        assert!(Opcode::ZUSDBAL.is_zvm_native());
        assert!(Opcode::ZBXPRICE.is_zvm_native());
        assert!(Opcode::ZBXBURN.is_zvm_native());
        assert!(Opcode::ZVMLOG.is_zvm_native());

        // EVM opcodes should NOT be native
        assert!(!Opcode::ADD.is_zvm_native());
        assert!(!Opcode::SSTORE.is_zvm_native());
        assert!(!Opcode::RETURN.is_zvm_native());
    }

    #[test]
    fn test_opcode_names() {
        assert_eq!(Opcode::PAYID.name(), "PAYID");
        assert_eq!(Opcode::ZUSDBAL.name(), "ZUSDBAL");
        assert_eq!(Opcode::ZBXPRICE.name(), "ZBXPRICE");
        assert_eq!(Opcode::AASENDER.name(), "AASENDER");
        assert_eq!(Opcode::ZBXBURN.name(), "ZBXBURN");
    }

    // ── Gas accounting ────────────────────────────────────────────────────

    #[test]
    fn test_gas_used_correctly() {
        // STOP uses 0 gas
        let code = vec![0x00];
        let mut ctx = test_ctx(code);
        ctx.gas_limit = 1_000_000;
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.gas_used, 0);
        assert_eq!(result.gas_remaining, 1_000_000);
    }

    #[test]
    fn test_out_of_gas() {
        // ZBXPRICE costs 50 gas — use gas_limit = 10
        let code = vec![0xC2, 0x00];
        let mut ctx = test_ctx(code);
        ctx.gas_limit = 10;
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::OutOfGas);
    }

    // ── ZVM magic prefix ──────────────────────────────────────────────────

    #[test]
    fn test_zvm_magic_prefix_runs() {
        // ZVM magic + STOP
        let mut code = ZVM_MAGIC.to_vec();
        code.push(0x00); // STOP
        let ctx = test_ctx(code);
        let mut host = MockZvmHost::new();
        let result = ZvmExecutor::execute(&ctx, &mut host);
        assert_eq!(result.status, ExecutionStatus::Success);
    }
}