//! Point-to-point transport and management procedures for individual KNX devices.

mod management;

pub use management::ScannedInterfaceObject;

use super::{Knx, ReadCallbackGuard};
use crate::application::callbacks::{CallbackHandle, TelegramCallbackFn, TelegramFilter};
use crate::error::{DeviceError, KnxError, Result};
use crate::protocol::{Address, RawGroupAddress, Telegram, TelegramType};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;

struct ConnectionlessResponseCallback {
    tx: tokio::sync::broadcast::Sender<Telegram>,
}

#[async_trait::async_trait]
impl TelegramCallbackFn for ConnectionlessResponseCallback {
    async fn call(&self, telegram: &Telegram) {
        let _ = self.tx.send(telegram.clone());
    }
}

const P2P_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const P2P_MAX_RETRANSMISSIONS: usize = 3;

struct P2pTelegramCallback {
    tx: mpsc::Sender<Telegram>,
}

#[async_trait::async_trait]
impl TelegramCallbackFn for P2pTelegramCallback {
    async fn call(&self, telegram: &Telegram) {
        let _ = self.tx.send(telegram.clone()).await;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum P2pMode {
    Connected,
    Connectionless,
}

pub(super) type ActiveP2pSessions =
    Arc<std::sync::Mutex<std::collections::HashSet<(crate::protocol::IndividualAddress, P2pMode)>>>;

pub(super) struct P2pCallbackRegistration {
    knx: Knx,
    handle: Option<CallbackHandle>,
    active_sessions: ActiveP2pSessions,
    key: (crate::protocol::IndividualAddress, P2pMode),
}

impl Drop for P2pCallbackRegistration {
    fn drop(&mut self) {
        self.active_sessions.lock().unwrap().remove(&self.key);
        if let Some(handle) = self.handle.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let knx = self.knx.clone();
            runtime.spawn(async move {
                knx.unregister_callback(handle).await;
            });
        }
    }
}

/// An established connection-oriented transport link to one individual address.
///
/// Methods require `&mut self`, so requests on one link are serialized and each
/// connected-data sequence has a single owner.
pub struct P2pConnection {
    pub(super) knx: Knx,
    pub(super) destination: crate::protocol::IndividualAddress,
    pub(super) receiver: mpsc::Receiver<Telegram>,
    pub(super) pending: std::collections::VecDeque<Telegram>,
    pub(super) callback: Option<P2pCallbackRegistration>,
    pub(super) send_sequence: u8,
    pub(super) receive_sequence: u8,
    pub(super) closed: bool,
}

/// An address-scoped receive context for unnumbered individual telegrams.
///
/// Dropping the context releases its address reservation. Use [Self::close] to
/// also wait for callback removal.
pub struct P2pConnectionless {
    registration: P2pCallbackRegistration,
    receiver: tokio::sync::broadcast::Receiver<Telegram>,
}

impl P2pConnectionless {
    /// Send an unnumbered APDU after discarding previously buffered replies.
    pub async fn send(&mut self, apdu: Vec<u8>) -> Result<()> {
        if !(2..=u8::MAX as usize).contains(&apdu.len()) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("P2P APDU length must be in 2..=255, got {}", apdu.len()),
            }
            .into());
        }
        let telegram = Telegram::individual_data(self.registration.key.0, apdu);
        self.receiver = self.receiver.resubscribe();
        let knx = &self.registration.knx;
        tokio::select! {
            biased;
            _ = Self::wait_for_shutdown(knx) => Err(crate::error::TransportError::QueueClosed.into()),
            result = knx.send_telegram_acknowledged(&telegram) => result,
        }
    }

    /// Receive the next matching telegram, or `None` when the deadline passes.
    ///
    /// Reuse the same deadline to receive several replies within one window.
    /// Returns a queue error if more than 16 telegrams accumulate unread.
    pub async fn receive(
        &mut self,
        expected_service: Option<[u8; 2]>,
        deadline: tokio::time::Instant,
    ) -> Result<Option<Telegram>> {
        use tokio::sync::broadcast::error::RecvError;

        let knx = &self.registration.knx;
        let received = async {
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return Ok(None);
                }
                match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
                    Ok(Ok(telegram))
                        if expected_service
                            .is_none_or(|service| telegram.payload.starts_with(&service)) =>
                    {
                        return Ok(Some(telegram));
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(RecvError::Lagged(_))) => {
                        return Err(crate::error::TransportError::QueueFull.into());
                    }
                    Ok(Err(RecvError::Closed)) => {
                        return Err(crate::error::TransportError::QueueClosed.into());
                    }
                    Err(_) => return Ok(None),
                }
            }
        };
        tokio::select! {
            biased;
            _ = Self::wait_for_shutdown(knx) => Err(crate::error::TransportError::QueueClosed.into()),
            result = received => result,
        }
    }

    /// Send an APDU and return the first matching reply.
    pub async fn request(
        &mut self,
        apdu: Vec<u8>,
        expected_service: Option<[u8; 2]>,
        timeout: std::time::Duration,
    ) -> Result<Telegram> {
        self.send(apdu).await?;
        self.receive(expected_service, tokio::time::Instant::now() + timeout)
            .await?
            .ok_or_else(|| {
                DeviceError::CommunicationTimeout {
                    device: format!("individual address {}", self.registration.key.0),
                    timeout_ms: timeout.as_millis() as u64,
                }
                .into()
            })
    }

    /// Close the context and unregister its receive callback.
    pub async fn close(mut self) {
        if let Some(handle) = self.registration.handle {
            self.registration.knx.unregister_callback(handle).await;
            self.registration.handle = None;
        }
    }

    async fn wait_for_shutdown(knx: &Knx) {
        let stopping = knx.shutdown_started.notified();
        tokio::pin!(stopping);
        stopping.as_mut().enable();
        if !knx.shutdown_flag.load(Ordering::SeqCst) {
            stopping.await;
        }
    }
}

