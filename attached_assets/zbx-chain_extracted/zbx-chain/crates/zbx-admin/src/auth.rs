//! Admin authentication: bearer-token + HMAC-SHA256 request signing.
//!
//! Admin endpoints are protected by a shared secret configured in
//! `admin.secret_file` (path to a file containing a 32-byte hex secret).
//! Each request must include:
//!   `Authorization: Bearer <token>`
//! where `token = hex(HMAC-SHA256(secret, request_body || unix_timestamp))`.

use crate::error::AdminError;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Token validity window (±30 seconds).
pub const TOKEN_WINDOW_SECS: u64 = 30;

/// Admin role levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdminRole {
    ReadOnly  = 0,  // view-only operations
    Operator  = 1,  // mempool / peer management
    Validator = 2,  // validator set changes
    SuperUser = 3,  // config reload, emergency stop, backup
}

impl AdminRole {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "readonly"  => Some(AdminRole::ReadOnly),
            "operator"  => Some(AdminRole::Operator),
            "validator" => Some(AdminRole::Validator),
            "superuser" => Some(AdminRole::SuperUser),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            AdminRole::ReadOnly  => "readonly",
            AdminRole::Operator  => "operator",
            AdminRole::Validator => "validator",
            AdminRole::SuperUser => "superuser",
        }
    }
}

/// An authenticated admin session.
#[derive(Debug, Clone)]
pub struct AdminSession {
    pub token_id:   String,
    pub role:       AdminRole,
    pub issued_at:  u64,
    pub expires_at: u64,
    pub client_ip:  Option<String>,
}

impl AdminSession {
    pub fn is_expired(&self) -> bool {
        unix_now() > self.expires_at
    }
    pub fn has_role(&self, required: AdminRole) -> bool {
        self.role >= required
    }
}

/// Verify a bearer token against the node secret and current timestamp.
pub fn verify_token(
    token:      &str,
    secret:     &[u8],
    body:       &[u8],
) -> Result<(), AdminError> {
    // Token format: "<timestamp_hex>.<hmac_hex>"
    let parts: Vec<&str> = token.splitn(2, '.').collect();
    if parts.len() != 2 {
        return Err(AdminError::AuthFailed("malformed token".into()));
    }
    let ts_bytes = hex::decode(parts[0])
        .map_err(|_| AdminError::AuthFailed("invalid timestamp encoding".into()))?;
    if ts_bytes.len() != 8 {
        return Err(AdminError::AuthFailed("timestamp must be 8 bytes".into()));
    }
    let mut ts_arr = [0u8; 8];
    ts_arr.copy_from_slice(&ts_bytes);
    let ts = u64::from_be_bytes(ts_arr);
    let now = unix_now();
    if ts.abs_diff(now) > TOKEN_WINDOW_SECS {
        return Err(AdminError::AuthFailed(format!(
            "token timestamp drift too large: |now({}) - ts({})| > {}s",
            now, ts, TOKEN_WINDOW_SECS
        )));
    }
    // Verify HMAC.
    let expected = compute_hmac(secret, &ts.to_be_bytes(), body);
    let provided  = hex::decode(parts[1])
        .map_err(|_| AdminError::AuthFailed("invalid HMAC encoding".into()))?;
    if !constant_time_eq(&expected, &provided) {
        return Err(AdminError::AuthFailed("HMAC verification failed".into()));
    }
    Ok(())
}

/// Generate a bearer token for a given body at the current time.
pub fn generate_token(secret: &[u8], body: &[u8]) -> String {
    let ts    = unix_now().to_be_bytes();
    let hmac  = compute_hmac(secret, &ts, body);
    format!("{}.{}", hex::encode(ts), hex::encode(hmac))
}

fn compute_hmac(secret: &[u8], ts: &[u8], body: &[u8]) -> Vec<u8> {
    // Real HMAC-SHA256 (RFC 2104). The previous SHA3 concat construction was
    // not an HMAC at all and would not interoperate with any standard HMAC
    // verifier — see AUDIT_2026-04-30.md C-07.
    let mut mac = HmacSha256::new_from_slice(secret)
        .expect("HMAC accepts any key length");
    mac.update(ts);
    mac.update(body);
    mac.finalize().into_bytes().to_vec()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Use the `subtle` crate's audited constant-time comparison rather than
    // hand-rolled byte XOR. Behaviour is identical, but `subtle` carries
    // explicit guarantees against compiler short-circuiting.
    a.ct_eq(b).into()
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}