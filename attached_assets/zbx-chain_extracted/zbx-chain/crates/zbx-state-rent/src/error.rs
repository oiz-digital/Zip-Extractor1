//! State rent error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RentError {
    #[error("account is not hibernated")]
    AccountNotHibernated,
    #[error("insufficient revival payment: required {required}, provided {provided}")]
    InsufficientRevivalPayment { required: u128, provided: u128 },
    #[error("account has expired and cannot be revived")]
    AccountExpired,
    #[error("rent calculation overflow")]
    Overflow,
}