impl Knx {
    /// Send a connectionless individual-address request and await its matching response.
    ///
    /// The two-byte APCI service prefix identifies the expected response. Requests are
    /// serialized because connectionless responses do not carry a request sequence.
    pub async fn request_individual(
        &self,
        destination: crate::protocol::IndividualAddress,
        request_apdu: Vec<u8>,
        expected_service: [u8; 2],
        timeout_duration: std::time::Duration,
    ) -> Result<Vec<u8>> {
        let _request_guard = self.individual_request_lock.lock().await;
        let mut session = self.connectionless_p2p(destination).await?;
        let result = session
            .request(request_apdu, Some(expected_service), timeout_duration)
            .await;
        session.close().await;
        result.map(|telegram| telegram.payload)
    }

    /// Check whether an individual address is occupied by a device.
    ///
    /// A descriptor response or an explicit P2P refusal indicates an occupied
    /// address. A timeout indicates no device answered the probe.
    pub async fn check_individual_address(
        &self,
        destination: crate::protocol::IndividualAddress,
        timeout: std::time::Duration,
    ) -> Result<bool> {
        let mut connection = self.connect_p2p(destination).await?;
        let response = connection.read_device_descriptor(0, timeout).await;
        let _ = connection.close().await;
        match response {
            Ok(_)
            | Err(crate::error::KnxError::Device(
                crate::error::DeviceError::CommunicationRefused { .. },
            )) => Ok(true),
            Err(crate::error::KnxError::Device(
                crate::error::DeviceError::CommunicationTimeout { .. },
            )) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Read addresses of devices currently in programming mode.
    ///
    /// Returns all responders before the timeout elapses. Set `raise_if_multiple`
    /// to return an error when a second device responds.
    pub async fn read_individual_addresses_in_programming_mode(
        &self,
        timeout_duration: std::time::Duration,
        raise_if_multiple: bool,
    ) -> Result<Vec<crate::protocol::IndividualAddress>> {
        let responses = self
            .collect_broadcast_responses(
                vec![0x01, 0x00],
                vec![0x01, 0x40],
                timeout_duration,
                raise_if_multiple.then_some(2),
            )
            .await?;
        let addresses = responses
            .iter()
            .map(|response| response.source)
            .collect::<Vec<_>>();
        if raise_if_multiple && addresses.len() > 1 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "More than one KNX device is in programming mode".to_owned(),
            }
            .into());
        }
        Ok(addresses)
    }

    /// Assign an address to the single device in programming mode, then restart it.
    ///
    /// Rejects an occupied address unless it belongs to the programming-mode device.
    /// A device that does not answer an address check is considered absent.
    pub async fn write_individual_address_in_programming_mode(
        &self,
        address: crate::protocol::IndividualAddress,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let occupied = self.check_individual_address(address, timeout).await?;
        let devices = self
            .read_individual_addresses_in_programming_mode(timeout, true)
            .await?;
        let current =
            devices
                .first()
                .copied()
                .ok_or_else(|| crate::error::ProtocolError::InvalidFrame {
                    details: "No KNX device is in programming mode".to_owned(),
                })?;
        if occupied && current != address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("Individual address {address} is already occupied"),
            }
            .into());
        }
        if !occupied {
            let mut apdu = vec![0x00, 0xc0];
            apdu.extend_from_slice(&u16::from(address).to_be_bytes());
            let broadcast =
                RawGroupAddress::try_new(0, 0, 0).expect("0/0/0 is a valid group address");
            let mut telegram = Telegram::group_write(broadcast, apdu);
            telegram.telegram_type = TelegramType::IndividualData;
            telegram.compact_payload = false;
            self.send_telegram_acknowledged(&telegram).await?;
        }
        let mut connection = self.connect_p2p(address).await?;
        let result = async {
            connection.read_device_descriptor(0, timeout).await?;
            connection.restart().await
        }
        .await;
        let close_result = connection.close().await;
        result.and(close_result)
    }

    /// Read the address assigned to a device with the given serial number.
    pub async fn read_individual_address_by_serial_number(
        &self,
        serial: [u8; 6],
        timeout_duration: std::time::Duration,
    ) -> Result<Option<crate::protocol::IndividualAddress>> {
        let request = crate::protocol::IndividualAddressSerialRead { serial }.encode();
        let mut response_prefix = vec![0x03, 0xDD];
        response_prefix.extend_from_slice(&serial);
        let response = self
            .collect_broadcast_responses(request, response_prefix, timeout_duration, Some(1))
            .await?
            .into_iter()
            .next();
        let Some(response) = response else {
            return Ok(None);
        };
        let decoded = crate::protocol::IndividualAddressSerialResponse::decode(&response.payload)?;
        if decoded.serial != serial || decoded.address != response.source {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "IndividualAddressSerialResponse does not match its sender or request"
                    .to_owned(),
            }
            .into());
        }
        Ok(Some(response.source))
    }

    /// Assign an individual address to the device with the given serial number
    /// and verify the address by reading it back.
    pub async fn write_individual_address_by_serial_number(
        &self,
        serial: [u8; 6],
        address: crate::protocol::IndividualAddress,
        timeout_duration: std::time::Duration,
    ) -> Result<()> {
        let broadcast = RawGroupAddress::try_new(0, 0, 0).expect("0/0/0 is a valid group address");
        let mut telegram = Telegram::group_write(
            broadcast,
            crate::protocol::IndividualAddressSerialWrite { serial, address }.encode(),
        );
        telegram.telegram_type = TelegramType::IndividualData;
        telegram.compact_payload = false;
        self.send_telegram(&telegram).await?;

        let actual = self
            .read_individual_address_by_serial_number(serial, timeout_duration)
            .await?
            .ok_or_else(|| crate::error::DeviceError::CommunicationTimeout {
                device: format!("serial number {serial:02X?}"),
                timeout_ms: timeout_duration.as_millis() as u64,
            })?;
        if actual != address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Individual address assignment verification failed: expected {address}, got {actual}"
                ),
            }
            .into());
        }
        Ok(())
    }

    async fn collect_broadcast_responses(
        &self,
        request_apdu: Vec<u8>,
        response_prefix: Vec<u8>,
        timeout_duration: std::time::Duration,
        max_responses: Option<usize>,
    ) -> Result<Vec<Telegram>> {
        let _request_guard = self.individual_request_lock.lock().await;
        let stopping = self.shutdown_started.notified();
        tokio::pin!(stopping);
        stopping.as_mut().enable();
        if self.shutdown_flag.load(Ordering::SeqCst) {
            return Err(crate::error::TransportError::QueueClosed.into());
        }

        let broadcast = RawGroupAddress::try_new(0, 0, 0).expect("0/0/0 is a valid group address");
        let (tx, mut rx) = mpsc::channel(16);
        let filter = TelegramFilter::Custom(Arc::new(move |telegram| {
            telegram.destination == Address::Group(broadcast)
                && telegram.payload.starts_with(&response_prefix)
        }));
        let handle = self
            .register_telegram_callback_filtered(P2pTelegramCallback { tx }, filter, false)
            .await;
        let mut callback_guard = ReadCallbackGuard {
            handler: self.event_handler.clone(),
            handle: Some(handle),
        };

        let operation = async {
            let mut telegram = Telegram::group_write(broadcast, request_apdu);
            telegram.telegram_type = TelegramType::IndividualData;
            telegram.compact_payload = false;
            let (sent, completed) = self
                .telegram_queue
                .enqueue_outgoing_tracked(telegram)
                .await?;
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                tokio::select! {
                    sent = sent => sent.map_err(|_| crate::error::TransportError::QueueClosed.into()),
                    result = completed => result.map_err(|_| crate::error::TransportError::QueueClosed)?,
                }
            })
            .await
            .map_err(|_| crate::error::TransportError::QueueTimeout { timeout_ms: 10_000 })??;

            let mut responses = Vec::new();
            let deadline = tokio::time::sleep(timeout_duration);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => return Ok(responses),
                    response = rx.recv() => match response {
                        Some(response) => {
                            responses.push(response);
                            if max_responses.is_some_and(|limit| responses.len() >= limit) {
                                return Ok(responses);
                            }
                        }
                        None => return Ok(responses),
                    }
                }
            }
        };
        let result = tokio::select! {
            result = operation => result,
            _ = &mut stopping => Err(crate::error::TransportError::QueueClosed.into()),
        };
        self.unregister_callback(handle).await;
        callback_guard.handle = None;
        result
    }

    /// Open a connection-oriented transport link to an individual address.
    ///
    /// Only one connected session per peer may be active on a Knx instance.
    /// Connectionless requests remain available through
    /// [`Self::request_individual`].
    pub async fn connect_p2p(
        &self,
        destination: crate::protocol::IndividualAddress,
    ) -> Result<P2pConnection> {
        let (tx, receiver) = mpsc::channel(16);
        let callback = self
            .register_p2p_peer(destination, P2pMode::Connected, P2pTelegramCallback { tx })
            .await?;
        let connection = P2pConnection {
            knx: self.clone(),
            destination,
            receiver,
            pending: std::collections::VecDeque::new(),
            callback: Some(callback),
            send_sequence: 0,
            receive_sequence: 0,
            closed: false,
        };
        connection
            .send_control(crate::protocol::TpciFrame::Connect)
            .await?;
        Ok(connection)
    }

    /// Open a receive context for connectionless data from one individual address.
    ///
    /// Only one context per peer can be open. A connected P2P link to the same
    /// device can be active at the same time.
    pub async fn connectionless_p2p(
        &self,
        destination: crate::protocol::IndividualAddress,
    ) -> Result<P2pConnectionless> {
        let (tx, receiver) = tokio::sync::broadcast::channel(16);
        let registration = self
            .register_p2p_peer(
                destination,
                P2pMode::Connectionless,
                ConnectionlessResponseCallback { tx },
            )
            .await?;
        Ok(P2pConnectionless {
            registration,
            receiver,
        })
    }

    async fn register_p2p_peer(
        &self,
        destination: crate::protocol::IndividualAddress,
        mode: P2pMode,
        callback: impl TelegramCallbackFn + Send + Sync + 'static,
    ) -> Result<P2pCallbackRegistration> {
        let key = (destination, mode);
        let active_sessions = self.active_p2p_sessions.clone();
        if !active_sessions.lock().unwrap().insert(key) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("A P2P session of this type to {destination} is already active"),
            }
            .into());
        }
        let mut registration = P2pCallbackRegistration {
            knx: self.clone(),
            handle: None,
            active_sessions,
            key,
        };
        let local_address = self.config.individual_address;
        let filter = TelegramFilter::Custom(Arc::new(move |telegram| {
            if telegram.source != destination
                || telegram.destination != Address::Individual(local_address)
            {
                return false;
            }
            match mode {
                P2pMode::Connected => {
                    telegram.telegram_type == TelegramType::IndividualControl
                        || (telegram.telegram_type == TelegramType::IndividualData
                            && telegram.tpci & 0xc0 == 0x40)
                }
                P2pMode::Connectionless => {
                    telegram.telegram_type == TelegramType::IndividualData
                        && telegram.tpci & 0xfc == 0
                }
            }
        }));
        registration.handle = Some(
            self.register_telegram_callback_filtered(callback, filter, false)
                .await,
        );
        Ok(registration)
    }

    /// Open a P2P connection and switch the device's programming mode.
    ///
    /// The mask version determines whether the device uses memory or a property.
    pub async fn switch_programming_mode(
        &self,
        destination: crate::protocol::IndividualAddress,
        enabled: bool,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let mut connection = self.connect_p2p(destination).await?;
        let result = connection
            .switch_programming_mode(enabled, verify, timeout)
            .await;
        let close_result = connection.close().await;
        result.and(close_result)
    }

    /// Restart a device using a temporary P2P connection.
    ///
    /// A basic restart is unconfirmed; success means the request was sent.
    pub async fn restart_device(
        &self,
        destination: crate::protocol::IndividualAddress,
    ) -> Result<()> {
        let mut connection = self.connect_p2p(destination).await?;
        let result = connection.restart().await;
        let close_result = connection.close().await;
        result.and(close_result)
    }
}

