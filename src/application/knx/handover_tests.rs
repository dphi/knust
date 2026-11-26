use super::*;
use crate::protocol::{
    CemiFrame, GroupValueService, address::IndividualAddress, cemi::MessageCode,
};
use crate::transport::{ConnectionStats, TransportState};
use std::sync::atomic::AtomicUsize;
use tokio::sync::Mutex;
use tokio::time::{Duration, timeout};

struct Peer {
    incoming: Mutex<mpsc::Receiver<Vec<u8>>>,
    outgoing: mpsc::Sender<Vec<u8>>,
    closes: AtomicUsize,
    fail_after_send: AtomicBool,
}

#[async_trait::async_trait]
impl Connection for Peer {
    async fn send(&self, frame: &[u8]) -> Result<()> {
        self.outgoing.send(frame.to_vec()).await.unwrap();
        if self.fail_after_send.load(Ordering::SeqCst) {
            return Err(crate::error::TransportError::ConnectionClosed.into());
        }
        Ok(())
    }
    async fn recv(&self) -> Result<Vec<u8>> {
        self.incoming
            .lock()
            .await
            .recv()
            .await
            .ok_or(crate::error::TransportError::ConnectionClosed.into())
    }
    async fn close(&self) -> Result<()> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn state(&self) -> TransportState {
        TransportState::Connected
    }
    fn stats(&self) -> ConnectionStats {
        ConnectionStats::default()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn peer() -> (Arc<Peer>, mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
    let (tx, rx) = mpsc::channel(8);
    let (sent, received) = mpsc::channel(8);
    (
        Arc::new(Peer {
            incoming: Mutex::new(rx),
            outgoing: sent,
            closes: AtomicUsize::new(0),
            fail_after_send: AtomicBool::new(false),
        }),
        tx,
        received,
    )
}

async fn knx() -> Knx {
    Knx::new(ConnectionConfig {
        connection_type: ConnectionType::Routing,
        auto_reconnect: false,
        ..Default::default()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn udp_connect_uses_configured_timeout() {
    let gateway = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = gateway.local_addr().unwrap();
    let config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some(addr.ip()),
        gateway_port: Some(addr.port()),
        timeout_ms: 50,
        ..Default::default()
    };
    let result =
        tokio::time::timeout(Duration::from_secs(1), Knx::create_connection(&config)).await;
    assert!(matches!(
        result,
        Ok(Err(crate::error::KnxError::Transport(
            crate::error::TransportError::Timeout { timeout_ms: 50 }
        )))
    ));
}

#[tokio::test]
async fn disconnect_invalidates_a_handshake_that_finishes_late() {
    use crate::error::TransportError;
    use crate::protocol::knxip::{
        ConnectResponse, DisconnectResponse, Hpai, KnxIpFrame, ServiceType,
    };

    let gateway = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = gateway.local_addr().unwrap();
    let knx = Knx::builder().tunnel(address).build().await.unwrap();
    let client = knx.clone();
    let connecting = tokio::spawn(async move { client.connect().await });
    let mut buf = [0; 1024];
    let (received, peer) = timeout(Duration::from_secs(1), gateway.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        KnxIpFrame::parse(&buf[..received])
            .unwrap()
            .header
            .service_type,
        ServiceType::ConnectRequest
    );
    assert!(matches!(
        knx.connect().await,
        Err(KnxError::Transport(
            TransportError::ConnectionTransitionInProgress
        ))
    ));
    knx.disconnect().await.unwrap();

    let response = ConnectResponse {
        channel_id: 8,
        status: ConnectResponse::STATUS_OK,
        data_endpoint: Hpai::new(address),
        crd: vec![4, 4, 0x11, 0x10],
        assigned_address: Some(IndividualAddress::new(1, 1, 16)),
    };
    gateway
        .send_to(
            &KnxIpFrame::new(ServiceType::ConnectResponse, response.serialize()).serialize(),
            peer,
        )
        .await
        .unwrap();
    let (received, cleanup_peer) = timeout(Duration::from_secs(1), gateway.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleanup_peer, peer);
    assert_eq!(
        KnxIpFrame::parse(&buf[..received])
            .unwrap()
            .header
            .service_type,
        ServiceType::DisconnectRequest
    );
    let response = DisconnectResponse::new(8, DisconnectResponse::STATUS_OK);
    gateway
        .send_to(
            &KnxIpFrame::new(ServiceType::DisconnectResponse, response.serialize()).serialize(),
            peer,
        )
        .await
        .unwrap();

    assert!(matches!(
        connecting.await.unwrap(),
        Err(KnxError::Transport(TransportError::ConnectionClosed))
    ));
    assert_eq!(knx.state().await, ConnectionState::Disconnected);
    assert!(knx.connection.borrow().is_none());
}

#[tokio::test]
async fn dropping_a_pending_connect_restores_the_disconnected_state() {
    let gateway = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let knx = Knx::builder()
        .tunnel(gateway.local_addr().unwrap())
        .build()
        .await
        .unwrap();
    let client = knx.clone();
    let connecting = tokio::spawn(async move { client.connect().await });
    timeout(Duration::from_secs(1), gateway.recv_from(&mut [0; 1024]))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(knx.state().await, ConnectionState::Connecting);
    connecting.abort();
    assert!(connecting.await.unwrap_err().is_cancelled());
    assert_eq!(knx.state().await, ConnectionState::Disconnected);
}

#[tokio::test(start_paused = true)]
async fn sender_uses_replacement_connection() {
    let knx = knx().await;
    let (old, _old_tx, mut old_sent) = peer();
    let (new, _new_tx, mut new_sent) = peer();
    knx.connection.send_replace(Some(old.clone()));
    knx.sending_task.replace(knx.start_telegram_sending_task());
    let telegram = Telegram::group_write(RawGroupAddress::try_from_raw(1).unwrap(), vec![1]);
    knx.send_telegram(&telegram).await.unwrap();
    let wire = timeout(Duration::from_secs(1), old_sent.recv())
        .await
        .unwrap()
        .unwrap();
    let frame = crate::protocol::knxip::KnxIpFrame::parse(&wire).unwrap();
    assert_eq!(
        frame.header.service_type,
        crate::protocol::knxip::ServiceType::RoutingIndication
    );
    assert_eq!(
        CemiFrame::parse(&frame.body).unwrap().message_code,
        MessageCode::LDataInd
    );
    knx.connection.send_replace(Some(new));
    knx.send_telegram(&telegram).await.unwrap();
    timeout(Duration::from_secs(1), new_sent.recv())
        .await
        .expect("sender retained old connection")
        .unwrap();
    assert!(old_sent.try_recv().is_err());
    knx.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_invalid_outgoing_telegram_does_not_retire_a_healthy_connection() {
    let knx = knx().await;
    let (connection, _incoming, mut sent) = peer();
    knx.connection.send_replace(Some(connection.clone()));
    knx.start().await.unwrap();
    let address = RawGroupAddress::try_from_raw(1).unwrap();
    let error = knx
        .send_telegram_acknowledged(&Telegram::group_write(address, vec![1; 256]))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        KnxError::Protocol(crate::error::ProtocolError::InvalidFrame { .. })
    ));
    assert!(sent.try_recv().is_err());

    knx.send_telegram_acknowledged(&Telegram::group_write(address, vec![1]))
        .await
        .unwrap();
    assert!(sent.try_recv().is_ok());
    assert_eq!(connection.closes.load(Ordering::SeqCst), 0);
    knx.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_write_that_may_have_reached_the_peer_is_not_replayed_after_reconnect() {
    let knx = Knx::builder().routing(None).build().await.unwrap();
    let (old, _old_incoming, mut old_sent) = peer();
    old.fail_after_send.store(true, Ordering::SeqCst);
    let (new, _new_incoming, mut new_sent) = peer();
    knx.connection.send_replace(Some(old));
    knx.sending_task.replace(knx.start_telegram_sending_task());

    let address = RawGroupAddress::try_from_raw(1).unwrap();
    let first = Telegram::group_write(address, vec![1]);
    let (result, ()) = tokio::join!(knx.send_telegram_acknowledged(&first), async {
        old_sent.recv().await.unwrap();
        knx.connection.send_replace(Some(new));
    });
    assert!(matches!(
        result,
        Err(KnxError::Transport(
            crate::error::TransportError::ConnectionClosed
        ))
    ));
    assert!(new_sent.try_recv().is_err());

    knx.send_telegram_acknowledged(&Telegram::group_write(address, vec![0]))
        .await
        .unwrap();
    assert!(new_sent.try_recv().is_ok());
    knx.stop().await;
}

#[tokio::test(start_paused = true)]
async fn outgoing_telegrams_expire_during_an_outage_and_in_the_backlog() {
    let knx = knx().await;
    let address = RawGroupAddress::try_from_raw(1).unwrap();
    let telegram = Telegram::group_write(address, vec![1]);
    let (_first_sent, first_result) = knx
        .telegram_queue
        .enqueue_outgoing_tracked(telegram.clone())
        .await
        .unwrap();
    let (_second_sent, second_result) = knx
        .telegram_queue
        .enqueue_outgoing_tracked(telegram.clone())
        .await
        .unwrap();
    knx.sending_task.replace(knx.start_telegram_sending_task());
    tokio::task::yield_now().await;
    tokio::time::advance(knx.telegram_queue.max_outgoing_age()).await;

    for result in [first_result, second_result] {
        assert!(matches!(
            timeout(Duration::from_secs(1), result)
                .await
                .unwrap()
                .unwrap(),
            Err(KnxError::Transport(
                crate::error::TransportError::QueueTimeout { .. }
            ))
        ));
    }

    let (replacement, _incoming, mut sent) = peer();
    knx.connection.send_replace(Some(replacement));
    assert!(sent.try_recv().is_err());
    knx.send_telegram_acknowledged(&telegram).await.unwrap();
    assert!(sent.try_recv().is_ok());
    assert!(sent.try_recv().is_err());
    knx.stop().await;
}

#[tokio::test(start_paused = true)]
async fn receiver_uses_replacement_and_keeps_queue_open_after_socket_failure() {
    let knx = knx().await;
    let (old, old_tx, _old_sent) = peer();
    let (new, new_tx, _new_sent) = peer();
    knx.connection.send_replace(Some(old.clone()));
    knx.start_telegram_receiving_task().await.unwrap();
    tokio::task::yield_now().await;
    drop(old_tx);
    tokio::task::yield_now().await;
    assert!(
        knx.send_telegram(&Telegram::group_read(
            RawGroupAddress::try_from_raw(1).unwrap()
        ))
        .await
        .is_ok(),
        "socket loss closed shared queue"
    );
    knx.connection.send_replace(Some(new));
    let cemi = CemiFrame::new(
        MessageCode::LDataInd,
        IndividualAddress::new(1, 1, 2),
        Address::Group(RawGroupAddress::try_from_raw(1).unwrap()),
        GroupValueService::Write(vec![1]).encode(),
    );
    let frame = crate::protocol::knxip::KnxIpFrame::new(
        crate::protocol::knxip::ServiceType::RoutingIndication,
        cemi.serialize().unwrap(),
    );
    new_tx.send(frame.serialize()).await.unwrap();
    let received = timeout(
        Duration::from_secs(1),
        knx.telegram_queue.dequeue_incoming(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(received.payload, [1]);
    knx.stop().await;
}

#[tokio::test(start_paused = true)]
async fn stale_loss_event_cannot_close_replacement_connection() {
    let knx = knx().await;
    let (old, _old_tx, _old_sent) = peer();
    let (new, _new_tx, _new_sent) = peer();
    knx.connection.send_replace(Some(old.clone()));
    knx.start_connection_control_task().await.unwrap();
    knx.connection.send_replace(Some(new.clone()));
    knx.control_tx
        .send(ConnectionControlMessage {
            event: ConnectionControlEvent::TunnelLost {
                channel_id: 0,
                reason: "old socket failed".into(),
            },
            connection: old,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        new.closes.load(Ordering::SeqCst),
        0,
        "stale event closed healthy replacement"
    );
    knx.stop().await;
}

#[tokio::test]
async fn repeated_reconnect_with_reused_channel_restores_both_directions() {
    use crate::protocol::knxip::{
        ConnectResponse, DisconnectRequest, DisconnectResponse, Hpai, KnxIpFrame, ServiceType,
        TunnellingRequest,
    };
    use tokio::net::UdpSocket;
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let gateway = socket.local_addr().unwrap();
    let (connected_tx, mut connected_rx) = mpsc::channel(8);
    let (written_tx, mut written_rx) = mpsc::channel(8);
    let server = tokio::spawn({
        let socket = socket.clone();
        async move {
            let mut bytes = [0; 1024];
            let mut generation = 0;
            let mut server_send_sequence = 0u8;
            loop {
                let (len, peer) = socket.recv_from(&mut bytes).await.unwrap();
                let frame = KnxIpFrame::parse(&bytes[..len]).unwrap();
                match frame.header.service_type {
                    ServiceType::ConnectRequest => {
                        generation += 1;
                        server_send_sequence = 0;
                        let response = ConnectResponse {
                            channel_id: 7,
                            status: 0,
                            data_endpoint: Hpai::new(gateway),
                            crd: vec![4, 4, 0x11, 1],
                            assigned_address: None,
                        };
                        socket
                            .send_to(
                                &KnxIpFrame::new(
                                    ServiceType::ConnectResponse,
                                    response.serialize(),
                                )
                                .serialize(),
                                peer,
                            )
                            .await
                            .unwrap();
                        connected_tx.send(peer).await.unwrap();
                    }
                    ServiceType::DisconnectRequest => {
                        socket
                            .send_to(
                                &KnxIpFrame::new(
                                    ServiceType::DisconnectResponse,
                                    DisconnectResponse::new(7, 0).serialize(),
                                )
                                .serialize(),
                                peer,
                            )
                            .await
                            .unwrap();
                    }
                    ServiceType::TunnellingRequest => {
                        // Drop both requests on this session to force send-triggered reconnect.
                        if generation == 2 {
                            continue;
                        }
                        let request = TunnellingRequest::parse(&frame.body).unwrap();
                        let ack = crate::protocol::knxip::TunnellingAck::new_ok(
                            request.communication_channel_id,
                            request.sequence_counter,
                        );
                        socket
                            .send_to(
                                &KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize())
                                    .serialize(),
                                peer,
                            )
                            .await
                            .unwrap();
                        let mut confirmation = CemiFrame::parse(&request.raw_cemi).unwrap();
                        confirmation.message_code = MessageCode::LDataCon;
                        let confirmation = TunnellingRequest::new(
                            7,
                            server_send_sequence,
                            confirmation.serialize().unwrap(),
                        );
                        socket
                            .send_to(
                                &KnxIpFrame::new(
                                    ServiceType::TunnellingRequest,
                                    confirmation.serialize(),
                                )
                                .serialize(),
                                peer,
                            )
                            .await
                            .unwrap();
                        server_send_sequence = server_send_sequence.wrapping_add(1);
                        let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
                        let service =
                            GroupValueService::decode(cemi.tpci, &cemi.apci_data).unwrap();
                        written_tx
                            .send((peer, service.payload().unwrap().to_vec()))
                            .await
                            .unwrap();
                    }
                    _ => {}
                }
            }
        }
    });
    let knx = Knx::new(ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some(gateway.ip()),
        gateway_port: Some(gateway.port()),
        ..Default::default()
    })
    .await
    .unwrap();
    let (values_tx, mut values_rx) = mpsc::channel(8);
    struct Received(mpsc::Sender<u8>);
    #[async_trait::async_trait]
    impl TelegramCallbackFn for Received {
        async fn call(&self, telegram: &Telegram) {
            self.0.send(telegram.payload[0]).await.unwrap();
        }
    }
    knx.register_telegram_callback(Received(values_tx)).await;
    timeout(Duration::from_secs(2), knx.connect())
        .await
        .unwrap()
        .unwrap();
    knx.start().await.unwrap();
    for value in 1..=3 {
        let mut peer = timeout(Duration::from_secs(2), connected_rx.recv())
            .await
            .unwrap()
            .unwrap();
        if value == 2 {
            let failed = knx
                .send_telegram_acknowledged(&Telegram::group_write(
                    RawGroupAddress::try_from_raw(1).unwrap(),
                    vec![0x20],
                ))
                .await;
            assert!(matches!(
                failed,
                Err(KnxError::Transport(
                    crate::error::TransportError::Timeout { .. }
                ))
            ));
            peer = timeout(Duration::from_secs(2), connected_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(written_rx.try_recv().is_err());
        }
        knx.send_telegram(&Telegram::group_write(
            RawGroupAddress::try_from_raw(1).unwrap(),
            vec![value],
        ))
        .await
        .unwrap();
        let (sender, payload) = timeout(Duration::from_secs(5), written_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sender, peer);
        assert_eq!(payload, [value]);
        let cemi = CemiFrame::new(
            MessageCode::LDataInd,
            IndividualAddress::new(1, 1, 2),
            Address::Group(RawGroupAddress::try_from_raw(1).unwrap()),
            GroupValueService::Write(vec![value]).encode(),
        );
        let incoming = KnxIpFrame::new(
            ServiceType::TunnellingRequest,
            TunnellingRequest::new(7, 1, cemi.serialize().unwrap()).serialize(),
        );
        socket.send_to(&incoming.serialize(), peer).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), values_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            value
        );
        if value < 3 {
            let disconnect = KnxIpFrame::new(
                ServiceType::DisconnectRequest,
                DisconnectRequest::new(7, Hpai::new(gateway)).serialize(),
            );
            socket.send_to(&disconnect.serialize(), peer).await.unwrap();
        }
    }
    knx.shutdown().await.unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test(start_paused = true)]
async fn read_deadline_starts_after_transmission_not_enqueue() {
    let knx = knx().await;
    let address = RawGroupAddress::try_from_raw(1).unwrap();
    let reading = tokio::spawn({
        let knx = knx.clone();
        async move {
            knx.read_group_value(address, Duration::from_millis(100))
                .await
        }
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !reading.is_finished(),
        "response deadline ran while request was queued"
    );
    let (connection, _incoming, mut sent) = peer();
    knx.connection.send_replace(Some(connection));
    knx.sending_task.replace(knx.start_telegram_sending_task());
    timeout(Duration::from_secs(1), sent.recv())
        .await
        .unwrap()
        .unwrap();
    knx.test_notify_telegram_received(&Telegram::received(
        IndividualAddress::new(1, 1, 2),
        address,
        crate::protocol::TelegramType::GroupValueResponse,
        vec![42],
    ))
    .await;
    assert_eq!(reading.await.unwrap().unwrap(), [42]);
    assert_eq!(knx.total_callback_count().await, 0);
    knx.stop().await;
}

#[tokio::test(start_paused = true)]
async fn p2p_ack_deadline_starts_after_transmission() {
    let knx = knx().await;
    let (events, receiver) = mpsc::channel(4);
    let mut p2p = P2pConnection {
        knx: knx.clone(),
        destination: IndividualAddress::new(1, 1, 5),
        receiver,
        pending: Default::default(),
        callback: None,
        send_sequence: 0,
        receive_sequence: 0,
        closed: false,
    };
    let sending = tokio::spawn(async move {
        let result = p2p.send_data(vec![3, 0], true).await;
        p2p.closed = true;
        assert!(p2p.closed);
        result
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        !sending.is_finished(),
        "ACK timeout ran while telegram was queued"
    );

    let (connection, _incoming, mut sent) = peer();
    knx.connection.send_replace(Some(connection));
    knx.sending_task.replace(knx.start_telegram_sending_task());
    timeout(Duration::from_secs(1), sent.recv())
        .await
        .unwrap()
        .unwrap();
    let mut ack = Telegram::individual_data(knx.config.individual_address, Vec::new());
    ack.telegram_type = TelegramType::IndividualControl;
    ack.tpci = crate::protocol::TpciFrame::Ack { sequence: 0 }.encode();
    events.send(ack).await.unwrap();
    sending.await.unwrap().unwrap();
    assert!(
        timeout(Duration::from_millis(100), sent.recv())
            .await
            .is_err()
    );
    knx.stop().await;
}

#[tokio::test(start_paused = true)]
async fn timed_out_p2p_send_is_not_transmitted_later() {
    let knx = knx().await;
    let (_events, receiver) = mpsc::channel(4);
    let mut p2p = P2pConnection {
        knx: knx.clone(),
        destination: IndividualAddress::new(1, 1, 5),
        receiver,
        pending: Default::default(),
        callback: None,
        send_sequence: 0,
        receive_sequence: 0,
        closed: false,
    };
    let sending = tokio::spawn(async move {
        let result = p2p.send_data(vec![3, 0], true).await;
        p2p.closed = true;
        assert!(p2p.closed);
        result
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_secs(11)).await;
    assert!(matches!(
        sending.await.unwrap(),
        Err(KnxError::Transport(
            crate::error::TransportError::QueueTimeout { .. }
        ))
    ));
    let (connection, _incoming, mut sent) = peer();
    knx.connection.send_replace(Some(connection));
    knx.sending_task.replace(knx.start_telegram_sending_task());
    assert!(
        timeout(Duration::from_millis(100), sent.recv())
            .await
            .is_err()
    );
    knx.stop().await;
}

#[tokio::test(start_paused = true)]
async fn unsent_or_cancelled_reads_do_not_transmit_later_or_leak_callbacks() {
    let knx = knx().await;
    let address = RawGroupAddress::try_from_raw(1).unwrap();
    let reading = tokio::spawn({
        let knx = knx.clone();
        async move { knx.read_group_value(address, Duration::from_secs(1)).await }
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_secs(11)).await;
    assert!(reading.await.unwrap().is_err());
    let cancelled = tokio::spawn({
        let knx = knx.clone();
        async move { knx.read_group_value(address, Duration::from_secs(1)).await }
    });
    tokio::task::yield_now().await;
    cancelled.abort();
    let _ = cancelled.await;
    tokio::task::yield_now().await;
    assert_eq!(
        knx.total_callback_count().await,
        0,
        "cancelled read leaked callback"
    );
    let (connection, _incoming, mut sent) = peer();
    knx.connection.send_replace(Some(connection));
    knx.sending_task.replace(knx.start_telegram_sending_task());
    assert!(
        timeout(Duration::from_millis(100), sent.recv())
            .await
            .is_err(),
        "expired read sent after its deadline"
    );
    knx.stop().await;
}

#[tokio::test(start_paused = true)]
async fn shutdown_fails_unsent_reads_promptly() {
    let knx = knx().await;
    let reading = tokio::spawn({
        let knx = knx.clone();
        async move {
            knx.read_group_value(
                RawGroupAddress::try_from_raw(1).unwrap(),
                Duration::from_secs(60),
            )
            .await
        }
    });
    tokio::task::yield_now().await;
    knx.stop().await;
    let result = timeout(Duration::from_millis(100), reading)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        result,
        Err(KnxError::Transport(
            crate::error::TransportError::QueueClosed
        ))
    ));
    assert_eq!(knx.total_callback_count().await, 0);
}

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_reads_already_waiting_for_a_bus_response() {
    let knx = knx().await;
    let (connection, _incoming, mut sent) = peer();
    knx.connection.send_replace(Some(connection));
    knx.sending_task.replace(knx.start_telegram_sending_task());
    let reading = tokio::spawn({
        let knx = knx.clone();
        async move {
            knx.read_group_value(
                RawGroupAddress::try_from_raw(1).unwrap(),
                Duration::from_secs(60),
            )
            .await
        }
    });
    timeout(Duration::from_secs(1), sent.recv())
        .await
        .unwrap()
        .unwrap();
    knx.stop().await;
    assert!(
        timeout(Duration::from_millis(100), reading)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(knx.total_callback_count().await, 0);
}
