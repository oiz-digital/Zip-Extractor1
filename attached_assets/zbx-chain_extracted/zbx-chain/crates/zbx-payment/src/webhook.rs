//! Webhook event builder — structured payloads for off-chain integrations.
//!
//! When ZbxPaymentGateway emits on-chain events (InvoicePaid, Refunded, etc.),
//! an off-chain indexer can decode them and deliver structured JSON webhooks
//! to merchant endpoints.  This module defines the payload schemas.

use serde::{Deserialize, Serialize};

/// Top-level webhook event envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    /// ZBX Chain event type.
    pub event_type:   WebhookEventType,
    /// Chain ID (8989 mainnet / 8990 testnet).
    pub chain_id:     u64,
    /// Block number of the on-chain event.
    pub block_number: u64,
    /// Transaction hash.
    pub tx_hash:      String,
    /// Event-specific payload.
    pub payload:      WebhookPayload,
    /// Unix timestamp of the webhook delivery attempt.
    pub delivered_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WebhookEventType {
    InvoiceCreated,
    InvoicePaid,
    InvoiceCancelled,
    Refunded,
    MerchantRegistered,
    MerchantWithdrawal,
}

/// Discriminated union of event payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebhookPayload {
    InvoiceCreated(InvoiceCreatedPayload),
    InvoicePaid(InvoicePaidPayload),
    InvoiceCancelled(InvoiceCancelledPayload),
    Refunded(RefundedPayload),
    MerchantRegistered(MerchantRegisteredPayload),
    MerchantWithdrawal(MerchantWithdrawalPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceCreatedPayload {
    pub invoice_id:  String,
    pub merchant_id: String,
    pub order_id:    String,
    pub token:       String,
    pub amount:      String,
    pub expires_at:  u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoicePaidPayload {
    pub invoice_id:   String,
    pub merchant_id:  String,
    pub payer:        String,
    pub input_token:  String,
    pub input_amount: String,
    pub amount_paid:  String,
    pub fee:          String,
    /// True if the invoice is now fully paid.
    pub fully_paid:   bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceCancelledPayload {
    pub invoice_id:  String,
    pub merchant_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefundedPayload {
    pub invoice_id: String,
    pub payer:      String,
    pub amount:     String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerchantRegisteredPayload {
    pub merchant_id:    String,
    pub owner:          String,
    pub name:           String,
    pub payout_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerchantWithdrawalPayload {
    pub merchant_id: String,
    pub token:       String,
    pub amount:      String,
    pub payout_to:   String,
}

/// Sign and deliver a webhook to a merchant URL.
///
/// In production this runs as an async indexer service.  The HMAC-SHA256
/// signature over the JSON body allows merchants to verify authenticity.
pub struct WebhookDelivery {
    pub url:       String,
    pub secret:    Vec<u8>,
}

impl WebhookDelivery {
    pub fn new(url: String, secret: Vec<u8>) -> Self {
        Self { url, secret }
    }

    /// Compute HMAC-SHA256 signature of the body.
    pub fn sign(&self, body: &[u8]) -> Vec<u8> {
        // Placeholder: real implementation uses ring or hmac crate.
        // Returns keccak256(secret || body) as a simplified stand-in.
        let mut data = self.secret.clone();
        data.extend_from_slice(body);
        zbx_crypto::keccak256(&data).as_bytes().to_vec()
    }

    /// Build a signed HTTP request body (JSON).
    pub fn build_request(&self, event: &WebhookEvent) -> Result<(Vec<u8>, String), String> {
        let body = serde_json::to_vec(event)
            .map_err(|e| e.to_string())?;
        let sig  = hex::encode(self.sign(&body));
        Ok((body, sig))
    }
}
