//! Invoice lifecycle — off-chain mirror of ZbxPaymentGateway invoice state.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use zbx_types::address::Address;

/// Invoice status, mirroring the Solidity enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvoiceStatus {
    Pending,
    Paid,
    Cancelled,
}

/// An invoice (payment request).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub invoice_id:       [u8; 32],
    pub merchant_id:      [u8; 32],
    /// Off-chain order reference.
    pub order_id:         [u8; 32],
    /// Required payment token.
    pub token:            Address,
    /// Required amount (in token's raw decimals).
    pub amount:           u128,
    /// Amount paid so far (supports partial payments).
    pub amount_paid:      u128,
    /// Unix timestamp when the invoice expires.
    pub expires_at:       u64,
    pub status:           InvoiceStatus,
    /// The address that last made a payment.
    pub last_payer:       Option<Address>,
    /// Unix timestamp when the invoice was paid (for refund window).
    pub paid_at:          Option<u64>,
}

impl Invoice {
    /// Whether more payment is needed.
    pub fn is_fully_paid(&self) -> bool {
        self.amount_paid >= self.amount
    }

    /// Remaining amount needed to fully pay this invoice.
    pub fn remaining(&self) -> u128 {
        self.amount.saturating_sub(self.amount_paid)
    }

    /// Whether the invoice is still open and not expired.
    pub fn is_payable(&self, now: u64) -> bool {
        self.status == InvoiceStatus::Pending
            && now <= self.expires_at
            && !self.is_fully_paid()
    }
}

/// In-memory invoice store.
#[derive(Debug, Default)]
pub struct InvoiceStore {
    invoices:    HashMap<[u8; 32], Invoice>,
    by_merchant: HashMap<[u8; 32], Vec<[u8; 32]>>,
    by_order:    HashMap<[u8; 32], [u8; 32]>,  // order_id → invoice_id
}

impl InvoiceStore {
    pub fn new() -> Self { Self::default() }

    pub fn insert(&mut self, inv: Invoice) {
        self.by_merchant.entry(inv.merchant_id).or_default().push(inv.invoice_id);
        self.by_order.insert(inv.order_id, inv.invoice_id);
        self.invoices.insert(inv.invoice_id, inv);
    }

    pub fn get(&self, id: &[u8; 32]) -> Option<&Invoice> {
        self.invoices.get(id)
    }

    pub fn get_mut(&mut self, id: &[u8; 32]) -> Option<&mut Invoice> {
        self.invoices.get_mut(id)
    }

    /// Find invoice by off-chain orderId.
    pub fn by_order_id(&self, order_id: &[u8; 32]) -> Option<&Invoice> {
        self.by_order.get(order_id)
            .and_then(|id| self.invoices.get(id))
    }

    /// All invoices for a merchant.
    pub fn for_merchant(&self, merchant_id: &[u8; 32]) -> Vec<&Invoice> {
        self.by_merchant.get(merchant_id)
            .map(|ids| ids.iter().filter_map(|id| self.invoices.get(id)).collect())
            .unwrap_or_default()
    }

    /// Pending invoices for a merchant that are not yet expired.
    pub fn open_for_merchant(&self, merchant_id: &[u8; 32], now: u64) -> Vec<&Invoice> {
        self.for_merchant(merchant_id)
            .into_iter()
            .filter(|inv| inv.is_payable(now))
            .collect()
    }

    /// Record a payment event (updates amount_paid and status).
    pub fn record_payment(
        &mut self,
        invoice_id: &[u8; 32],
        amount:     u128,
        payer:      Address,
        now:        u64,
    ) {
        if let Some(inv) = self.invoices.get_mut(invoice_id) {
            inv.amount_paid  += amount;
            inv.last_payer    = Some(payer);
            if inv.is_fully_paid() {
                inv.status  = InvoiceStatus::Paid;
                inv.paid_at = Some(now);
            }
        }
    }

    pub fn cancel(&mut self, invoice_id: &[u8; 32]) {
        if let Some(inv) = self.invoices.get_mut(invoice_id) {
            inv.status = InvoiceStatus::Cancelled;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address { [b; 20] }
    fn id(b: u8) -> [u8; 32] { [b; 32] }

    fn make_invoice(amount: u128, expires_at: u64) -> Invoice {
        Invoice {
            invoice_id:  id(1),
            merchant_id: id(2),
            order_id:    id(3),
            token:       addr(0xAA),
            amount,
            amount_paid: 0,
            expires_at,
            status:      InvoiceStatus::Pending,
            last_payer:  None,
            paid_at:     None,
        }
    }

    #[test]
    fn new_invoice_is_payable() {
        let inv = make_invoice(1_000, 9999);
        assert!(inv.is_payable(1000));
        assert!(!inv.is_fully_paid());
        assert_eq!(inv.remaining(), 1_000);
    }

    #[test]
    fn expired_invoice_not_payable() {
        let inv = make_invoice(1_000, 500);
        assert!(!inv.is_payable(501));
    }

    #[test]
    fn partial_payment_tracked() {
        let inv = make_invoice(1_000, 9999);
        assert_eq!(inv.remaining(), 1_000);
        let mut inv2 = inv;
        inv2.amount_paid = 400;
        assert_eq!(inv2.remaining(), 600);
        assert!(!inv2.is_fully_paid());
    }

    #[test]
    fn full_payment_marks_paid() {
        let mut store = InvoiceStore::new();
        let inv = make_invoice(500, 9999);
        let iid = inv.invoice_id;
        store.insert(inv);
        store.record_payment(&iid, 500, addr(5), 1000);
        let updated = store.get(&iid).unwrap();
        assert_eq!(updated.status, InvoiceStatus::Paid);
        assert_eq!(updated.paid_at, Some(1000));
        assert_eq!(updated.last_payer, Some(addr(5)));
    }

    #[test]
    fn cancel_sets_cancelled_status() {
        let mut store = InvoiceStore::new();
        let inv = make_invoice(500, 9999);
        let iid = inv.invoice_id;
        store.insert(inv);
        store.cancel(&iid);
        assert_eq!(store.get(&iid).unwrap().status, InvoiceStatus::Cancelled);
    }

    #[test]
    fn by_order_id_lookup() {
        let mut store = InvoiceStore::new();
        let inv = make_invoice(100, 9999);
        let oid = inv.order_id;
        store.insert(inv);
        assert!(store.by_order_id(&oid).is_some());
        assert!(store.by_order_id(&id(99)).is_none());
    }

    #[test]
    fn open_for_merchant_filters_expired_and_cancelled() {
        let mut store = InvoiceStore::new();
        let mid = id(2);

        let mut inv_open = make_invoice(100, 9999);
        inv_open.invoice_id = id(10);
        inv_open.order_id   = id(10);

        let mut inv_expired = make_invoice(100, 1);
        inv_expired.invoice_id = id(11);
        inv_expired.order_id   = id(11);

        let mut inv_cancelled = make_invoice(100, 9999);
        inv_cancelled.invoice_id = id(12);
        inv_cancelled.order_id   = id(12);
        inv_cancelled.status     = InvoiceStatus::Cancelled;

        store.insert(inv_open);
        store.insert(inv_expired);
        store.insert(inv_cancelled);

        let open = store.open_for_merchant(&mid, 1000);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].invoice_id, id(10));
    }
}
