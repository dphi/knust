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
    waiters: Mutex<HashMap<u16, Vec<Waiter>>>,
}

struct Waiter {
    channel: Option<u8>,
    sequence: Option<u8>,
    tx: oneshot::Sender<KnxIpFrame>,
}

impl FrameRouter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a waiter without channel/sequence filtering.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn register(&self, service_type: ServiceType) -> oneshot::Receiver<KnxIpFrame> {
        self.register_matching(service_type, None, None)
    }

    pub(crate) fn register_ack(
        &self,
        service: ServiceType,
        channel: u8,
        sequence: u8,
    ) -> oneshot::Receiver<KnxIpFrame> {
        self.register_matching(service, Some(channel), Some(sequence))
    }

    pub(crate) fn register_channel_response(
        &self,
        service: ServiceType,
        channel: u8,
    ) -> oneshot::Receiver<KnxIpFrame> {
        self.register_matching(service, Some(channel), None)
    }

    fn register_matching(
        &self,
        service: ServiceType,
        channel: Option<u8>,
        sequence: Option<u8>,
    ) -> oneshot::Receiver<KnxIpFrame> {
        let (tx, rx) = oneshot::channel();
        let mut waiters = self.waiters.lock().unwrap();
        let queue = waiters.entry(service as u16).or_default();
        queue.retain(|waiter| !waiter.tx.is_closed());
        queue.push(Waiter {
            channel,
            sequence,
            tx,
        });
        rx
    }

    /// Deliver a frame only to a waiter whose service, channel and sequence match.
    /// Returns `Some(frame)` if no live waiter accepts it, or `None` if consumed.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn dispatch(&self, mut frame: KnxIpFrame) -> Option<KnxIpFrame> {
        let key = frame.header.service_type as u16;
        let (channel, sequence) = match frame.header.service_type {
            ServiceType::TunnellingAck | ServiceType::DeviceConfigurationAck
                if frame.body.len() == 4 && frame.body[0] == 4 =>
            {
                (Some(frame.body[1]), Some(frame.body[2]))
            }
            ServiceType::ConnectionstateResponse | ServiceType::DisconnectResponse
                if frame.body.len() == 2 =>
            {
                (Some(frame.body[0]), None)
            }
            _ => (None, None),
        };
        let mut waiters = self.waiters.lock().unwrap();
        if let Some(queue) = waiters.get_mut(&key) {
            queue.retain(|waiter| !waiter.tx.is_closed());
            while let Some(index) = queue.iter().position(|waiter| {
                waiter
                    .channel
                    .is_none_or(|expected| channel == Some(expected))
                    && waiter
                        .sequence
                        .is_none_or(|expected| sequence == Some(expected))
            }) {
                let waiter = queue.remove(index);
                match waiter.tx.send(frame) {
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
            if queue.is_empty() {
                waiters.remove(&key);
            }
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

    #[tokio::test]
    async fn acknowledgements_require_channel_sequence_and_valid_structure() {
        for service in [
            ServiceType::TunnellingAck,
            ServiceType::DeviceConfigurationAck,
        ] {
            let router = FrameRouter::new();
            let mut rx = router.register_ack(service, 7, 5);
            for body in [
                vec![4, 8, 5, 0],
                vec![4, 7, 4, 0],
                vec![4, 7, 5],
                vec![3, 7, 5, 0],
                vec![4, 7, 5, 0, 0],
            ] {
                assert!(router.dispatch(KnxIpFrame::new(service, body)).is_some());
                assert!(matches!(
                    rx.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
            }
            assert!(
                router
                    .dispatch(KnxIpFrame::new(service, vec![4, 7, 5, 0]))
                    .is_none()
            );
            assert_eq!(rx.await.unwrap().body, [4, 7, 5, 0]);
        }
    }

    #[tokio::test]
    async fn channel_responses_ignore_other_channels_and_expired_waiters_are_pruned() {
        let router = FrameRouter::new();
        drop(router.register_channel_response(ServiceType::ConnectionstateResponse, 7));
        let mut rx = router.register_channel_response(ServiceType::ConnectionstateResponse, 7);
        assert_eq!(
            router.waiters.lock().unwrap()[&(ServiceType::ConnectionstateResponse as u16)].len(),
            1
        );
        assert!(
            router
                .dispatch(KnxIpFrame::new(
                    ServiceType::ConnectionstateResponse,
                    vec![8, 0]
                ))
                .is_some()
        );
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(
            router
                .dispatch(KnxIpFrame::new(
                    ServiceType::ConnectionstateResponse,
                    vec![7, 0]
                ))
                .is_none()
        );
        assert_eq!(rx.await.unwrap().body, [7, 0]);
    }
}
