//! Tunnel-channel isolation regressions against a local KNXnet/IP peer.

use super::*;
use crate::protocol::knxip::{
    ConnectResponse, DisconnectRequest, DisconnectResponse, Hpai, KnxIpFrame, ServiceType,
    TunnellingAck, TunnellingRequest,
};
use tokio::net::UdpSocket;
use tokio::time::{Duration, timeout};

async fn connected_tunnel() -> (Arc<dyn Connection>, UdpSocket) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut tunnel = Tunnel::new_udp(peer.local_addr().unwrap());
    let handshake = async {
        let mut bytes = [0; 1024];
        let (len, sender) = peer.recv_from(&mut bytes).await.unwrap();
        assert_eq!(
            KnxIpFrame::parse(&bytes[..len])
                .unwrap()
                .header
                .service_type,
            ServiceType::ConnectRequest
        );
        let response = ConnectResponse {
            channel_id: 7,
            status: 0,
            data_endpoint: Hpai::new(peer.local_addr().unwrap()),
            crd: vec![4, 4, 0x11, 1],
            assigned_address: None,
        };
        peer.send_to(
            &KnxIpFrame::new(ServiceType::ConnectResponse, response.serialize()).serialize(),
            sender,
        )
        .await
        .unwrap();
    };
    let (result, ()) = timeout(Duration::from_secs(1), async {
        tokio::join!(tunnel.connect(), handshake)
    })
    .await
    .unwrap();
    result.unwrap();
    (Arc::new(tunnel), peer)
}