impl P2pConnection {
    fn timeout_error(&self, timeout: std::time::Duration) -> KnxError {
        DeviceError::CommunicationTimeout {
            device: format!("individual address {}", self.destination),
            timeout_ms: timeout.as_millis() as u64,
        }
        .into()
    }

    async fn next_event_until(
        &mut self,
        deadline: tokio::time::Instant,
        timeout: std::time::Duration,
    ) -> Result<Telegram> {
        if let Some(telegram) = self.pending.pop_front() {
            return Ok(telegram);
        }
        match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
            Ok(Some(telegram)) => Ok(telegram),
            Ok(None) => Err(crate::error::TransportError::QueueClosed.into()),
            Err(_) => Err(self.timeout_error(timeout)),
        }
    }

    async fn next_channel_event_until(
        &mut self,
        deadline: tokio::time::Instant,
        timeout: std::time::Duration,
    ) -> Result<Telegram> {
        match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
            Ok(Some(telegram)) => Ok(telegram),
            Ok(None) => Err(crate::error::TransportError::QueueClosed.into()),
            Err(_) => Err(self.timeout_error(timeout)),
        }
    }

    fn control_telegram(&self, tpci: crate::protocol::TpciFrame) -> Telegram {
        let mut telegram = Telegram::individual_data(self.destination, Vec::new());
        telegram.telegram_type = TelegramType::IndividualControl;
        telegram.tpci = tpci.encode();
        telegram
    }

    async fn send_control(&self, tpci: crate::protocol::TpciFrame) -> Result<()> {
        self.knx
            .send_telegram_acknowledged(&self.control_telegram(tpci))
            .await
    }

    /// Send one connected-data APDU, repeating it up to three times after the
    /// initial transmission if its matching T_ACK is lost or rejected. Success
    /// does not wait for a CEMI L_Data.con bus confirmation.
    pub async fn send_data(&mut self, apdu: Vec<u8>, wait_for_ack: bool) -> Result<()> {
        if self.closed {
            return Err(crate::error::TransportError::QueueClosed.into());
        }
        if !(2..=u8::MAX as usize).contains(&apdu.len()) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("P2P APDU length must be in 2..=255, got {}", apdu.len()),
            }
            .into());
        }
        let sequence = self.send_sequence;
        let mut telegram = Telegram::individual_data(self.destination, apdu);
        telegram.source = self.knx.config.individual_address;
        telegram.tpci = crate::protocol::TpciFrame::DataConnected { sequence }.encode();
        for attempt in 0..=P2P_MAX_RETRANSMISSIONS {
            let (sent, completed) = self
                .knx
                .telegram_queue
                .enqueue_outgoing_tracked(telegram.clone())
                .await?;
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                tokio::select! {
                    sent = sent => sent.map_err(|_| crate::error::TransportError::QueueClosed.into()),
                    result = completed => result.map_err(|_| crate::error::TransportError::QueueClosed)?,
                }
            }).await.map_err(|_| crate::error::TransportError::QueueTimeout { timeout_ms: 10_000 })??;
            if !wait_for_ack {
                self.send_sequence = (sequence + 1) & 0x0f;
                return Ok(());
            }
            let deadline = tokio::time::Instant::now() + P2P_ACK_TIMEOUT;
            loop {
                let event = match self
                    .next_channel_event_until(deadline, P2P_ACK_TIMEOUT)
                    .await
                {
                    Ok(event) => event,
                    Err(crate::error::KnxError::Device(DeviceError::CommunicationTimeout {
                        ..
                    })) => {
                        break;
                    }
                    Err(error) => return Err(error),
                };
                if event.telegram_type == TelegramType::IndividualControl {
                    match crate::protocol::TpciFrame::decode(event.tpci) {
                        crate::protocol::TpciFrame::Ack { sequence: ack } if ack == sequence => {
                            self.send_sequence = (sequence + 1) & 0x0f;
                            return Ok(());
                        }
                        crate::protocol::TpciFrame::Nak { sequence: nak } if nak == sequence => {
                            break;
                        }
                        crate::protocol::TpciFrame::Disconnect => {
                            self.closed = true;
                            return Err(DeviceError::CommunicationRefused {
                                device: format!("individual address {}", self.destination),
                            }
                            .into());
                        }
                        _ => {}
                    }
                }
                self.pending.push_back(event);
            }
            if attempt == P2P_MAX_RETRANSMISSIONS {
                return Err(
                    self.timeout_error(P2P_ACK_TIMEOUT * (P2P_MAX_RETRANSMISSIONS as u32 + 1))
                );
            }
        }
        unreachable!("the retry loop either succeeds or returns")
    }

    /// Receive one APDU from the connected peer, optionally filtering by its
    /// two-byte response service. Connected data is acknowledged before return.
    pub async fn receive(
        &mut self,
        expected_service: Option<[u8; 2]>,
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        if self.closed {
            return Err(crate::error::TransportError::QueueClosed.into());
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let event = self.next_event_until(deadline, timeout).await?;
            if event.telegram_type == TelegramType::IndividualControl {
                if crate::protocol::TpciFrame::decode(event.tpci)
                    == crate::protocol::TpciFrame::Disconnect
                {
                    self.closed = true;
                    return Err(DeviceError::CommunicationRefused {
                        device: format!("individual address {}", self.destination),
                    }
                    .into());
                }
                continue;
            }
            if event.telegram_type != TelegramType::IndividualData {
                continue;
            }
            if let crate::protocol::TpciFrame::DataConnected { sequence } =
                crate::protocol::TpciFrame::decode(event.tpci)
            {
                let new_data = if sequence == self.receive_sequence {
                    self.receive_sequence = (sequence + 1) & 0x0f;
                    true
                } else if sequence == (self.receive_sequence.wrapping_sub(1) & 0x0f) {
                    false
                } else {
                    self.send_control(crate::protocol::TpciFrame::Nak {
                        sequence: self.receive_sequence,
                    })
                    .await?;
                    continue;
                };
                self.send_control(crate::protocol::TpciFrame::Ack { sequence })
                    .await?;
                if !new_data {
                    continue;
                }
            }
            let matches_service = expected_service.is_none_or(|service| {
                // Memory and descriptor responses carry payload bits in the service octet.
                if service[1] & 0xC0 == 0x40 && service[1] & 0x3F == 0 {
                    event.payload.first() == Some(&service[0])
                        && event.payload.get(1).is_some_and(|byte| byte & 0xC0 == 0x40)
                } else {
                    event.payload.starts_with(&service)
                }
            });
            if matches_service {
                return Ok(event.payload);
            }
        }
    }

    /// Send a connected request and return the first response with the expected
    /// APCI service within `timeout`.
    pub async fn request(
        &mut self,
        apdu: Vec<u8>,
        expected_response_service: [u8; 2],
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        self.send_data(apdu, true).await?;
        self.receive(Some(expected_response_service), timeout).await
    }

    /// Tear down the transport link and unregister its receive callback.
    pub async fn close(&mut self) -> Result<()> {
        let result = if self.closed {
            Ok(())
        } else {
            self.closed = true;
            self.send_control(crate::protocol::TpciFrame::Disconnect)
                .await
        };
        if let Some(mut callback) = self.callback.take()
            && let Some(handle) = callback.handle.take()
        {
            callback.knx.unregister_callback(handle).await;
        }
        result
    }
}

impl Drop for P2pConnection {
    fn drop(&mut self) {
        if !self.closed
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let knx = self.knx.clone();
            let destination = self.destination;
            runtime.spawn(async move {
                let mut telegram = Telegram::individual_data(destination, Vec::new());
                telegram.telegram_type = TelegramType::IndividualControl;
                telegram.tpci = crate::protocol::TpciFrame::Disconnect.encode();
                let _ = knx.send_telegram_acknowledged(&telegram).await;
            });
        }
    }
}
