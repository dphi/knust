//! Transport abstraction for sending/receiving complete KNX/IP frames.
//!
//! The KNX-layer tunnel state machine is identical over UDP and TCP; only the
//! byte transport differs:
//!
//! * **UDP** — one datagram is exactly one KNX/IP frame.
//! * **TCP** — a byte stream that must be re-framed using the 6-byte KNX/IP
//!   header's `total_length` field (bytes 4..6).
//!
//! [`FrameTransport`] hides that difference so a single `Tunnel` implementation
//! can drive both.

use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

use crate::error::{Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;

/// The kind of byte transport backing a tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Udp,
    Tcp,
}

/// Sends and receives whole KNX/IP frames over a byte transport.
#[async_trait]
pub trait FrameTransport: Send + Sync {
    /// Send one already-serialized KNX/IP frame.
    async fn send_frame(&self, frame: &[u8]) -> Result<()>;

    /// Receive exactly one complete KNX/IP frame.
    async fn recv_frame(&self) -> Result<Vec<u8>>;

    /// Which transport this is.
    fn kind(&self) -> TransportKind;
}

/// Try to split one complete KNX/IP frame off the front of `buf`.
///
/// Returns `Ok(Some(frame))` and removes it from `buf` when a full frame is
/// buffered, `Ok(None)` when more bytes are needed, or `Err` if the length
/// field is malformed.
pub(crate) fn try_extract_frame(buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>> {
    // KNX/IP header is 6 bytes: header_len, version, service(2), total_length(2).
    if buf.len() < 6 {
        return Ok(None);
    }
    let total_length = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    if total_length < 6 {
        return Err(TransportError::InvalidConfiguration {
            details: format!("Invalid KNX/IP frame length: {total_length}"),
        }
        .into());
    }
    if buf.len() < total_length {
        return Ok(None);
    }
    Ok(Some(buf.drain(..total_length).collect()))
}

/// UDP frame transport over a socket connected to the gateway.
pub struct UdpFrameTransport {
    socket: std::sync::Arc<UdpSocket>,
}

impl UdpFrameTransport {
    /// Bind a local socket and connect it to `gateway_addr`.
    ///
    /// Connecting lets the OS pick a concrete local address and restricts the
    /// socket to the gateway, so `recv` only yields datagrams from it. Returns
    /// the transport and the bound local address (for diagnostics).
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::SocketError`] if binding, connecting, or
    /// querying the local address of the UDP socket fails.
    pub async fn connect(gateway_addr: SocketAddr) -> Result<(Self, SocketAddr)> {
        let socket =
            UdpSocket::bind("0.0.0.0:0")
                .await
                .map_err(|e| TransportError::SocketError {
                    operation: "bind".to_string(),
                    source: e,
                })?;
        socket
            .connect(gateway_addr)
            .await
            .map_err(|e| TransportError::SocketError {
                operation: "connect".to_string(),
                source: e,
            })?;
        let local_addr = socket
            .local_addr()
            .map_err(|e| TransportError::SocketError {
                operation: "get_local_addr".to_string(),
                source: e,
            })?;
        Ok((
            Self {
                socket: std::sync::Arc::new(socket),
            },
            local_addr,
        ))
    }
}

#[async_trait]
impl FrameTransport for UdpFrameTransport {
    async fn send_frame(&self, frame: &[u8]) -> Result<()> {
        self.socket
            .send(frame)
            .await
            .map_err(|e| TransportError::SocketError {
                operation: "send".to_string(),
                source: e,
            })?;
        Ok(())
    }

    async fn recv_frame(&self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; 1024];
        let len = self
            .socket
            .recv(&mut buf)
            .await
            .map_err(|e| TransportError::SocketError {
                operation: "recv".to_string(),
                source: e,
            })?;
        buf.truncate(len);
        Ok(buf)
    }

    fn kind(&self) -> TransportKind {
        TransportKind::Udp
    }
}

struct TcpReader {
    half: OwnedReadHalf,
    buf: Vec<u8>,
}

/// TCP frame transport that re-frames the byte stream into KNX/IP frames.
pub struct TcpFrameTransport {
    write: tokio::sync::Mutex<OwnedWriteHalf>,
    read: tokio::sync::Mutex<TcpReader>,
}

impl TcpFrameTransport {
    /// Connect a TCP stream to the gateway (with timeout) and split it for
    /// concurrent send/recv. Returns the transport and the local address.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Timeout`] if `connect_timeout` elapses first,
    /// [`TransportError::ConnectionFailed`] if the TCP connect fails, or
    /// [`TransportError::SocketError`] if querying the local address fails.
    pub async fn connect(
        gateway_addr: SocketAddr,
        connect_timeout: Duration,
    ) -> Result<(Self, SocketAddr)> {
        let stream = timeout(connect_timeout, TcpStream::connect(gateway_addr))
            .await
            .map_err(|_| TransportError::Timeout {
                timeout_ms: connect_timeout.as_millis() as u64,
            })?
            .map_err(|e| TransportError::ConnectionFailed {
                address: gateway_addr.to_string(),
                source: e,
            })?;
        let local_addr = stream
            .local_addr()
            .map_err(|e| TransportError::SocketError {
                operation: "get_local_addr".to_string(),
                source: e,
            })?;
        let (read_half, write_half) = stream.into_split();
        Ok((
            Self {
                write: tokio::sync::Mutex::new(write_half),
                read: tokio::sync::Mutex::new(TcpReader {
                    half: read_half,
                    buf: Vec::with_capacity(1024),
                }),
            },
            local_addr,
        ))
    }

    /// Wrap an already-accepted TCP stream (server side of a connection),
    /// as opposed to `connect`, which dials out.
    pub fn from_accepted_stream(stream: TcpStream) -> Self {
        let (read_half, write_half) = stream.into_split();
        Self {
            write: tokio::sync::Mutex::new(write_half),
            read: tokio::sync::Mutex::new(TcpReader {
                half: read_half,
                buf: Vec::with_capacity(1024),
            }),
        }
    }
}

