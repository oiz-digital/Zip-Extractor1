use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("connection refused: {0}")]
    ConnectionRefused(String),

    #[error("peer {0} not found")]
    PeerNotFound(String),

    #[error("message decode error: {0}")]
    MessageDecode(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("handshake failed with {0}: {1}")]
    HandshakeFailed(String, String),

    #[error("max peers ({0}) reached")]
    MaxPeers(usize),

    #[error("invalid peer id: {0}")]
    InvalidPeerId(String),
}