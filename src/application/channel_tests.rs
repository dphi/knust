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
    .serialize();
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
    )
    .await
    .unwrap();
    assert!(matches!(
        control_rx.try_recv().unwrap(),
        ConnectionControlEvent::SendDisconnectResponse { channel_id: 7 }
    ));
    assert!(matches!(
        control_rx.try_recv().unwrap(),
        ConnectionControlEvent::TunnelLost { channel_id: 7, .. }
    ));
    assert!(control_rx.try_recv().is_err());
}