#[async_trait]
impl FrameTransport for TcpFrameTransport {
    async fn send_frame(&self, frame: &[u8]) -> Result<()> {
        let mut w = self.write.lock().await;
        w.write_all(frame)
            .await
            .map_err(|e| TransportError::SocketError {
                operation: "tcp_write".to_string(),
                source: e,
            })?;
        w.flush().await.map_err(|e| TransportError::SocketError {
            operation: "tcp_flush".to_string(),
            source: e,
        })?;
        Ok(())
    }

    async fn recv_frame(&self) -> Result<Vec<u8>> {
        let mut r = self.read.lock().await;
        loop {
            if let Some(frame) = try_extract_frame(&mut r.buf)? {
                return Ok(frame);
            }
            let mut tmp = [0u8; 1024];
            let n = r.half.read(&mut tmp).await.map_err(|e| {
                log_transport!(LogLevel::Error, "Failed to read from TCP stream: {}", e);
                TransportError::SocketError {
                    operation: "tcp_read".to_string(),
                    source: e,
                }
            })?;
            if n == 0 {
                return Err(TransportError::ConnectionClosed.into());
            }
            r.buf.extend_from_slice(&tmp[..n]);
        }
    }

    fn kind(&self) -> TransportKind {
        TransportKind::Tcp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal KNX/IP frame of `total_length` bytes (header + filler).
    fn frame(total_length: u16) -> Vec<u8> {
        let mut f = vec![0x06, 0x10, 0x02, 0x06];
        f.extend_from_slice(&total_length.to_be_bytes());
        f.resize(total_length as usize, 0xAB);
        f
    }

    #[test]
    fn extract_returns_none_for_partial_header() {
        let mut buf = vec![0x06, 0x10, 0x02];
        assert!(try_extract_frame(&mut buf).unwrap().is_none());
        assert_eq!(buf.len(), 3, "buffer must be left intact");
    }

    #[test]
    fn extract_returns_none_for_partial_body() {
        let full = frame(10);
        let mut buf = full[..8].to_vec(); // header says 10, only 8 present
        assert!(try_extract_frame(&mut buf).unwrap().is_none());
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn extract_returns_exact_frame() {
        let full = frame(10);
        let mut buf = full.clone();
        let extracted = try_extract_frame(&mut buf).unwrap().expect("a frame");
        assert_eq!(extracted, full);
        assert!(buf.is_empty(), "frame should be consumed");
    }

    #[test]
    fn extract_splits_coalesced_frames() {
        let a = frame(8);
        let b = frame(12);
        let mut buf = a.clone();
        buf.extend_from_slice(&b);

        let first = try_extract_frame(&mut buf).unwrap().expect("first frame");
        assert_eq!(first, a);
        let second = try_extract_frame(&mut buf).unwrap().expect("second frame");
        assert_eq!(second, b);
        assert!(buf.is_empty());
        assert!(try_extract_frame(&mut buf).unwrap().is_none());
    }

    #[test]
    fn extract_rejects_bad_length() {
        // total_length field = 3 (< 6 header) is malformed.
        let mut buf = vec![0x06, 0x10, 0x02, 0x06, 0x00, 0x03, 0xFF];
        assert!(try_extract_frame(&mut buf).is_err());
    }
}