async fn receive(peer: &UdpSocket) -> KnxIpFrame {
    let mut bytes = [0; 1024];
    let len = timeout(Duration::from_secs(1), peer.recv(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    KnxIpFrame::parse(&bytes[..len]).unwrap()
}

async fn assert_no_reply(peer: &UdpSocket) {
    let mut bytes = [0; 1024];
    assert!(
        timeout(Duration::from_millis(30), peer.recv(&mut bytes))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn foreign_requests_do_not_ack_advance_sequence_or_deliver() {
    use crate::protocol::{
        CemiFrame, GroupValueService,
        address::{Address, GroupAddress, IndividualAddress},
        cemi::MessageCode,
    };
    let (connection, peer) = connected_tunnel().await;
    let tunnel = connection.as_any().downcast_ref::<Tunnel>().unwrap();
    let queue = TelegramQueue::new();
    let (control_tx, _control_rx) = mpsc::channel(4);
    let cemi = CemiFrame::new(
        MessageCode::LDataInd,
        IndividualAddress::new(1, 1, 1),
        Address::Group(GroupAddress::try_from_raw(1).unwrap()),
        GroupValueService::Write(vec![1]).encode(),
    )
    .serialize()
    .unwrap();
    for (channel, sequence) in [(8, 0), (8, 255), (8, 5), (0, 0)] {
        let frame = KnxIpFrame::new(
            ServiceType::TunnellingRequest,
            TunnellingRequest::new(channel, sequence, cemi.clone()).serialize(),
        );
        Knx::handle_incoming_frame(
            &frame.serialize(),
            &queue,
            &control_tx,
            Some(7),
            &connection,
            None,
            #[cfg(feature = "secure")]
            &RwLock::new(None),
        )
        .await
        .unwrap();
        assert_eq!(tunnel.expected_sequence(), 0);
        assert!(queue.is_empty().await);
    }
    assert_no_reply(&peer).await;

    // Valid requests still deliver once; duplicates ACK without delivering twice.
    for (sequence, expected_sequence, expected_count, status) in [
        (0, 1, 1, TunnellingAck::STATUS_OK),
        (0, 1, 1, TunnellingAck::STATUS_OK),
        (1, 2, 2, TunnellingAck::STATUS_OK),
        (5, 2, 2, TunnellingAck::STATUS_ERROR_SEQUENCE_NUMBER),
    ] {
        let frame = KnxIpFrame::new(
            ServiceType::TunnellingRequest,
            TunnellingRequest::new(7, sequence, cemi.clone()).serialize(),
        );
        Knx::handle_incoming_frame(
            &frame.serialize(),
            &queue,
            &control_tx,
            Some(7),
            &connection,
            None,
            #[cfg(feature = "secure")]
            &RwLock::new(None),
        )
        .await
        .unwrap();
        let reply = receive(&peer).await;
        assert_eq!(reply.header.service_type, ServiceType::TunnellingAck);
        let ack = TunnellingAck::parse(&reply.body).unwrap();
        assert_eq!(ack.communication_channel_id, 7);
        assert_eq!(ack.sequence_counter, sequence);
        assert_eq!(ack.status_code, status);
        assert_eq!(tunnel.expected_sequence(), expected_sequence);
        assert_eq!(queue.len().await, expected_count);
    }
}

#[tokio::test]
async fn foreign_disconnect_replies_with_channel_error_without_losing_tunnel() {
    let (connection, peer) = connected_tunnel().await;
    let queue = TelegramQueue::new();
    let (control_tx, mut control_rx) = mpsc::channel(4);
    for current_channel in [Some(7), None] {
        let request = DisconnectRequest::new(8, Hpai::new(peer.local_addr().unwrap()));
        let frame = KnxIpFrame::new(ServiceType::DisconnectRequest, request.serialize());
        Knx::handle_incoming_frame(
            &frame.serialize(),
            &queue,
            &control_tx,
            current_channel,
            &connection,
            None,
            #[cfg(feature = "secure")]
            &RwLock::new(None),
        )
        .await
        .unwrap();
        let reply = receive(&peer).await;
        assert_eq!(reply.header.service_type, ServiceType::DisconnectResponse);
        let response = DisconnectResponse::parse(&reply.body).unwrap();
        assert_eq!(response.communication_channel_id, 8);
        assert_eq!(response.status, 0x21);
        assert!(control_rx.try_recv().is_err());
        assert!(
            connection
                .as_any()
                .downcast_ref::<Tunnel>()
                .unwrap()
                .is_connected()
        );
    }

    let request = DisconnectRequest::new(7, Hpai::new(peer.local_addr().unwrap()));
    let frame = KnxIpFrame::new(ServiceType::DisconnectRequest, request.serialize());
    Knx::handle_incoming_frame(
        &frame.serialize(),
        &queue,
        &control_tx,
        Some(7),
        &connection,
        None,
        #[cfg(feature = "secure")]
        &RwLock::new(None),
    )
    .await
    .unwrap();
    assert!(matches!(
        control_rx.try_recv().unwrap().event,
        ConnectionControlEvent::SendDisconnectResponse { channel_id: 7 }
    ));
    assert!(matches!(
        control_rx.try_recv().unwrap().event,
        ConnectionControlEvent::TunnelLost { channel_id: 7, .. }
    ));
    assert!(control_rx.try_recv().is_err());
}

#[tokio::test]
async fn udp_send_ignores_foreign_and_stale_acknowledgements() {
    let (connection, peer) = connected_tunnel().await;
    let destination = connection
        .as_any()
        .downcast_ref::<Tunnel>()
        .unwrap()
        .local_addr();
    let knx = Knx::new(ConnectionConfig {
        connection_type: ConnectionType::Routing,
        ..Default::default()
    })
    .await
    .unwrap();
    knx.connection.send_replace(Some(connection.clone()));
    knx.start_telegram_receiving_task().await.unwrap();
    for sequence in [5, 6] {
        let data = KnxIpFrame::new(
            ServiceType::TunnellingRequest,
            TunnellingRequest::new(7, sequence, vec![0x11]).serialize(),
        )
        .serialize();
        let sending = tokio::spawn({
            let connection = connection.clone();
            async move { Knx::send_telegram_frame(&connection, &data, 2, &mut None).await }
        });
        assert_eq!(
            receive(&peer).await.header.service_type,
            ServiceType::TunnellingRequest
        );
        for (channel, ack_sequence) in [(8, sequence), (7, sequence - 1)] {
            let ack = TunnellingAck::new_ok(channel, ack_sequence);
            peer.send_to(
                &KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize()).serialize(),
                destination,
            )
            .await
            .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !sending.is_finished(),
            "a different request's ACK confirmed this send"
        );
        let ack = TunnellingAck::new_ok(7, sequence);
        peer.send_to(
            &KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize()).serialize(),
            destination,
        )
        .await
        .unwrap();
        assert!(
            timeout(Duration::from_secs(1), sending)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
    }
    knx.stop().await;
}

#[tokio::test]
async fn udp_retry_reuses_identical_sequence_and_cancellation_releases_sender() {
    let (connection, peer) = connected_tunnel().await;
    let destination = connection
        .as_any()
        .downcast_ref::<Tunnel>()
        .unwrap()
        .local_addr();
    let knx = Knx::new(ConnectionConfig {
        connection_type: ConnectionType::Routing,
        ..Default::default()
    })
    .await
    .unwrap();
    knx.connection.send_replace(Some(connection.clone()));
    knx.start_telegram_receiving_task().await.unwrap();
    let data = KnxIpFrame::new(
        ServiceType::TunnellingRequest,
        TunnellingRequest::new(7, 9, vec![0x11]).serialize(),
    )
    .serialize();
    let sending = tokio::spawn({
        let connection = connection.clone();
        let data = data.clone();
        async move { Knx::send_telegram_frame(&connection, &data, 2, &mut None).await }
    });
    let first = receive(&peer).await.serialize();
    let mut bytes = [0; 1024];
    let len = timeout(Duration::from_secs(2), peer.recv(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes[..len], first);
    let ack = TunnellingAck::new_ok(7, 9);
    peer.send_to(
        &KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize()).serialize(),
        destination,
    )
    .await
    .unwrap();
    assert!(sending.await.unwrap().is_ok());
    let cancelled = tokio::spawn({
        let connection = connection.clone();
        let data = data.clone();
        async move { Knx::send_telegram_frame(&connection, &data, 2, &mut None).await }
    });
    receive(&peer).await;
    cancelled.abort();
    let _ = cancelled.await;
    let next = tokio::spawn({
        let connection = connection.clone();
        async move { Knx::send_telegram_frame(&connection, &data, 1, &mut None).await }
    });
    receive(&peer).await;
    let ack = TunnellingAck::new(7, 9, TunnellingAck::STATUS_ERROR_SEQUENCE_NUMBER);
    peer.send_to(
        &KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize()).serialize(),
        destination,
    )
    .await
    .unwrap();
    assert!(
        timeout(Duration::from_secs(1), next)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    knx.stop().await;
}

#[tokio::test]
async fn read_response_can_arrive_before_tunnelling_ack() {
    use crate::protocol::{
        CemiFrame, GroupValueService, TelegramType,
        address::{Address, GroupAddress, IndividualAddress},
        cemi::MessageCode,
    };
    let (connection, peer) = connected_tunnel().await;
    let destination = connection
        .as_any()
        .downcast_ref::<Tunnel>()
        .unwrap()
        .local_addr();
    let knx = Knx::new(ConnectionConfig {
        connection_type: ConnectionType::Routing,
        ..Default::default()
    })
    .await
    .unwrap();
    knx.connection.send_replace(Some(connection.clone()));
    knx.start_telegram_receiving_task().await.unwrap();
    *knx.sending_task.write().await = Some(knx.start_telegram_sending_task());
    let address = GroupAddress::try_from_raw(1).unwrap();
    let reading = tokio::spawn({
        let knx = knx.clone();
        async move {
            knx.read_group_value(address, Duration::from_millis(200))
                .await
        }
    });
    let outgoing = receive(&peer).await;
    let request = TunnellingRequest::parse(&outgoing.body).unwrap();
    let cemi = CemiFrame::new(
        MessageCode::LDataInd,
        IndividualAddress::new(1, 1, 1),
        Address::Group(address),
        GroupValueService::Response(vec![42]).encode(),
    );
    let response = KnxIpFrame::new(
        ServiceType::TunnellingRequest,
        TunnellingRequest::new(7, 0, cemi.serialize().unwrap()).serialize(),
    );
    peer.send_to(&response.serialize(), destination)
        .await
        .unwrap();
    // Feed the response through the normal callback dispatcher, not just the socket.
    let telegram = timeout(
        Duration::from_secs(1),
        knx.telegram_queue.dequeue_incoming(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(telegram.telegram_type, TelegramType::GroupValueResponse);
    knx.test_notify_telegram_received(&telegram).await;
    assert_eq!(
        timeout(Duration::from_millis(100), reading)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        [42]
    );
    let ack = TunnellingAck::new_ok(7, request.sequence_counter);
    peer.send_to(
        &KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize()).serialize(),
        destination,
    )
    .await
    .unwrap();
    knx.stop().await;
}

#[tokio::test]
async fn shutdown_keeps_receiver_alive_until_queued_send_is_acknowledged() {
    let (connection, peer) = connected_tunnel().await;
    let destination = connection
        .as_any()
        .downcast_ref::<Tunnel>()
        .unwrap()
        .local_addr();
    let knx = Knx::new(ConnectionConfig {
        connection_type: ConnectionType::Routing,
        ..Default::default()
    })
    .await
    .unwrap();
    let cleanup = knx.cleanup_task.write().await.take().unwrap();
    cleanup.abort();
    let _ = cleanup.await;
    knx.connection.send_replace(Some(connection.clone()));
    knx.start_telegram_receiving_task().await.unwrap();
    *knx.sending_task.write().await = Some(knx.start_telegram_sending_task());
    knx.send_telegram(&Telegram::group_write(
        GroupAddress::try_from_raw(1).unwrap(),
        vec![1],
    ))
    .await
    .unwrap();
    let request = TunnellingRequest::parse(&receive(&peer).await.body).unwrap();
    let stopping = tokio::spawn({
        let knx = knx.clone();
        async move { knx.stop().await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let ack = TunnellingAck::new_ok(7, request.sequence_counter);
    peer.send_to(
        &KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize()).serialize(),
        destination,
    )
    .await
    .unwrap();
    timeout(Duration::from_millis(500), stopping)
        .await
        .expect("receiver exited before ACK was delivered")
        .unwrap();
    assert_no_reply(&peer).await;
}
