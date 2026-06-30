use thiserror::Error;

#[derive(Debug, Error)]
pub enum StateError {
    #[error("account not found: {0}")]
    AccountNotFound(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("trie error: {0}")]
    Trie(String),

    #[error("code not found for hash: {0}")]
    CodeNotFound(String),
}