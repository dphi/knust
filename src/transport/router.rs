//! Frame dispatch router for the single receive loop.
//!
//! A single receive loop parses each KNX/IP frame once and hands it to the
//! router. The router correlates the frame to a one-shot waiter registered for
//! that service type (request/response flows such as the heartbeat's
//! `ConnectionState_Request` -> `ConnectionState_Response`). Frames with no waiter
//! are returned to the caller to be handled as persistent/unsolicited frames
//! (e.g. `TunnellingRequest`, `DisconnectRequest`).
//!
//! This mirrors the callback-registry model used by the Python knx
//! `KNXIPTransport.handle_knxipframe`, but uses typed one-shot channels for
//! response correlation instead of a side-channel notification.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::oneshot;

use crate::protocol::knxip::{KnxIpFrame, ServiceType};

/// Routes parsed frames to one-shot response waiters keyed by service type.
#[derive(Default)]
pub struct FrameRouter {
    waiters: Mutex<HashMap<u16, Vec<oneshot::Sender<KnxIpFrame>>>>,
}

impl FrameRouter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a one-shot waiter for the next frame of `service_type`.
    ///
    /// The returned receiver resolves when a matching frame is dispatched.
    /// Callers should `await` it with a timeout; if the receiver is dropped
    /// (timeout), the stale sender is skipped on the next dispatch.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn register(&self, service_type: ServiceType) -> oneshot::Receiver<KnxIpFrame> {
        let (tx, rx) = oneshot::channel();
        self.waiters
            .lock()
            .unwrap()
            .entry(service_type as u16)
            .or_default()
            .push(tx);
        rx
    }

    /// Dispatch a frame to a waiting one-shot, if any.
    ///
    /// Returns `None` if the frame was delivered to a waiter (consumed), or
    /// `Some(frame)` if there was no live waiter and the caller should handle
    /// the frame itself.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn dispatch(&self, mut frame: KnxIpFrame) -> Option<KnxIpFrame> {
        let key = frame.header.service_type as u16;
        let mut waiters = self.waiters.lock().unwrap();
        if let Some(queue) = waiters.get_mut(&key) {
            // Deliver to the oldest live waiter (FIFO). Skip any whose receiver
            // has already been dropped (e.g. timed out).
            while !queue.is_empty() {
                let tx = queue.remove(0);
                match tx.send(frame) {
                    Ok(()) => {
                        if queue.is_empty() {
                            waiters.remove(&key);
                        }
                        return None;
                    }
                    Err(returned) => {
                        frame = returned;
                    }
                }
            }
            waiters.remove(&key);
        }
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::knxip::{KnxIpFrame, ServiceType};

    fn frame(service_type: ServiceType) -> KnxIpFrame {
        KnxIpFrame::new(service_type, vec![0x00, 0x00])
    }

    #[tokio::test]
    async fn dispatch_delivers_to_registered_waiter() {
        let router = FrameRouter::new();
        let rx = router.register(ServiceType::ConnectionstateResponse);

        let consumed = router.dispatch(frame(ServiceType::ConnectionstateResponse));
        assert!(consumed.is_none(), "frame should be consumed by the waiter");

        let received = rx.await.expect("waiter should receive the frame");
        assert_eq!(
            received.header.service_type,
            ServiceType::ConnectionstateResponse
        );
    }

    #[test]
    fn dispatch_returns_frame_when_no_waiter() {
        let router = FrameRouter::new();
        let out = router.dispatch(frame(ServiceType::TunnellingRequest));
        assert!(
            out.is_some(),
            "unmatched frame should be returned to caller"
        );
    }

    #[test]
    fn dispatch_skips_dropped_waiter() {
        let router = FrameRouter::new();
        let rx = router.register(ServiceType::ConnectionstateResponse);
        drop(rx); // simulate a timed-out waiter

        let out = router.dispatch(frame(ServiceType::ConnectionstateResponse));
        assert!(
            out.is_some(),
            "frame should be returned when the only waiter was dropped"
        );
    }
}
