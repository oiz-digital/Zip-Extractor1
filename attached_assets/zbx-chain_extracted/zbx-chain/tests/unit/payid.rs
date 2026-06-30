//! Unit tests for zbx-payid (UPI-style @zbx format).

#[cfg(test)]
mod payid_tests {
    use zbx_payid::parser::parse_pay_id;
    use zbx_payid::registry::PayIdRegistry;
    use zbx_payid::error::PayIdError;
    use zbx_payid::format_pay_id;
    use zbx_types::CHAIN_ID_MAINNET;

    // ── Parser: valid formats ─────────────────────────────────────────────

    #[test]
    fn test_parse_bare_name() {
        let p = parse_pay_id("ali").unwrap();
        assert_eq!(p.name, "ali");
        assert_eq!(p.handle, "zbx");
        assert_eq!(p.canonical, "ali@zbx");
        assert!(!p.is_sub_id);
    }

    #[test]
    fn test_parse_full_upi_style() {
        let p = parse_pay_id("ali@zbx").unwrap();
        assert_eq!(p.name, "ali");
        assert_eq!(p.handle, "zbx");
        assert_eq!(p.canonical, "ali@zbx");
    }

    #[test]
    fn test_parse_uppercase_normalized() {
        let p = parse_pay_id("ALI@ZBX").unwrap();
        assert_eq!(p.canonical, "ali@zbx");
    }

    #[test]
    fn test_parse_sub_id() {
        let p = parse_pay_id("shop.ali@zbx").unwrap();
        assert!(p.is_sub_id);
        assert_eq!(p.name, "shop.ali");
        assert_eq!(p.parent, Some("ali".to_string()));
        assert_eq!(p.canonical, "shop.ali@zbx");
    }

    #[test]
    fn test_parse_sub_id_bare() {
        let p = parse_pay_id("shop.ali").unwrap();
        assert!(p.is_sub_id);
        assert_eq!(p.canonical, "shop.ali@zbx");
    }

    #[test]
    fn test_parse_with_hyphen() {
        let p = parse_pay_id("my-shop@zbx").unwrap();
        assert_eq!(p.name, "my-shop");
    }

    #[test]
    fn test_parse_with_numbers() {
        let p = parse_pay_id("trader123@zbx").unwrap();
        assert_eq!(p.name, "trader123");
    }

    // ── Parser: invalid formats ───────────────────────────────────────────

    #[test]
    fn test_parse_raw_address_rejected() {
        let result = parse_pay_id("0x742d35Cc6634C0532925a3b844Bc454e4438f44e");
        assert!(matches!(result, Err(PayIdError::InvalidFormat(_))));
    }

    #[test]
    fn test_parse_too_short() {
        assert!(parse_pay_id("ab@zbx").is_err());
    }

    #[test]
    fn test_parse_too_long() {
        let long = format!("{}@zbx", "a".repeat(33));
        assert!(parse_pay_id(&long).is_err());
    }

    #[test]
    fn test_parse_leading_hyphen() {
        assert!(parse_pay_id("-ali@zbx").is_err());
    }

    #[test]
    fn test_parse_trailing_hyphen() {
        assert!(parse_pay_id("ali-@zbx").is_err());
    }

    #[test]
    fn test_parse_space_invalid() {
        assert!(parse_pay_id("ali baba@zbx").is_err());
    }

    #[test]
    fn test_parse_empty_handle() {
        assert!(parse_pay_id("ali@").is_err());
    }

    // ── format_pay_id helper ──────────────────────────────────────────────

    #[test]
    fn test_format_pay_id() {
        assert_eq!(format_pay_id("ali"), "ali@zbx");
        assert_eq!(format_pay_id("ALI"), "ali@zbx");
        assert_eq!(format_pay_id("MyShop"), "myshop@zbx");
    }

    // ── Registry cache ────────────────────────────────────────────────────

    #[test]
    fn test_registry_cache_miss() {
        let reg = PayIdRegistry::new();
        assert!(reg.get("ali@zbx").is_none());
    }

    #[test]
    fn test_registry_insert_get() {
        use zbx_payid::resolver::ResolvedPayId;
        let reg = PayIdRegistry::new();
        let resolved = ResolvedPayId {
            input: "ali@zbx".to_string(),
            pay_id: "ali@zbx".to_string(),
            address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            is_smart_wallet: true,
            chain_id: CHAIN_ID_MAINNET,
        };
        reg.insert("ali@zbx".to_string(), resolved.clone());
        let cached = reg.get("ali@zbx").unwrap();
        assert_eq!(cached.pay_id, "ali@zbx");
        assert_eq!(cached.address, resolved.address);
    }

    #[test]
    fn test_registry_invalidate() {
        use zbx_payid::resolver::ResolvedPayId;
        let reg = PayIdRegistry::new();
        reg.insert("ali@zbx".to_string(), ResolvedPayId {
            input: "ali@zbx".to_string(),
            pay_id: "ali@zbx".to_string(),
            address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            is_smart_wallet: false,
            chain_id: CHAIN_ID_MAINNET,
        });
        reg.invalidate("ali@zbx");
        assert!(reg.get("ali@zbx").is_none());
    }

    #[test]
    fn test_different_handles_independent() {
        // ali@zbx and shop.ali@zbx are different entries
        let p1 = parse_pay_id("ali@zbx").unwrap();
        let p2 = parse_pay_id("shop.ali@zbx").unwrap();
        assert_ne!(p1.canonical, p2.canonical);
        assert_ne!(p1.name, p2.name);
    }
}