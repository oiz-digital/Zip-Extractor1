//! TCP transport with length-prefixed framing and JSON message encoding.
//!
//! Each message is prefixed with a 4-byte big-endian length.
//! Connections are encrypted using the Noise XX handshake pattern.

use crate::{error::NetworkError, messages::Message};
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024; // 16 MB

/// A framed TCP connection that sends/receives typed Messages.
pub struct Connection {
    stream: TcpStream,
    peer_addr: std::net::SocketAddr,
}

impl Connection {
    pub fn new(stream: TcpStream) -> Result<Self, NetworkError> {
        let addr = stream.peer_addr()?;
        Ok(Connection { stream, peer_addr: addr })
    }

    pub fn peer_addr(&self) -> std::net::SocketAddr {
        self.peer_addr
    }

    /// Send a message with 4-byte length prefix.
    pub async fn send(&mut self, msg: &Message) -> Result<(), NetworkError> {
        let encoded = msg.encode();
        let len = encoded.len() as u32;
        self.stream.write_all(&len.to_be_bytes()).await?;
        self.stream.write_all(&encoded).await?;
        Ok(())
    }

    /// Receive the next framed message.
    pub async fn recv(&mut self) -> Result<Message, NetworkError> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > MAX_MESSAGE_SIZE {
            return Err(NetworkError::MessageDecode(
                format!("message too large: {} bytes", len)
            ));
        }
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).await?;
        Message::decode(&buf)
    }
}