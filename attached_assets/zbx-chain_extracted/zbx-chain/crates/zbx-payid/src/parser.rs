//! Pay ID string parser — UPI style: ali@zbx

use crate::error::PayIdError;

/// Parsed Pay ID components.
#[derive(Debug, Clone, PartialEq)]
pub struct PayIdParts {
    /// Bare name without suffix (e.g., "ali" from "ali@zbx").
    pub name: String,
    /// Network handle (always "zbx" for ZBX chain).
    pub handle: String,
    /// Whether this is a sub-ID (e.g., "shop.ali@zbx").
    pub is_sub_id: bool,
    /// Parent name for sub-IDs (e.g., "ali" from "shop.ali@zbx").
    pub parent: Option<String>,
    /// Canonical display form: "ali@zbx".
    pub canonical: String,
}

/// Parse a Pay ID string.
///
/// Accepted formats:
/// - `ali`          → name="ali",      handle="zbx"
/// - `ali@zbx`      → name="ali",      handle="zbx"
/// - `ALI@ZBX`      → normalized to "ali@zbx"
/// - `shop.ali@zbx` → sub-ID: name="shop.ali", parent="ali"
/// - `shop.ali`     → sub-ID (handle inferred as "zbx")
/// - `0x742d...`    → returns Err (use direct address, not Pay ID)
pub fn parse_pay_id(input: &str) -> Result<PayIdParts, PayIdError> {
    let input = input.trim().to_lowercase();

    // Reject raw 0x addresses
    if input.starts_with("0x") {
        return Err(PayIdError::InvalidFormat(
            "0x addresses are not Pay IDs — use the resolver directly".into()
        ));
    }

    // Split on '@'
    let (name_part, handle) = if let Some(pos) = input.find('@') {
        let name   = input[..pos].to_string();
        let handle = input[pos+1..].to_string();
        if handle.is_empty() {
            return Err(PayIdError::InvalidFormat("handle after @ cannot be empty".into()));
        }
        (name, handle)
    } else {
        // No '@' — assume @zbx
        (input.clone(), "zbx".to_string())
    };

    // Check for sub-ID (dot in name part)
    let parts: Vec<&str> = name_part.splitn(2, '.').collect();
    let (is_sub_id, parent) = if parts.len() == 2 {
        let sub    = parts[0];
        let parent = parts[1];
        validate_name_part(sub)?;
        validate_name_part(parent)?;
        (true, Some(parent.to_string()))
    } else {
        validate_name_part(&name_part)?;
        (false, None)
    };

    let canonical = format!("{}@{}", name_part, handle);

    Ok(PayIdParts {
        name: name_part,
        handle,
        is_sub_id,
        parent,
        canonical,
    })
}

/// Validate the name portion (before @).
fn validate_name_part(name: &str) -> Result<(), PayIdError> {
    if name.len() < 3 || name.len() > 32 {
        return Err(PayIdError::InvalidFormat(
            format!("'{}' must be 3-32 characters", name)
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(PayIdError::InvalidFormat(
            format!("'{}' cannot start or end with hyphen", name)
        ));
    }
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return Err(PayIdError::InvalidFormat(
                format!("invalid character '{}' in Pay ID name — only a-z, 0-9 and hyphen allowed", c)
            ));
        }
    }
    Ok(())
}

/// Validate the display name (full human name like "Salman Tyagi").
///
/// Rules:
/// - MANDATORY — empty string is rejected
/// - 2–64 characters
/// - Only Unicode letters, spaces, dots, hyphens, and apostrophes
/// - Cannot start or end with a space
///
/// Valid   : "Salman Tyagi", "Ali", "María López", "O'Brien", "Jean-Pierre"
/// Invalid : "", "  ", "Salman123", "ZBX@user"
pub fn validate_display_name(name: &str) -> Result<(), PayIdError> {
    let trimmed = name.trim();

    if trimmed.is_empty() {
        return Err(PayIdError::DisplayNameRequired);
    }
    if trimmed.len() < 2 {
        return Err(PayIdError::DisplayNameInvalid(
            "display name must be at least 2 characters (e.g. 'Salman Tyagi')".into()
        ));
    }
    if trimmed.len() > 64 {
        return Err(PayIdError::DisplayNameInvalid(
            "display name must be 64 characters or fewer".into()
        ));
    }
    for c in trimmed.chars() {
        if !c.is_alphabetic() && c != ' ' && c != '-' && c != '\'' && c != '.' {
            return Err(PayIdError::DisplayNameInvalid(
                format!("invalid character '{}' — only letters, spaces, hyphens, dots and apostrophes allowed", c)
            ));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_name_at_zbx() {
        let p = parse_pay_id("ali@zbx").unwrap();
        assert_eq!(p.name, "ali");
        assert_eq!(p.handle, "zbx");
        assert!(!p.is_sub_id);
        assert_eq!(p.canonical, "ali@zbx");
    }

    #[test]
    fn parse_name_without_at_assumes_zbx() {
        let p = parse_pay_id("salman").unwrap();
        assert_eq!(p.handle, "zbx");
    }

    #[test]
    fn parse_uppercase_normalized() {
        let p = parse_pay_id("ALI@ZBX").unwrap();
        assert_eq!(p.canonical, "ali@zbx");
    }

    #[test]
    fn parse_sub_id() {
        let p = parse_pay_id("shop.ali@zbx").unwrap();
        assert!(p.is_sub_id);
        assert_eq!(p.parent.as_deref(), Some("ali"));
    }

    #[test]
    fn parse_sub_id_without_handle() {
        let p = parse_pay_id("shop.ali").unwrap();
        assert!(p.is_sub_id);
        assert_eq!(p.handle, "zbx");
    }

    #[test]
    fn reject_0x_address() {
        assert!(parse_pay_id("0x742d35Cc6634C").is_err());
    }

    #[test]
    fn reject_empty_handle() {
        assert!(parse_pay_id("ali@").is_err());
    }

    #[test]
    fn reject_too_short_name() {
        assert!(parse_pay_id("ab@zbx").is_err());
    }

    #[test]
    fn reject_special_chars() {
        assert!(parse_pay_id("ali!@zbx").is_err());
    }

    #[test]
    fn reject_leading_hyphen() {
        assert!(parse_pay_id("-ali@zbx").is_err());
    }

    #[test]
    fn validate_display_name_ok() {
        assert!(validate_display_name("Salman Tyagi").is_ok());
        assert!(validate_display_name("María López").is_ok());
        assert!(validate_display_name("O'Brien").is_ok());
    }

    #[test]
    fn validate_display_name_rejects_empty() {
        assert!(validate_display_name("").is_err());
        assert!(validate_display_name("  ").is_err());
    }

    #[test]
    fn validate_display_name_rejects_digits() {
        assert!(validate_display_name("Salman123").is_err());
    }
}
