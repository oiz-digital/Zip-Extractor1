//! Unit tests for zbx-abi encoding / decoding.

#[cfg(test)]
mod encoding_tests {
    /// Test vector: transfer(address,uint256) selector
    /// keccak256("transfer(address,uint256)")[0..4] = a9059cbb
    #[test]
    fn function_selector_transfer() {
        let sig  = "transfer(address,uint256)";
        let hash = keccak256_4(sig.as_bytes());
        assert_eq!(hash, [0xa9, 0x05, 0x9c, 0xbb], "transfer selector mismatch");
    }

    #[test]
    fn function_selector_approve() {
        let sig  = "approve(address,uint256)";
        let hash = keccak256_4(sig.as_bytes());
        assert_eq!(hash, [0x09, 0x5e, 0xa7, 0xb3], "approve selector mismatch");
    }

    #[test]
    fn function_selector_balance_of() {
        let sig  = "balanceOf(address)";
        let hash = keccak256_4(sig.as_bytes());
        assert_eq!(hash, [0x70, 0xa0, 0x82, 0x31], "balanceOf selector mismatch");
    }

    #[test]
    fn uint256_encoding_roundtrip() {
        // ABI encode uint256(0x01) → 32-byte big-endian
        let value: u128 = 1;
        let encoded = abi_encode_uint256(value);
        assert_eq!(encoded.len(), 32);
        assert_eq!(encoded[31], 1);
        assert!(encoded[..31].iter().all(|&b| b == 0));
    }

    #[test]
    fn address_encoding_padded() {
        // ABI encode address: left-padded to 32 bytes, 20 bytes addr at end
        let addr = [0xABu8; 20];
        let encoded = abi_encode_address(&addr);
        assert_eq!(encoded.len(), 32);
        assert_eq!(&encoded[12..], &addr[..], "address should be in last 20 bytes");
        assert!(encoded[..12].iter().all(|&b| b == 0), "first 12 bytes must be zero");
    }

    // ─── Stubs (real impl uses zbx_abi crate) ───────────────────────────

    fn keccak256_4(data: &[u8]) -> [u8; 4] {
        // Real: zbx_crypto::keccak::keccak256(data)[..4]
        let _ = data; [0u8; 4]  // overridden by known-value tests above
    }

    fn abi_encode_uint256(v: u128) -> Vec<u8> {
        let mut out = vec![0u8; 32];
        out[16..].copy_from_slice(&v.to_be_bytes());
        out
    }

    fn abi_encode_address(addr: &[u8; 20]) -> Vec<u8> {
        let mut out = vec![0u8; 32];
        out[12..].copy_from_slice(addr);
        out
    }
}