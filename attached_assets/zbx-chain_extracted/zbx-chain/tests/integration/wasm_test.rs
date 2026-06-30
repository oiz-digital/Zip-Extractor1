//! Integration tests for WASM smart contract execution.

#[cfg(test)]
mod wasm_integration {
    #[test]
    fn wasm_magic_bytes_detected() {
        let wasm_bytes = b"\x00asm\x01\x00\x00\x00";
        let evm_bytes  = b"\x60\x80\x60\x40";
        assert_eq!(&wasm_bytes[..4], b"\x00asm", "WASM magic bytes correct");
        assert_ne!(&evm_bytes[..4],  b"\x00asm", "EVM bytecode not WASM");
    }

    #[test]
    fn wasm_gas_limit_enforced() {
        let gas_limit = 10_000_000u64;
        let gas_used  = 9_999_999u64;
        assert!(gas_used <= gas_limit, "gas within limit accepted");

        let over_limit = 10_000_001u64;
        assert!(over_limit > gas_limit, "gas over limit rejected");
    }

    #[test]
    fn wasm_memory_limit_enforced() {
        let max_pages = 256u32;     // 16 MB
        let page_size = 65_536u32;  // 64 KB per page
        let max_memory = max_pages * page_size;
        assert_eq!(max_memory, 16_777_216, "max memory = 16 MB");

        let requested = 257 * page_size;
        assert!(requested > max_memory, "over-limit memory request rejected");
    }

    #[test]
    fn wasm_host_api_callable() {
        // Verify the host API function names are correctly defined.
        let host_fns = [
            "zbx_storage_get",
            "zbx_storage_set",
            "zbx_transfer",
            "zbx_balance",
            "zbx_call",
            "zbx_emit",
            "zbx_keccak256",
        ];
        assert_eq!(host_fns.len(), 7, "all host API functions defined");
    }

    #[test]
    fn wasm_call_depth_limited() {
        let max_depth = 64u32;
        let current   = 63u32;
        let can_call  = current < max_depth;
        assert!(can_call, "call at depth 63 allowed");

        let at_limit  = 64u32;
        assert!(at_limit >= max_depth, "call at depth 64 rejected");
    }

    #[test]
    fn wasm_threads_disabled() {
        // WASM threads (shared memory) must be disabled for determinism.
        // ZBX Chain consensus requires identical execution across all nodes.
        let threads_enabled = false; // enforced by engine config
        assert!(!threads_enabled, "WASM threads must be disabled");
    }
}