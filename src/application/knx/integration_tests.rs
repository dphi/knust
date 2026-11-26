use super::*;
use crate::application::callbacks::{ConnectionState, TelegramFilter};
use crate::protocol::{
    GroupValueService,
    address::{Address, IndividualAddress, MainGroup, MiddleGroup, RawGroupAddress},
    cemi::{CemiFrame, MessageCode},
    telegram::{Direction, Priority, Telegram, TelegramType},
};
use crate::transport::ConnectionType;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{Duration, sleep};

/// Telegram callback that sleeps past the callback timeout, used to test
/// error handling in `test_knx_callback_error_handling_integration`.
struct SlowCallback {
    counter: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl crate::application::callbacks::TelegramCallbackFn for SlowCallback {
    async fn call(&self, _telegram: &Telegram) {
        self.counter.fetch_add(1, Ordering::SeqCst);
        // Sleep longer than the callback timeout to test error handling
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct LoopbackCallback(tokio::sync::mpsc::Sender<Telegram>);

#[async_trait::async_trait]
impl crate::application::callbacks::TelegramCallbackFn for LoopbackCallback {
    async fn call(&self, telegram: &Telegram) {
        let _ = self.0.send(telegram.clone()).await;
    }
}

fn assert_parsed_group_service(
    service: &GroupValueService,
    expected_type: TelegramType,
    expected_payload: &[u8],
) -> Telegram {
    let source = IndividualAddress::new(1, 1, 5);
    let destination = Address::Group(RawGroupAddress::new(
        MainGroup::new(1),
        MiddleGroup::new(2),
        3,
    ));
    let frame = CemiFrame::new(MessageCode::LDataInd, source, destination, service.encode());
    let before_parse = std::time::SystemTime::now();

    let telegram = Knx::parse_cemi_to_telegram(&frame.serialize().unwrap()).unwrap();

    let after_parse = std::time::SystemTime::now();
    assert_eq!(telegram.source, source);
    assert_eq!(telegram.destination, destination);
    assert_eq!(telegram.priority, Priority::Normal);
    assert_eq!(telegram.direction, Direction::Incoming);
    assert_eq!(telegram.telegram_type, expected_type);
    assert_eq!(telegram.payload, expected_payload);
    assert!(telegram.timestamp >= before_parse);
    assert!(telegram.timestamp <= after_parse);
    telegram
}

#[test]
fn parse_cemi_to_telegram_classifies_group_value_read() {
    assert_parsed_group_service(
        &GroupValueService::Read,
        TelegramType::GroupValueRead,
        &[0x00],
    );
}

#[test]
fn parse_cemi_to_telegram_classifies_group_value_response() {
    let telegram = assert_parsed_group_service(
        &GroupValueService::Response(vec![0x0c, 0x3f]),
        TelegramType::GroupValueResponse,
        &[0x0c, 0x3f],
    );
    assert_ne!(telegram.telegram_type, TelegramType::GroupValueWrite);
}

#[test]
fn parse_cemi_to_telegram_classifies_group_value_write() {
    let telegram = assert_parsed_group_service(
        &GroupValueService::Write(vec![0x0c, 0x3f]),
        TelegramType::GroupValueWrite,
        &[0x0c, 0x3f],
    );
    assert_ne!(telegram.telegram_type, TelegramType::GroupValueResponse);
}

#[cfg(feature = "server")]
#[tokio::test]
async fn tunnel_server_loopback_round_trips_group_telegrams() {
    use crate::transport::TunnelServer;

    let client_address = IndividualAddress::new(1, 1, 8);
    let device_address = IndividualAddress::new(1, 1, 9);
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut server_traffic = server.subscribe();
    let group = RawGroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);

    knx.send_telegram(&Telegram::group_write(group, vec![7]))
        .await
        .unwrap();
    let sent = tokio::time::timeout(Duration::from_secs(2), server_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent.destination, Address::Group(group));
    assert_eq!(sent.payload, [7]);

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let callback = knx
        .register_telegram_callback_filtered(LoopbackCallback(tx), TelegramFilter::default(), false)
        .await;
    let mut received = Telegram::group_write(group, vec![9]);
    received.source = device_address;
    server.send(received).await.unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.source, device_address);
    assert_eq!(received.destination, Address::Group(group));
    assert_eq!(received.payload, [9]);

    knx.unregister_callback(callback).await;
    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn individual_request_matches_device_and_response_service() {
    use crate::transport::TunnelServer;

    let client_address = IndividualAddress::new(1, 1, 10);
    let device_address = IndividualAddress::new(1, 1, 11);
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server_addr.ip())
        .gateway_port(server_addr.port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut received = server.subscribe();

    let knx_for_request = knx.clone();
    let request = tokio::spawn(async move {
        knx_for_request
            .request_individual(
                device_address,
                vec![0x02, 0xC8, 0xAA],
                [0x02, 0xC9],
                Duration::from_secs(2),
            )
            .await
    });
    let outgoing = tokio::time::timeout(Duration::from_secs(2), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outgoing.telegram_type, TelegramType::IndividualData);
    assert_eq!(outgoing.destination, Address::Individual(device_address));
    assert_eq!(outgoing.payload, [0x02, 0xC8, 0xAA]);

    let mut wrong_service = Telegram::individual_data(client_address, vec![0x02, 0xC8, 0xBB]);
    wrong_service.source = device_address;
    server.send(wrong_service).await.unwrap();

    let mut wrong_sender = Telegram::individual_data(client_address, vec![0x02, 0xC9, 0x11]);
    wrong_sender.source = IndividualAddress::new(1, 1, 12);
    server.send(wrong_sender).await.unwrap();

    let mut response = Telegram::individual_data(client_address, vec![0x02, 0xC9, 0x42, 0x55]);
    response.source = device_address;
    server.send(response).await.unwrap();

    assert_eq!(request.await.unwrap().unwrap(), [0x02, 0xC9, 0x42, 0x55]);
    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn connectionless_p2p_collects_replies_alongside_a_connected_session() {
    use crate::protocol::TpciFrame;
    use crate::transport::TunnelServer;
    use tokio::time::{Instant, timeout};

    let client_address = IndividualAddress::new(1, 1, 10);
    let device_address = IndividualAddress::new(1, 1, 11);
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(IndividualAddress::new(1, 1, 240))
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut traffic = server.subscribe();

    let mut connected = knx.connect_p2p(device_address).await.unwrap();
    let connect = timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(connect.tpci, TpciFrame::Connect.encode());
    assert_eq!(connect.source, client_address);

    let mut session = knx.connectionless_p2p(device_address).await.unwrap();
    assert!(knx.connectionless_p2p(device_address).await.is_err());

    let reply = |payload: Vec<u8>| {
        let mut telegram = Telegram::individual_data(client_address, payload);
        telegram.source = device_address;
        telegram
    };
    knx.test_notify_telegram_received(&reply(vec![0x03, 0x40, 0, 0]))
        .await;
    session.send(vec![0x03, 0x00]).await.unwrap();
    let request = timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.destination, Address::Individual(device_address));
    assert_eq!(request.source, client_address);
    assert_eq!(request.tpci, TpciFrame::DataUnconnected.encode());
    assert_eq!(request.payload, [0x03, 0x00]);

    let replies = [[0x03, 0x40, 0x00, 0x12], [0x03, 0x40, 0x00, 0x20]];
    server.send(reply(vec![0x03, 0x00])).await.unwrap();
    server.send(reply(replies[0].to_vec())).await.unwrap();
    let mut numbered = reply(vec![0x03, 0x40, 0x03, 0x00]);
    numbered.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
    server.send(numbered).await.unwrap();
    server.send(reply(replies[1].to_vec())).await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    for expected in replies {
        let received = session
            .receive(Some([0x03, 0x40]), deadline)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.payload, expected);
    }
    assert_eq!(
        connected
            .receive(Some([0x03, 0x40]), Duration::from_secs(2))
            .await
            .unwrap(),
        [0x03, 0x40, 0x03, 0x00],
    );
    let ack = timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ack.tpci, TpciFrame::Ack { sequence: 0 }.encode());
    assert!(
        session
            .receive(None, Instant::now() + Duration::from_millis(50))
            .await
            .unwrap()
            .is_none()
    );

    session.close().await;
    knx.connectionless_p2p(device_address)
        .await
        .unwrap()
        .close()
        .await;
    connected.close().await.unwrap();
    assert_eq!(knx.total_callback_count().await, 0);
    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn programming_mode_assignment_rejects_conflicts_and_ambiguous_scans() {
    use crate::protocol::TpciFrame;
    use crate::transport::TunnelServer;
    let client_address = IndividualAddress::new(1, 1, 53);
    let assigned = IndividualAddress::new(1, 1, 54);
    let other = IndividualAddress::new(1, 1, 55);
    let second = IndividualAddress::new(1, 1, 56);
    let broadcast = RawGroupAddress::try_new(0, 0, 0).unwrap();
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut traffic = server.subscribe();
    for addresses in [vec![other], vec![], vec![other, second]] {
        let client = knx.clone();
        let assigning = tokio::spawn(async move {
            client
                .write_individual_address_in_programming_mode(assigned, Duration::from_millis(500))
                .await
        });
        let connect = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(connect.tpci, TpciFrame::Connect.encode());
        let probe = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(probe.payload, [0x03, 0x00]);
        let mut refusal = Telegram::individual_data(client_address, Vec::new());
        refusal.source = assigned;
        refusal.telegram_type = TelegramType::IndividualControl;
        refusal.tpci = TpciFrame::Disconnect.encode();
        server.send(refusal).await.unwrap();
        let scan = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(scan.destination, Address::Group(broadcast));
        assert_eq!(scan.payload, [0x01, 0x00]);
        for device in addresses {
            server
                .send(Telegram::received(
                    device,
                    broadcast,
                    TelegramType::IndividualData,
                    vec![0x01, 0x40],
                ))
                .await
                .unwrap();
        }
        assert!(assigning.await.unwrap().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), traffic.recv())
                .await
                .is_err()
        );
    }
    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn programming_mode_assignment_verifies_and_restarts_device() {
    use crate::protocol::TpciFrame;
    use crate::transport::TunnelServer;
    let client_address = IndividualAddress::new(1, 1, 50);
    let old_address = IndividualAddress::new(1, 1, 51);
    let assigned = IndividualAddress::new(1, 1, 52);
    let broadcast = RawGroupAddress::try_new(0, 0, 0).unwrap();
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut traffic = server.subscribe();
    let client = knx.clone();
    let assigning = tokio::spawn(async move {
        client
            .write_individual_address_in_programming_mode(assigned, Duration::from_millis(500))
            .await
    });

    let connect = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(connect.tpci, TpciFrame::Connect.encode());
    assert_eq!(connect.destination, Address::Individual(assigned));
    for _ in 0..4 {
        let probe = tokio::time::timeout(Duration::from_secs(4), traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(probe.payload, [0x03, 0x00]);
        assert_eq!(
            probe.tpci,
            TpciFrame::DataConnected { sequence: 0 }.encode()
        );
    }
    let disconnect = tokio::time::timeout(Duration::from_secs(4), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disconnect.tpci, TpciFrame::Disconnect.encode());
    let scan = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scan.destination, Address::Group(broadcast));
    assert_eq!(scan.payload, [0x01, 0x00]);
    server
        .send(Telegram::received(
            old_address,
            broadcast,
            TelegramType::IndividualData,
            vec![0x01, 0x40],
        ))
        .await
        .unwrap();
    let write = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    let mut expected = vec![0x00, 0xc0];
    expected.extend_from_slice(&u16::from(assigned).to_be_bytes());
    assert_eq!(write.destination, Address::Group(broadcast));
    assert_eq!(write.payload, expected);
    let connect = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(connect.tpci, TpciFrame::Connect.encode());
    let probe = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(probe.payload, [0x03, 0x00]);
    let mut ack = Telegram::individual_data(client_address, Vec::new());
    ack.source = assigned;
    ack.telegram_type = TelegramType::IndividualControl;
    ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
    server.send(ack).await.unwrap();
    let mut response = Telegram::individual_data(
        client_address,
        crate::protocol::management::DeviceDescriptorResponse {
            descriptor: 0,
            data: vec![0x06, 0x10],
        }
        .encode(),
    );
    response.source = assigned;
    response.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
    server.send(response).await.unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ack.tpci, TpciFrame::Ack { sequence: 0 }.encode());
    let restart = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restart.payload, [0x03, 0x80]);
    assert_eq!(
        restart.tpci,
        TpciFrame::DataConnected { sequence: 1 }.encode()
    );
    let disconnect = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disconnect.tpci, TpciFrame::Disconnect.encode());
    assigning.await.unwrap().unwrap();
    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn programming_mode_address_scan_collects_broadcast_responses() {
    use crate::transport::TunnelServer;

    let client_address = IndividualAddress::new(1, 1, 10);
    let first = IndividualAddress::new(1, 1, 11);
    let second = IndividualAddress::new(15, 15, 255);
    let broadcast = RawGroupAddress::try_new(0, 0, 0).expect("0/0/0 is a valid group address");
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut traffic = server.subscribe();

    let knx_for_scan = knx.clone();
    let scan = tokio::spawn(async move {
        knx_for_scan
            .read_individual_addresses_in_programming_mode(Duration::from_secs(2), false)
            .await
    });
    let request = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.destination, Address::Group(broadcast));
    assert_eq!(request.payload, [0x01, 0x00]);
    for address in [first, second] {
        server
            .send(Telegram::received(
                address,
                broadcast,
                TelegramType::IndividualData,
                vec![0x01, 0x40],
            ))
            .await
            .unwrap();
    }
    assert_eq!(scan.await.unwrap().unwrap(), [first, second]);

    let serial = [1, 2, 3, 4, 5, 6];
    let knx_for_serial_read = knx.clone();
    let serial_read = tokio::spawn(async move {
        knx_for_serial_read
            .read_individual_address_by_serial_number(serial, Duration::from_secs(2))
            .await
    });
    let serial_request = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serial_request.payload,
        crate::protocol::IndividualAddressSerialRead { serial }.encode()
    );
    for (reply_serial, address) in [([6, 5, 4, 3, 2, 1], second), (serial, first)] {
        server
            .send(Telegram::received(
                address,
                broadcast,
                TelegramType::IndividualData,
                crate::protocol::IndividualAddressSerialResponse {
                    serial: reply_serial,
                    address,
                }
                .encode(),
            ))
            .await
            .unwrap();
    }
    assert_eq!(serial_read.await.unwrap().unwrap(), Some(first));

    let assigned = IndividualAddress::new(2, 3, 4);
    let knx_for_serial_write = knx.clone();
    let serial_write = tokio::spawn(async move {
        knx_for_serial_write
            .write_individual_address_by_serial_number(serial, assigned, Duration::from_secs(2))
            .await
    });
    let write = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        write.payload,
        crate::protocol::IndividualAddressSerialWrite {
            serial,
            address: assigned
        }
        .encode()
    );
    let read_back = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        read_back.payload,
        crate::protocol::IndividualAddressSerialRead { serial }.encode()
    );
    server
        .send(Telegram::received(
            assigned,
            broadcast,
            TelegramType::IndividualData,
            crate::protocol::IndividualAddressSerialResponse {
                serial,
                address: assigned,
            }
            .encode(),
        ))
        .await
        .unwrap();
    serial_write.await.unwrap().unwrap();

    let knx_for_scan = knx.clone();
    let strict_scan = tokio::spawn(async move {
        knx_for_scan
            .read_individual_addresses_in_programming_mode(Duration::from_secs(10), true)
            .await
    });
    let _request = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    for address in [first, second] {
        server
            .send(Telegram::received(
                address,
                broadcast,
                TelegramType::IndividualData,
                vec![0x01, 0x40],
            ))
            .await
            .unwrap();
    }
    let error = strict_scan.await.unwrap().unwrap_err();
    assert!(error.to_string().contains("More than one KNX device"));
    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn individual_address_check_detects_connected_and_refusing_devices() {
    use crate::protocol::TpciFrame;
    use crate::transport::TunnelServer;

    let client_address = IndividualAddress::new(1, 1, 40);
    let connected_device = IndividualAddress::new(1, 1, 41);
    let refusing_device = IndividualAddress::new(1, 1, 42);
    let silent_device = IndividualAddress::new(1, 1, 43);
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut peer_traffic = server.subscribe();

    let knx_for_check = knx.clone();
    let checking = tokio::spawn(async move {
        knx_for_check
            .check_individual_address(connected_device, Duration::from_secs(2))
            .await
    });
    let connect_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        connect_request.destination,
        Address::Individual(connected_device)
    );
    let descriptor_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(descriptor_read.payload, [0x03, 0x00]);
    let mut descriptor_ack = Telegram::individual_data(client_address, Vec::new());
    descriptor_ack.source = connected_device;
    descriptor_ack.telegram_type = TelegramType::IndividualControl;
    descriptor_ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
    server.send(descriptor_ack).await.unwrap();
    let mut descriptor_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::DeviceDescriptorResponse {
            descriptor: 0,
            data: vec![0x06, 0x10],
        }
        .encode(),
    );
    descriptor_response.source = connected_device;
    descriptor_response.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
    server.send(descriptor_response).await.unwrap();
    assert!(checking.await.unwrap().unwrap());
    let response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response_ack.tpci, TpciFrame::Ack { sequence: 0 }.encode());
    let disconnect = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disconnect.tpci, TpciFrame::Disconnect.encode());

    let knx_for_check = knx.clone();
    let checking = tokio::spawn(async move {
        knx_for_check
            .check_individual_address(refusing_device, Duration::from_secs(2))
            .await
    });
    let _connect_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    let descriptor = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(descriptor.payload, [0x03, 0x00]);
    let mut refusal = Telegram::individual_data(client_address, Vec::new());
    refusal.source = refusing_device;
    refusal.telegram_type = TelegramType::IndividualControl;
    refusal.tpci = TpciFrame::Disconnect.encode();
    server.send(refusal).await.unwrap();
    assert!(checking.await.unwrap().unwrap());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
            .await
            .is_err()
    );

    let knx_for_check = knx.clone();
    let checking = tokio::spawn(async move {
        knx_for_check
            .check_individual_address(silent_device, Duration::from_secs(1))
            .await
    });
    let _connect_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    let descriptor_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(descriptor_read.payload, [0x03, 0x00]);
    let mut descriptor_ack = Telegram::individual_data(client_address, Vec::new());
    descriptor_ack.source = silent_device;
    descriptor_ack.telegram_type = TelegramType::IndividualControl;
    descriptor_ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
    server.send(descriptor_ack).await.unwrap();
    assert!(!checking.await.unwrap().unwrap());
    let disconnect = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disconnect.tpci, TpciFrame::Disconnect.encode());

    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn programming_mode_property_switch_verifies_over_knust_loopback() {
    use crate::protocol::TpciFrame;
    use crate::transport::TunnelServer;

    let client_address = IndividualAddress::new(1, 1, 32);
    let device_address = IndividualAddress::new(1, 1, 33);
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut peer_traffic = server.subscribe();
    let knx_for_connect = knx.clone();
    let connect = tokio::spawn(async move { knx_for_connect.connect_p2p(device_address).await });
    tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    let mut p2p = connect.await.unwrap().unwrap();

    use crate::protocol::management::{PropertyValueResponse, PropertyValueWrite};
    let switching = tokio::spawn(async move {
        for (verify, expected_success) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let result = p2p
                .switch_programming_mode_property(true, verify, Duration::from_secs(2))
                .await;
            assert_eq!(result.is_ok(), expected_success);
        }
        p2p.close().await.unwrap();
    });
    for (sequence, count, data) in [
        (0, 1, vec![1]),
        (1, 1, vec![0]),
        (2, 1, vec![0]),
        (3, 0, vec![]),
    ] {
        let write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(write.tpci, TpciFrame::DataConnected { sequence }.encode());
        assert_eq!(
            PropertyValueWrite::decode(&write.payload).unwrap(),
            PropertyValueWrite {
                object_index: 0,
                property_id: 54,
                count: 1,
                start_index: 1,
                data: vec![1],
            }
        );
        let mut ack = Telegram::individual_data(client_address, Vec::new());
        ack.source = device_address;
        ack.telegram_type = TelegramType::IndividualControl;
        ack.tpci = TpciFrame::Ack { sequence }.encode();
        server.send(ack).await.unwrap();

        let mut response = Telegram::individual_data(
            client_address,
            PropertyValueResponse {
                object_index: 0,
                property_id: 54,
                count,
                start_index: 1,
                data,
            }
            .encode()
            .unwrap(),
        );
        response.source = device_address;
        response.tpci = TpciFrame::DataConnected { sequence }.encode();
        server.send(response).await.unwrap();
        let ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ack.tpci, TpciFrame::Ack { sequence }.encode());
    }
    let disconnect = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disconnect.tpci, TpciFrame::Disconnect.encode());
    switching.await.unwrap();

    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn programming_mode_selects_mask_and_rejects_unknown_over_loopback() {
    use crate::protocol::{
        TpciFrame,
        management::{
            DeviceDescriptorResponse, MemoryRead, MemoryResponse, PropertyValueResponse,
            PropertyValueWrite,
        },
    };
    use crate::transport::TunnelServer;
    let client_address = IndividualAddress::new(1, 1, 34);
    let device_address = IndividualAddress::new(1, 1, 35);
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut traffic = server.subscribe();

    for mask in [0x0012_u16, 0x0300, 0xffff] {
        let client = knx.clone();
        let task = tokio::spawn(async move {
            client
                .switch_programming_mode(device_address, true, false, Duration::from_secs(2))
                .await
        });
        let connect = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(connect.tpci, TpciFrame::Connect.encode());
        let descriptor = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(descriptor.payload, [0x03, 0x00]);
        let mut ack = Telegram::individual_data(client_address, Vec::new());
        ack.source = device_address;
        ack.telegram_type = TelegramType::IndividualControl;
        ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
        server.send(ack).await.unwrap();
        let mut response = Telegram::individual_data(
            client_address,
            DeviceDescriptorResponse {
                descriptor: 0,
                data: mask.to_be_bytes().to_vec(),
            }
            .encode(),
        );
        response.source = device_address;
        response.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
        server.send(response).await.unwrap();
        let ack = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ack.tpci, TpciFrame::Ack { sequence: 0 }.encode());

        if mask != 0xffff {
            let operation = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
                .await
                .unwrap()
                .unwrap();
            let payload = if mask == 0x0012 {
                assert_eq!(
                    MemoryRead::decode(&operation.payload).unwrap(),
                    MemoryRead {
                        address: 0x60,
                        count: 1
                    }
                );
                MemoryResponse {
                    address: 0x60,
                    count: 1,
                    data: vec![1],
                }
                .encode()
                .unwrap()
            } else {
                assert_eq!(
                    PropertyValueWrite::decode(&operation.payload).unwrap(),
                    PropertyValueWrite {
                        object_index: 0,
                        property_id: 54,
                        count: 1,
                        start_index: 1,
                        data: vec![1]
                    }
                );
                PropertyValueResponse {
                    object_index: 0,
                    property_id: 54,
                    count: 1,
                    start_index: 1,
                    data: vec![1],
                }
                .encode()
                .unwrap()
            };
            let mut ack = Telegram::individual_data(client_address, Vec::new());
            ack.source = device_address;
            ack.telegram_type = TelegramType::IndividualControl;
            ack.tpci = TpciFrame::Ack { sequence: 1 }.encode();
            server.send(ack).await.unwrap();
            let mut response = Telegram::individual_data(client_address, payload);
            response.source = device_address;
            response.tpci = TpciFrame::DataConnected { sequence: 1 }.encode();
            server.send(response).await.unwrap();
            let ack = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(ack.tpci, TpciFrame::Ack { sequence: 1 }.encode());
        }
        let disconnect = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(disconnect.tpci, TpciFrame::Disconnect.encode());
        assert_eq!(task.await.unwrap().is_ok(), mask != 0xffff);
    }
    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn basic_restart_is_sent_without_waiting_for_device_ack() {
    use crate::protocol::TpciFrame;
    use crate::transport::TunnelServer;

    let client_address = IndividualAddress::new(1, 1, 36);
    let device_address = IndividualAddress::new(1, 1, 37);
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut traffic = server.subscribe();
    let client = knx.clone();
    let restarting = tokio::spawn(async move { client.restart_device(device_address).await });

    let connect = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(connect.destination, Address::Individual(device_address));
    assert_eq!(connect.tpci, TpciFrame::Connect.encode());
    let restart = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restart.destination, Address::Individual(device_address));
    assert_eq!(
        restart.tpci,
        TpciFrame::DataConnected { sequence: 0 }.encode()
    );
    assert_eq!(restart.payload, [0x03, 0x80]);
    let disconnect = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disconnect.tpci, TpciFrame::Disconnect.encode());
    tokio::time::timeout(Duration::from_secs(2), restarting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), traffic.recv())
            .await
            .is_err()
    );

    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn programming_mode_memory_switch_verifies_over_knust_loopback() {
    use crate::protocol::{
        TpciFrame,
        management::{MemoryRead, MemoryResponse, MemoryWrite},
    };
    use crate::transport::TunnelServer;

    let client_address = IndividualAddress::new(1, 1, 30);
    let device_address = IndividualAddress::new(1, 1, 31);
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut traffic = server.subscribe();
    let client = knx.clone();
    let connect = tokio::spawn(async move { client.connect_p2p(device_address).await });
    let request = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.tpci, TpciFrame::Connect.encode());
    let mut p2p = connect.await.unwrap().unwrap();

    let switching = tokio::spawn(async move {
        p2p.switch_programming_mode_memory(true, true, Duration::from_secs(2))
            .await?;
        p2p.switch_programming_mode_memory(true, true, Duration::from_secs(2))
            .await?;
        p2p.switch_programming_mode_memory(false, true, Duration::from_secs(2))
            .await?;
        Ok::<_, KnxError>(p2p)
    });
    let mut value = 0x2a;
    let mut response_sequence = 0;
    for (sequence, write) in [
        (0, None),
        (1, Some(0xab)),
        (2, None),
        (3, None),
        (4, None),
        (5, Some(0x2a)),
        (6, None),
    ] {
        let request = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.tpci, TpciFrame::DataConnected { sequence }.encode());
        if let Some(expected) = write {
            assert_eq!(
                MemoryWrite::decode(&request.payload).unwrap(),
                MemoryWrite {
                    address: 0x0060,
                    count: 1,
                    data: vec![expected],
                }
            );
            value = expected;
        } else {
            assert_eq!(
                MemoryRead::decode(&request.payload).unwrap(),
                MemoryRead {
                    address: 0x0060,
                    count: 1
                }
            );
        }
        let mut ack = Telegram::individual_data(client_address, Vec::new());
        ack.source = device_address;
        ack.telegram_type = TelegramType::IndividualControl;
        ack.tpci = TpciFrame::Ack { sequence }.encode();
        server.send(ack).await.unwrap();
        if write.is_none() {
            let mut response = Telegram::individual_data(
                client_address,
                MemoryResponse {
                    address: 0x0060,
                    count: 1,
                    data: vec![value],
                }
                .encode()
                .unwrap(),
            );
            response.source = device_address;
            response.tpci = TpciFrame::DataConnected {
                sequence: response_sequence,
            }
            .encode();
            server.send(response).await.unwrap();
            let ack = tokio::time::timeout(Duration::from_secs(2), traffic.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                ack.tpci,
                TpciFrame::Ack {
                    sequence: response_sequence
                }
                .encode()
            );
            response_sequence += 1;
        }
    }
    let mut p2p = switching.await.unwrap().unwrap();
    p2p.close().await.unwrap();
    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn p2p_session_connects_sends_acknowledged_data_and_closes() {
    use crate::protocol::TpciFrame;
    use crate::transport::TunnelServer;

    let client_address = IndividualAddress::new(1, 1, 20);
    let device_address = IndividualAddress::new(1, 1, 21);
    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
        .await
        .unwrap();
    let knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip(server.local_addr().ip())
        .gateway_port(server.local_addr().port())
        .individual_address(client_address)
        .build()
        .await
        .unwrap();
    knx.connect().await.unwrap();
    knx.start().await.unwrap();
    let mut peer_traffic = server.subscribe();

    let knx_for_connect = knx.clone();
    let connect = tokio::spawn(async move { knx_for_connect.connect_p2p(device_address).await });
    let connect_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        connect_request.telegram_type,
        TelegramType::IndividualControl
    );
    assert_eq!(connect_request.tpci, TpciFrame::Connect.encode());
    assert_eq!(
        connect_request.destination,
        Address::Individual(device_address)
    );

    let mut p2p = connect.await.unwrap().unwrap();
    assert!(knx.connect_p2p(device_address).await.is_err());

    let request = tokio::spawn(async move {
        let response = p2p
            .request(vec![0x02, 0xC8, 0x10], [0x02, 0xC9], Duration::from_secs(2))
            .await?;
        Ok::<_, KnxError>((p2p, response))
    });
    let data = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(data.telegram_type, TelegramType::IndividualData);
    assert_eq!(data.tpci, TpciFrame::DataConnected { sequence: 0 }.encode());
    assert_eq!(data.payload, [0x02, 0xC8, 0x10]);

    let mut stale_ack = Telegram::individual_data(client_address, Vec::new());
    stale_ack.source = device_address;
    stale_ack.telegram_type = TelegramType::IndividualControl;
    stale_ack.tpci = TpciFrame::Ack { sequence: 1 }.encode();
    server.send(stale_ack).await.unwrap();

    let mut data_nak = Telegram::individual_data(client_address, Vec::new());
    data_nak.source = device_address;
    data_nak.telegram_type = TelegramType::IndividualControl;
    data_nak.tpci = TpciFrame::Nak { sequence: 0 }.encode();
    server.send(data_nak).await.unwrap();
    let retry = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retry.tpci,
        TpciFrame::DataConnected { sequence: 0 }.encode()
    );
    assert_eq!(retry.payload, [0x02, 0xC8, 0x10]);
    let timeout_retry = tokio::time::timeout(Duration::from_secs(4), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        timeout_retry.tpci,
        TpciFrame::DataConnected { sequence: 0 }.encode()
    );
    assert_eq!(timeout_retry.payload, [0x02, 0xC8, 0x10]);

    let mut data_ack = Telegram::individual_data(client_address, Vec::new());
    data_ack.source = device_address;
    data_ack.telegram_type = TelegramType::IndividualControl;
    data_ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
    server.send(data_ack).await.unwrap();
    let mut response = Telegram::individual_data(client_address, vec![0x02, 0xC9, 0x42]);
    response.source = device_address;
    response.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
    server.send(response).await.unwrap();
    let (mut p2p, response_apdu) = request.await.unwrap().unwrap();
    assert_eq!(response_apdu, [0x02, 0xC9, 0x42]);
    let response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response_ack.telegram_type, TelegramType::IndividualControl);
    assert_eq!(response_ack.tpci, TpciFrame::Ack { sequence: 0 }.encode());

    let mut duplicate = Telegram::individual_data(client_address, vec![0x02, 0xC9, 0x42]);
    duplicate.source = device_address;
    duplicate.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
    server.send(duplicate).await.unwrap();
    assert!(p2p.receive(None, Duration::from_millis(50)).await.is_err());
    let duplicate_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(duplicate_ack.tpci, TpciFrame::Ack { sequence: 0 }.encode());

    let send_next = tokio::spawn(async move {
        p2p.send_data(vec![0x02, 0xC8, 0x20], true).await?;
        Ok::<_, KnxError>(p2p)
    });
    let next_data = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next_data.tpci,
        TpciFrame::DataConnected { sequence: 1 }.encode()
    );
    let mut next_ack = Telegram::individual_data(client_address, Vec::new());
    next_ack.source = device_address;
    next_ack.telegram_type = TelegramType::IndividualControl;
    next_ack.tpci = TpciFrame::Ack { sequence: 1 }.encode();
    server.send(next_ack).await.unwrap();
    let mut p2p = send_next.await.unwrap().unwrap();

    let property_read = crate::protocol::management::PropertyValueRead {
        object_index: 1,
        property_id: 2,
        count: 1,
        start_index: 5,
    };
    let property_apdu = property_read.encode().unwrap();
    let reading = tokio::spawn(async move {
        let response = p2p
            .read_property_value(property_read, Duration::from_secs(2))
            .await?;
        Ok::<_, KnxError>((p2p, response))
    });
    let property_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        property_request.tpci,
        TpciFrame::DataConnected { sequence: 2 }.encode()
    );
    assert_eq!(property_request.payload, property_apdu);
    let mut property_ack = Telegram::individual_data(client_address, Vec::new());
    property_ack.source = device_address;
    property_ack.telegram_type = TelegramType::IndividualControl;
    property_ack.tpci = TpciFrame::Ack { sequence: 2 }.encode();
    server.send(property_ack).await.unwrap();
    let mut property_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 5,
            data: vec![0xAA, 0xBB],
        }
        .encode()
        .unwrap(),
    );
    property_response.source = device_address;
    property_response.tpci = TpciFrame::DataConnected { sequence: 1 }.encode();
    server.send(property_response).await.unwrap();
    let (mut p2p, property_response) = reading.await.unwrap().unwrap();
    assert_eq!(property_response.data, [0xAA, 0xBB]);
    let property_response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        property_response_ack.tpci,
        TpciFrame::Ack { sequence: 1 }.encode()
    );

    let mismatched_read = crate::protocol::management::PropertyValueRead {
        object_index: 1,
        property_id: 2,
        count: 1,
        start_index: 6,
    };
    let reading = tokio::spawn(async move {
        let result = p2p
            .read_property_value(mismatched_read, Duration::from_secs(2))
            .await;
        Ok::<_, KnxError>((p2p, result))
    });
    let mismatched_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mismatched_request.tpci,
        TpciFrame::DataConnected { sequence: 3 }.encode()
    );
    let mut mismatched_request_ack = Telegram::individual_data(client_address, Vec::new());
    mismatched_request_ack.source = device_address;
    mismatched_request_ack.telegram_type = TelegramType::IndividualControl;
    mismatched_request_ack.tpci = TpciFrame::Ack { sequence: 3 }.encode();
    server.send(mismatched_request_ack).await.unwrap();
    let mut mismatched_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 7,
            data: vec![0xCC],
        }
        .encode()
        .unwrap(),
    );
    mismatched_response.source = device_address;
    mismatched_response.tpci = TpciFrame::DataConnected { sequence: 2 }.encode();
    server.send(mismatched_response).await.unwrap();
    let (mut p2p, result) = reading.await.unwrap().unwrap();
    assert!(result.is_err());
    let mismatched_response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mismatched_response_ack.tpci,
        TpciFrame::Ack { sequence: 2 }.encode()
    );

    let property_write = crate::protocol::management::PropertyValueWrite {
        object_index: 1,
        property_id: 2,
        count: 1,
        start_index: 8,
        data: vec![0x12, 0x34],
    };
    let property_apdu = property_write.encode().unwrap();
    let writing = tokio::spawn(async move {
        let response = p2p
            .write_property_value(property_write, Duration::from_secs(2))
            .await?;
        Ok::<_, KnxError>((p2p, response))
    });
    let property_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        property_request.tpci,
        TpciFrame::DataConnected { sequence: 4 }.encode()
    );
    assert_eq!(property_request.payload, property_apdu);
    let mut property_ack = Telegram::individual_data(client_address, Vec::new());
    property_ack.source = device_address;
    property_ack.telegram_type = TelegramType::IndividualControl;
    property_ack.tpci = TpciFrame::Ack { sequence: 4 }.encode();
    server.send(property_ack).await.unwrap();
    let mut property_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 8,
            data: vec![0x12, 0x34],
        }
        .encode()
        .unwrap(),
    );
    property_response.source = device_address;
    property_response.tpci = TpciFrame::DataConnected { sequence: 3 }.encode();
    server.send(property_response).await.unwrap();
    let (mut p2p, property_response) = writing.await.unwrap().unwrap();
    assert_eq!(property_response.data, [0x12, 0x34]);
    let property_response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        property_response_ack.tpci,
        TpciFrame::Ack { sequence: 3 }.encode()
    );

    let description_read = crate::protocol::management::PropertyDescriptionRead {
        object_index: 1,
        property_id: 2,
        property_index: 1,
    };
    let description_apdu = description_read.encode();
    let reading = tokio::spawn(async move {
        let response = p2p
            .read_property_description(description_read, Duration::from_secs(2))
            .await?;
        Ok::<_, KnxError>((p2p, response))
    });
    let description_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        description_request.tpci,
        TpciFrame::DataConnected { sequence: 5 }.encode()
    );
    assert_eq!(description_request.payload, description_apdu);
    let mut description_ack = Telegram::individual_data(client_address, Vec::new());
    description_ack.source = device_address;
    description_ack.telegram_type = TelegramType::IndividualControl;
    description_ack.tpci = TpciFrame::Ack { sequence: 5 }.encode();
    server.send(description_ack).await.unwrap();
    let mut description_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyDescriptionResponse {
            object_index: 1,
            property_id: 2,
            property_index: 1,
            type_: 2,
            max_count: 10,
            access: 0,
        }
        .encode(),
    );
    description_response.source = device_address;
    description_response.tpci = TpciFrame::DataConnected { sequence: 4 }.encode();
    server.send(description_response).await.unwrap();
    let (mut p2p, description) = reading.await.unwrap().unwrap();
    assert_eq!(description.max_count, 10);
    assert_eq!(description.pdt(), 2);
    let description_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        description_response_ack.tpci,
        TpciFrame::Ack { sequence: 4 }.encode()
    );

    assert!(
        p2p.read_property_values(
            crate::protocol::management::PropertyValueRead {
                object_index: 1,
                property_id: 2,
                count: 3,
                start_index: 4094,
            },
            15,
            Duration::from_secs(2),
        )
        .await
        .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
            .await
            .is_err()
    );

    let values = tokio::spawn(async move {
        let data = p2p
            .read_property_values(
                crate::protocol::management::PropertyValueRead {
                    object_index: 1,
                    property_id: 2,
                    count: 3,
                    start_index: 10,
                },
                15,
                Duration::from_secs(2),
            )
            .await?;
        Ok::<_, KnxError>((p2p, data))
    });
    let first_chunk = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::PropertyValueRead::decode(&first_chunk.payload).unwrap(),
        crate::protocol::management::PropertyValueRead {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 10,
        }
    );
    let mut first_chunk_ack = Telegram::individual_data(client_address, Vec::new());
    first_chunk_ack.source = device_address;
    first_chunk_ack.telegram_type = TelegramType::IndividualControl;
    first_chunk_ack.tpci = TpciFrame::Ack { sequence: 6 }.encode();
    server.send(first_chunk_ack).await.unwrap();
    let mut first_chunk_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 10,
            data: vec![0xAA, 0xBB],
        }
        .encode()
        .unwrap(),
    );
    first_chunk_response.source = device_address;
    first_chunk_response.tpci = TpciFrame::DataConnected { sequence: 5 }.encode();
    server.send(first_chunk_response).await.unwrap();
    let first_chunk_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        first_chunk_response_ack.tpci,
        TpciFrame::Ack { sequence: 5 }.encode()
    );
    let second_chunk = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::PropertyValueRead::decode(&second_chunk.payload).unwrap(),
        crate::protocol::management::PropertyValueRead {
            object_index: 1,
            property_id: 2,
            count: 2,
            start_index: 11,
        }
    );
    let mut second_chunk_ack = Telegram::individual_data(client_address, Vec::new());
    second_chunk_ack.source = device_address;
    second_chunk_ack.telegram_type = TelegramType::IndividualControl;
    second_chunk_ack.tpci = TpciFrame::Ack { sequence: 7 }.encode();
    server.send(second_chunk_ack).await.unwrap();
    let mut second_chunk_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 2,
            start_index: 11,
            data: vec![0xCC, 0xDD, 0xEE, 0xFF],
        }
        .encode()
        .unwrap(),
    );
    second_chunk_response.source = device_address;
    second_chunk_response.tpci = TpciFrame::DataConnected { sequence: 6 }.encode();
    server.send(second_chunk_response).await.unwrap();
    let (mut p2p, values) = values.await.unwrap().unwrap();
    assert_eq!(values, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    let second_chunk_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        second_chunk_response_ack.tpci,
        TpciFrame::Ack { sequence: 6 }.encode()
    );

    assert!(
        p2p.write_property_values(
            crate::protocol::management::PropertyValueWrite {
                object_index: 1,
                property_id: 2,
                count: 2,
                start_index: 30,
                data: vec![1, 2, 3],
            },
            10,
            Duration::from_secs(2),
        )
        .await
        .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
            .await
            .is_err()
    );

    let writes = tokio::spawn(async move {
        p2p.write_property_values(
            crate::protocol::management::PropertyValueWrite {
                object_index: 1,
                property_id: 2,
                count: 3,
                start_index: 20,
                data: vec![1, 2, 3, 4, 5, 6],
            },
            10,
            Duration::from_secs(2),
        )
        .await?;
        Ok::<_, KnxError>(p2p)
    });
    let first_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::PropertyValueWrite::decode(&first_write.payload).unwrap(),
        crate::protocol::management::PropertyValueWrite {
            object_index: 1,
            property_id: 2,
            count: 2,
            start_index: 20,
            data: vec![1, 2, 3, 4],
        }
    );
    let mut first_write_ack = Telegram::individual_data(client_address, Vec::new());
    first_write_ack.source = device_address;
    first_write_ack.telegram_type = TelegramType::IndividualControl;
    first_write_ack.tpci = TpciFrame::Ack { sequence: 8 }.encode();
    server.send(first_write_ack).await.unwrap();
    let mut first_write_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 2,
            start_index: 20,
            data: Vec::new(),
        }
        .encode()
        .unwrap(),
    );
    first_write_response.source = device_address;
    first_write_response.tpci = TpciFrame::DataConnected { sequence: 7 }.encode();
    server.send(first_write_response).await.unwrap();
    let first_write_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        first_write_response_ack.tpci,
        TpciFrame::Ack { sequence: 7 }.encode()
    );
    let second_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::PropertyValueWrite::decode(&second_write.payload).unwrap(),
        crate::protocol::management::PropertyValueWrite {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 22,
            data: vec![5, 6],
        }
    );
    let mut second_write_ack = Telegram::individual_data(client_address, Vec::new());
    second_write_ack.source = device_address;
    second_write_ack.telegram_type = TelegramType::IndividualControl;
    second_write_ack.tpci = TpciFrame::Ack { sequence: 9 }.encode();
    server.send(second_write_ack).await.unwrap();
    let mut second_write_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 22,
            data: Vec::new(),
        }
        .encode()
        .unwrap(),
    );
    second_write_response.source = device_address;
    second_write_response.tpci = TpciFrame::DataConnected { sequence: 8 }.encode();
    server.send(second_write_response).await.unwrap();
    let mut p2p = writes.await.unwrap().unwrap();
    let second_write_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        second_write_response_ack.tpci,
        TpciFrame::Ack { sequence: 8 }.encode()
    );

    let reading_memory = tokio::spawn(async move {
        let response = p2p
            .read_memory(
                crate::protocol::management::MemoryRead {
                    address: 0x1234,
                    count: 2,
                },
                Duration::from_secs(2),
            )
            .await?;
        Ok::<_, KnxError>((p2p, response))
    });
    let memory_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::MemoryRead::decode(&memory_read.payload).unwrap(),
        crate::protocol::management::MemoryRead {
            address: 0x1234,
            count: 2,
        }
    );
    let mut memory_read_ack = Telegram::individual_data(client_address, Vec::new());
    memory_read_ack.source = device_address;
    memory_read_ack.telegram_type = TelegramType::IndividualControl;
    memory_read_ack.tpci = TpciFrame::Ack { sequence: 10 }.encode();
    server.send(memory_read_ack).await.unwrap();
    let mut memory_read_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::MemoryResponse {
            address: 0x1234,
            count: 2,
            data: vec![0xAA, 0xBB],
        }
        .encode()
        .unwrap(),
    );
    memory_read_response.source = device_address;
    memory_read_response.tpci = TpciFrame::DataConnected { sequence: 9 }.encode();
    server.send(memory_read_response).await.unwrap();
    let (mut p2p, memory_response) = reading_memory.await.unwrap().unwrap();
    assert_eq!(memory_response.data, [0xAA, 0xBB]);
    let memory_read_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        memory_read_response_ack.tpci,
        TpciFrame::Ack { sequence: 9 }.encode()
    );

    let reading_user_memory = tokio::spawn(async move {
        let response = p2p
            .read_user_memory(
                crate::protocol::management::UserMemoryRead {
                    address: 0xABCDE,
                    count: 2,
                },
                Duration::from_secs(2),
            )
            .await?;
        Ok::<_, KnxError>((p2p, response))
    });
    let user_memory_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::UserMemoryRead::decode(&user_memory_read.payload)
            .unwrap()
            .address,
        0xABCDE
    );
    let mut user_memory_read_ack = Telegram::individual_data(client_address, Vec::new());
    user_memory_read_ack.source = device_address;
    user_memory_read_ack.telegram_type = TelegramType::IndividualControl;
    user_memory_read_ack.tpci = TpciFrame::Ack { sequence: 11 }.encode();
    server.send(user_memory_read_ack).await.unwrap();
    let mut user_memory_read_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::UserMemoryResponse {
            address: 0xABCDE,
            count: 2,
            data: vec![0xCC, 0xDD],
        }
        .encode()
        .unwrap(),
    );
    user_memory_read_response.source = device_address;
    user_memory_read_response.tpci = TpciFrame::DataConnected { sequence: 10 }.encode();
    server.send(user_memory_read_response).await.unwrap();
    let (mut p2p, user_memory_response) = reading_user_memory.await.unwrap().unwrap();
    assert_eq!(user_memory_response.data, [0xCC, 0xDD]);
    let user_memory_read_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        user_memory_read_response_ack.tpci,
        TpciFrame::Ack { sequence: 10 }.encode()
    );

    assert!(
        p2p.write_memory(crate::protocol::management::MemoryWrite {
            address: 0xffff,
            count: 2,
            data: vec![1, 2],
        })
        .await
        .is_err()
    );
    assert!(
        p2p.write_user_memory(crate::protocol::management::UserMemoryWrite {
            address: 0,
            count: 2,
            data: vec![1],
        })
        .await
        .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
            .await
            .is_err()
    );

    let writing_memory = tokio::spawn(async move {
        p2p.write_memory(crate::protocol::management::MemoryWrite {
            address: 0x2345,
            count: 2,
            data: vec![0x11, 0x22],
        })
        .await?;
        Ok::<_, KnxError>((p2p, ()))
    });
    let memory_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::MemoryWrite::decode(&memory_write.payload)
            .unwrap()
            .address,
        0x2345
    );
    let mut memory_write_ack = Telegram::individual_data(client_address, Vec::new());
    memory_write_ack.source = device_address;
    memory_write_ack.telegram_type = TelegramType::IndividualControl;
    memory_write_ack.tpci = TpciFrame::Ack { sequence: 12 }.encode();
    server.send(memory_write_ack).await.unwrap();
    let (mut p2p, _) = writing_memory.await.unwrap().unwrap();

    let writing_user_memory = tokio::spawn(async move {
        p2p.write_user_memory(crate::protocol::management::UserMemoryWrite {
            address: 0xFEDCB,
            count: 1,
            data: vec![0x33],
        })
        .await?;
        Ok::<_, KnxError>((p2p, ()))
    });
    let user_memory_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::UserMemoryWrite::decode(&user_memory_write.payload)
            .unwrap()
            .address,
        0xFEDCB
    );
    let mut user_memory_write_ack = Telegram::individual_data(client_address, Vec::new());
    user_memory_write_ack.source = device_address;
    user_memory_write_ack.telegram_type = TelegramType::IndividualControl;
    user_memory_write_ack.tpci = TpciFrame::Ack { sequence: 13 }.encode();
    server.send(user_memory_write_ack).await.unwrap();
    let (mut p2p, _) = writing_user_memory.await.unwrap().unwrap();

    assert!(
        p2p.read_memory_range(0, usize::MAX, 15, Duration::from_secs(2))
            .await
            .is_err()
    );
    assert!(
        p2p.read_user_memory_range(0, usize::MAX, 15, Duration::from_secs(2))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
            .await
            .is_err()
    );

    let reading_memory_range = tokio::spawn(async move {
        let data = p2p
            .read_memory_range(0x1000, 15, 15, Duration::from_secs(2))
            .await?;
        Ok::<_, KnxError>((p2p, data))
    });
    let first_memory_chunk = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::MemoryRead::decode(&first_memory_chunk.payload)
            .unwrap()
            .count,
        11
    );
    let mut first_memory_chunk_ack = Telegram::individual_data(client_address, Vec::new());
    first_memory_chunk_ack.source = device_address;
    first_memory_chunk_ack.telegram_type = TelegramType::IndividualControl;
    first_memory_chunk_ack.tpci = TpciFrame::Ack { sequence: 14 }.encode();
    server.send(first_memory_chunk_ack).await.unwrap();
    let mut first_memory_chunk_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::MemoryResponse {
            address: 0x1000,
            count: 11,
            data: vec![0x11; 11],
        }
        .encode()
        .unwrap(),
    );
    first_memory_chunk_response.source = device_address;
    first_memory_chunk_response.tpci = TpciFrame::DataConnected { sequence: 11 }.encode();
    server.send(first_memory_chunk_response).await.unwrap();
    let first_memory_chunk_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        first_memory_chunk_response_ack.tpci,
        TpciFrame::Ack { sequence: 11 }.encode()
    );
    let second_memory_chunk = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::MemoryRead::decode(&second_memory_chunk.payload)
            .unwrap()
            .count,
        4
    );
    let mut second_memory_chunk_ack = Telegram::individual_data(client_address, Vec::new());
    second_memory_chunk_ack.source = device_address;
    second_memory_chunk_ack.telegram_type = TelegramType::IndividualControl;
    second_memory_chunk_ack.tpci = TpciFrame::Ack { sequence: 15 }.encode();
    server.send(second_memory_chunk_ack).await.unwrap();
    let mut second_memory_chunk_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::MemoryResponse {
            address: 0x100B,
            count: 4,
            data: vec![0x22; 4],
        }
        .encode()
        .unwrap(),
    );
    second_memory_chunk_response.source = device_address;
    second_memory_chunk_response.tpci = TpciFrame::DataConnected { sequence: 12 }.encode();
    server.send(second_memory_chunk_response).await.unwrap();
    let (mut p2p, memory_data) = reading_memory_range.await.unwrap().unwrap();
    assert_eq!(&memory_data[..11], &[0x11; 11]);
    assert_eq!(&memory_data[11..], &[0x22; 4]);
    let second_memory_chunk_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        second_memory_chunk_response_ack.tpci,
        TpciFrame::Ack { sequence: 12 }.encode()
    );

    let reading_user_memory_range = tokio::spawn(async move {
        let data = p2p
            .read_user_memory_range(0xABCDE, 16, 15, Duration::from_secs(2))
            .await?;
        Ok::<_, KnxError>((p2p, data))
    });
    let first_user_memory_chunk = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::UserMemoryRead::decode(&first_user_memory_chunk.payload)
            .unwrap()
            .count,
        10
    );
    let mut first_user_memory_chunk_ack = Telegram::individual_data(client_address, Vec::new());
    first_user_memory_chunk_ack.source = device_address;
    first_user_memory_chunk_ack.telegram_type = TelegramType::IndividualControl;
    first_user_memory_chunk_ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
    server.send(first_user_memory_chunk_ack).await.unwrap();
    let mut first_user_memory_chunk_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::UserMemoryResponse {
            address: 0xABCDE,
            count: 10,
            data: vec![0x33; 10],
        }
        .encode()
        .unwrap(),
    );
    first_user_memory_chunk_response.source = device_address;
    first_user_memory_chunk_response.tpci = TpciFrame::DataConnected { sequence: 13 }.encode();
    server.send(first_user_memory_chunk_response).await.unwrap();
    let first_user_memory_chunk_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        first_user_memory_chunk_response_ack.tpci,
        TpciFrame::Ack { sequence: 13 }.encode()
    );
    let second_user_memory_chunk =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        crate::protocol::management::UserMemoryRead::decode(&second_user_memory_chunk.payload)
            .unwrap()
            .count,
        6
    );
    let mut second_user_memory_chunk_ack = Telegram::individual_data(client_address, Vec::new());
    second_user_memory_chunk_ack.source = device_address;
    second_user_memory_chunk_ack.telegram_type = TelegramType::IndividualControl;
    second_user_memory_chunk_ack.tpci = TpciFrame::Ack { sequence: 1 }.encode();
    server.send(second_user_memory_chunk_ack).await.unwrap();
    let mut second_user_memory_chunk_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::UserMemoryResponse {
            address: 0xABCE8,
            count: 6,
            data: vec![0x44; 6],
        }
        .encode()
        .unwrap(),
    );
    second_user_memory_chunk_response.source = device_address;
    second_user_memory_chunk_response.tpci = TpciFrame::DataConnected { sequence: 14 }.encode();
    server
        .send(second_user_memory_chunk_response)
        .await
        .unwrap();
    let (mut p2p, user_memory_data) = reading_user_memory_range.await.unwrap().unwrap();
    assert_eq!(&user_memory_data[..10], &[0x33; 10]);
    assert_eq!(&user_memory_data[10..], &[0x44; 6]);
    let second_user_memory_chunk_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        second_user_memory_chunk_response_ack.tpci,
        TpciFrame::Ack { sequence: 14 }.encode()
    );

    assert!(
        p2p.write_memory_range(0xFFFF, &[1, 2], 15, false, Duration::from_secs(2),)
            .await
            .is_err()
    );
    assert!(
        p2p.write_user_memory_range(0x0F_FFFF, &[1, 2], 15, false, Duration::from_secs(2),)
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
            .await
            .is_err()
    );

    let writing_user_memory_range = tokio::spawn(async move {
        p2p.write_user_memory_range(
            0xABC00,
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            15,
            false,
            Duration::from_secs(2),
        )
        .await?;
        Ok::<_, KnxError>(p2p)
    });
    let first_user_memory_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::UserMemoryWrite::decode(&first_user_memory_write.payload)
            .unwrap(),
        crate::protocol::management::UserMemoryWrite {
            address: 0xABC00,
            count: 10,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        }
    );
    let mut first_user_memory_write_ack = Telegram::individual_data(client_address, Vec::new());
    first_user_memory_write_ack.source = device_address;
    first_user_memory_write_ack.telegram_type = TelegramType::IndividualControl;
    first_user_memory_write_ack.tpci = TpciFrame::Ack { sequence: 2 }.encode();
    server.send(first_user_memory_write_ack).await.unwrap();
    let second_user_memory_write =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        crate::protocol::management::UserMemoryWrite::decode(&second_user_memory_write.payload)
            .unwrap(),
        crate::protocol::management::UserMemoryWrite {
            address: 0xABC0A,
            count: 6,
            data: vec![11, 12, 13, 14, 15, 16],
        }
    );
    let mut second_user_memory_write_ack = Telegram::individual_data(client_address, Vec::new());
    second_user_memory_write_ack.source = device_address;
    second_user_memory_write_ack.telegram_type = TelegramType::IndividualControl;
    second_user_memory_write_ack.tpci = TpciFrame::Ack { sequence: 3 }.encode();
    server.send(second_user_memory_write_ack).await.unwrap();
    let mut p2p = writing_user_memory_range.await.unwrap().unwrap();

    let writing_verified_memory = tokio::spawn(async move {
        let result = p2p
            .write_memory_range(
                0x2000,
                &(1..=18).collect::<Vec<u8>>(),
                10,
                true,
                Duration::from_secs(2),
            )
            .await;
        Ok::<_, KnxError>((p2p, result))
    });
    let first_verified_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::MemoryWrite::decode(&first_verified_write.payload)
            .unwrap()
            .count,
        6
    );
    let mut first_verified_write_ack = Telegram::individual_data(client_address, Vec::new());
    first_verified_write_ack.source = device_address;
    first_verified_write_ack.telegram_type = TelegramType::IndividualControl;
    first_verified_write_ack.tpci = TpciFrame::Ack { sequence: 4 }.encode();
    server.send(first_verified_write_ack).await.unwrap();
    let first_verification_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::MemoryRead::decode(&first_verification_read.payload)
            .unwrap()
            .address,
        0x2000
    );
    let mut first_verification_read_ack = Telegram::individual_data(client_address, Vec::new());
    first_verification_read_ack.source = device_address;
    first_verification_read_ack.telegram_type = TelegramType::IndividualControl;
    first_verification_read_ack.tpci = TpciFrame::Ack { sequence: 5 }.encode();
    server.send(first_verification_read_ack).await.unwrap();
    let mut first_verification_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::MemoryResponse {
            address: 0x2000,
            count: 6,
            data: vec![1, 2, 3, 4, 5, 6],
        }
        .encode()
        .unwrap(),
    );
    first_verification_response.source = device_address;
    first_verification_response.tpci = TpciFrame::DataConnected { sequence: 15 }.encode();
    server.send(first_verification_response).await.unwrap();
    let first_verification_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        first_verification_response_ack.tpci,
        TpciFrame::Ack { sequence: 15 }.encode()
    );
    let second_verified_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::MemoryWrite::decode(&second_verified_write.payload)
            .unwrap()
            .address,
        0x2006
    );
    let mut second_verified_write_ack = Telegram::individual_data(client_address, Vec::new());
    second_verified_write_ack.source = device_address;
    second_verified_write_ack.telegram_type = TelegramType::IndividualControl;
    second_verified_write_ack.tpci = TpciFrame::Ack { sequence: 6 }.encode();
    server.send(second_verified_write_ack).await.unwrap();
    let second_verification_read =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        crate::protocol::management::MemoryRead::decode(&second_verification_read.payload)
            .unwrap()
            .address,
        0x2006
    );
    let mut second_verification_read_ack = Telegram::individual_data(client_address, Vec::new());
    second_verification_read_ack.source = device_address;
    second_verification_read_ack.telegram_type = TelegramType::IndividualControl;
    second_verification_read_ack.tpci = TpciFrame::Ack { sequence: 7 }.encode();
    server.send(second_verification_read_ack).await.unwrap();
    let mut mismatched_verification_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::MemoryResponse {
            address: 0x2006,
            count: 6,
            data: vec![0xFF; 6],
        }
        .encode()
        .unwrap(),
    );
    mismatched_verification_response.source = device_address;
    mismatched_verification_response.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
    server.send(mismatched_verification_response).await.unwrap();
    let (mut p2p, result) = writing_verified_memory.await.unwrap().unwrap();
    assert!(result.is_err());
    let mismatched_verification_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        mismatched_verification_ack.tpci,
        TpciFrame::Ack { sequence: 0 }.encode()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
            .await
            .is_err()
    );

    let verifying_property = tokio::spawn(async move {
        let result = p2p
            .verify_property_values(
                crate::protocol::management::PropertyValueRead {
                    object_index: 1,
                    property_id: 2,
                    count: 1,
                    start_index: 30,
                },
                &[0x5A],
                15,
                Duration::from_secs(2),
            )
            .await;
        Ok::<_, KnxError>((p2p, result))
    });
    let property_verification_read =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        crate::protocol::management::PropertyValueRead::decode(&property_verification_read.payload)
            .unwrap()
            .start_index,
        30
    );
    let mut property_verification_ack = Telegram::individual_data(client_address, Vec::new());
    property_verification_ack.source = device_address;
    property_verification_ack.telegram_type = TelegramType::IndividualControl;
    property_verification_ack.tpci = TpciFrame::Ack { sequence: 8 }.encode();
    server.send(property_verification_ack).await.unwrap();
    let mut property_verification_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 30,
            data: vec![0x5A],
        }
        .encode()
        .unwrap(),
    );
    property_verification_response.source = device_address;
    property_verification_response.tpci = TpciFrame::DataConnected { sequence: 1 }.encode();
    server.send(property_verification_response).await.unwrap();
    let (mut p2p, result) = verifying_property.await.unwrap().unwrap();
    result.unwrap();
    let property_verification_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        property_verification_response_ack.tpci,
        TpciFrame::Ack { sequence: 1 }.encode()
    );

    let verifying_mismatched_property = tokio::spawn(async move {
        let result = p2p
            .verify_property_values(
                crate::protocol::management::PropertyValueRead {
                    object_index: 1,
                    property_id: 2,
                    count: 1,
                    start_index: 31,
                },
                &[0x00],
                15,
                Duration::from_secs(2),
            )
            .await;
        Ok::<_, KnxError>((p2p, result))
    });
    let mismatched_property_read =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        crate::protocol::management::PropertyValueRead::decode(&mismatched_property_read.payload)
            .unwrap()
            .start_index,
        31
    );
    let mut mismatched_property_ack = Telegram::individual_data(client_address, Vec::new());
    mismatched_property_ack.source = device_address;
    mismatched_property_ack.telegram_type = TelegramType::IndividualControl;
    mismatched_property_ack.tpci = TpciFrame::Ack { sequence: 9 }.encode();
    server.send(mismatched_property_ack).await.unwrap();
    let mut mismatched_property_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 31,
            data: vec![0xFE],
        }
        .encode()
        .unwrap(),
    );
    mismatched_property_response.source = device_address;
    mismatched_property_response.tpci = TpciFrame::DataConnected { sequence: 2 }.encode();
    server.send(mismatched_property_response).await.unwrap();
    let (mut p2p, result) = verifying_mismatched_property.await.unwrap().unwrap();
    assert!(result.is_err());
    let mismatched_property_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        mismatched_property_response_ack.tpci,
        TpciFrame::Ack { sequence: 2 }.encode()
    );

    let scanning = tokio::spawn(async move {
        let objects = p2p.scan_interface_objects(Duration::from_secs(2)).await?;
        Ok::<_, KnxError>((p2p, objects))
    });
    let object_probe = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::PropertyDescriptionRead::decode(&object_probe.payload)
            .unwrap(),
        crate::protocol::management::PropertyDescriptionRead {
            object_index: 0,
            property_id: 0,
            property_index: 0,
        }
    );
    let mut object_probe_ack = Telegram::individual_data(client_address, Vec::new());
    object_probe_ack.source = device_address;
    object_probe_ack.telegram_type = TelegramType::IndividualControl;
    object_probe_ack.tpci = TpciFrame::Ack { sequence: 10 }.encode();
    server.send(object_probe_ack).await.unwrap();
    let mut object_probe_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyDescriptionResponse {
            object_index: 0,
            property_id: 1,
            property_index: 0,
            type_: 0x82,
            max_count: 1,
            access: 0,
        }
        .encode(),
    );
    object_probe_response.source = device_address;
    object_probe_response.tpci = TpciFrame::DataConnected { sequence: 3 }.encode();
    server.send(object_probe_response).await.unwrap();
    let object_probe_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        object_probe_response_ack.tpci,
        TpciFrame::Ack { sequence: 3 }.encode()
    );

    let object_type_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::PropertyValueRead::decode(&object_type_read.payload).unwrap(),
        crate::protocol::management::PropertyValueRead {
            object_index: 0,
            property_id: 1,
            count: 1,
            start_index: 1,
        }
    );
    let mut object_type_ack = Telegram::individual_data(client_address, Vec::new());
    object_type_ack.source = device_address;
    object_type_ack.telegram_type = TelegramType::IndividualControl;
    object_type_ack.tpci = TpciFrame::Ack { sequence: 11 }.encode();
    server.send(object_type_ack).await.unwrap();
    let mut object_type_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::PropertyValueResponse {
            object_index: 0,
            property_id: 1,
            count: 1,
            start_index: 1,
            data: vec![0, 0x11],
        }
        .encode()
        .unwrap(),
    );
    object_type_response.source = device_address;
    object_type_response.tpci = TpciFrame::DataConnected { sequence: 4 }.encode();
    server.send(object_type_response).await.unwrap();
    let object_type_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        object_type_response_ack.tpci,
        TpciFrame::Ack { sequence: 4 }.encode()
    );

    for (request_sequence, response_sequence, property_index, property_id) in
        [(12, 5, 0, 2), (13, 6, 1, 3), (14, 7, 2, 0), (15, 8, 0, 0)]
    {
        let property_probe = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        let request =
            crate::protocol::management::PropertyDescriptionRead::decode(&property_probe.payload)
                .unwrap();
        assert_eq!(
            request.object_index,
            if request_sequence == 15 { 1 } else { 0 }
        );
        assert_eq!(request.property_id, 0);
        assert_eq!(request.property_index, property_index);
        let mut property_probe_ack = Telegram::individual_data(client_address, Vec::new());
        property_probe_ack.source = device_address;
        property_probe_ack.telegram_type = TelegramType::IndividualControl;
        property_probe_ack.tpci = TpciFrame::Ack {
            sequence: request_sequence,
        }
        .encode();
        server.send(property_probe_ack).await.unwrap();
        let mut property_probe_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyDescriptionResponse {
                object_index: request.object_index,
                property_id,
                property_index,
                type_: 0x82,
                max_count: 4,
                access: 0,
            }
            .encode(),
        );
        property_probe_response.source = device_address;
        property_probe_response.tpci = TpciFrame::DataConnected {
            sequence: response_sequence,
        }
        .encode();
        server.send(property_probe_response).await.unwrap();
        let property_probe_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            property_probe_response_ack.tpci,
            TpciFrame::Ack {
                sequence: response_sequence,
            }
            .encode()
        );
    }
    let (mut p2p, objects) = scanning.await.unwrap().unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].object_index, 0);
    assert_eq!(objects[0].object_type, 0x0011);
    assert_eq!(objects[0].properties.len(), 2);
    assert_eq!(objects[0].properties[0].property_id, 2);
    assert_eq!(objects[0].properties[1].property_id, 3);

    assert!(
        p2p.master_reset(0, 2, Duration::from_secs(2))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
            .await
            .is_err()
    );

    let resetting = tokio::spawn(async move {
        let response = p2p.master_reset(1, 2, Duration::from_secs(2)).await?;
        Ok::<_, KnxError>((p2p, response))
    });
    let reset_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reset_request.payload, [0x03, 0x81, 1, 2]);
    assert_eq!(
        reset_request.tpci,
        TpciFrame::DataConnected { sequence: 0 }.encode()
    );
    let mut reset_ack = Telegram::individual_data(client_address, Vec::new());
    reset_ack.source = device_address;
    reset_ack.telegram_type = TelegramType::IndividualControl;
    reset_ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
    server.send(reset_ack).await.unwrap();
    let mut reset_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::RestartMasterResetResponse {
            error_code: 0,
            process_time: 10,
        }
        .encode(),
    );
    reset_response.source = device_address;
    reset_response.tpci = TpciFrame::DataConnected { sequence: 9 }.encode();
    server.send(reset_response).await.unwrap();
    let (mut p2p, reset_response) = resetting.await.unwrap().unwrap();
    assert_eq!(reset_response.process_time, 10);
    let reset_response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reset_response_ack.tpci,
        TpciFrame::Ack { sequence: 9 }.encode()
    );

    let resetting_with_error = tokio::spawn(async move {
        let result = p2p.master_reset(2, 0, Duration::from_secs(2)).await;
        Ok::<_, KnxError>((p2p, result))
    });
    let reset_error_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reset_error_request.payload, [0x03, 0x81, 2, 0]);
    let mut reset_error_ack = Telegram::individual_data(client_address, Vec::new());
    reset_error_ack.source = device_address;
    reset_error_ack.telegram_type = TelegramType::IndividualControl;
    reset_error_ack.tpci = TpciFrame::Ack { sequence: 1 }.encode();
    server.send(reset_error_ack).await.unwrap();
    let mut reset_error_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::RestartMasterResetResponse {
            error_code: 1,
            process_time: 0,
        }
        .encode(),
    );
    reset_error_response.source = device_address;
    reset_error_response.tpci = TpciFrame::DataConnected { sequence: 10 }.encode();
    server.send(reset_error_response).await.unwrap();
    let (mut p2p, result) = resetting_with_error.await.unwrap().unwrap();
    assert!(result.is_err());
    let reset_error_response_ack =
        tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        reset_error_response_ack.tpci,
        TpciFrame::Ack { sequence: 10 }.encode()
    );

    assert!(
        p2p.read_device_descriptor(0x40, Duration::from_secs(2))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
            .await
            .is_err()
    );

    let reading_mask = tokio::spawn(async move {
        let version = p2p.read_mask_version(Duration::from_secs(2)).await?;
        Ok::<_, KnxError>((p2p, version))
    });
    let descriptor_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(descriptor_read.payload, [0x03, 0x00]);
    let mut descriptor_ack = Telegram::individual_data(client_address, Vec::new());
    descriptor_ack.source = device_address;
    descriptor_ack.telegram_type = TelegramType::IndividualControl;
    descriptor_ack.tpci = TpciFrame::Ack { sequence: 2 }.encode();
    server.send(descriptor_ack).await.unwrap();
    let mut descriptor_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::DeviceDescriptorResponse {
            descriptor: 0,
            data: vec![0x06, 0x10],
        }
        .encode(),
    );
    descriptor_response.source = device_address;
    descriptor_response.tpci = TpciFrame::DataConnected { sequence: 11 }.encode();
    server.send(descriptor_response).await.unwrap();
    let (mut p2p, mask_version) = reading_mask.await.unwrap().unwrap();
    assert_eq!(mask_version, 0x0610);
    let descriptor_response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        descriptor_response_ack.tpci,
        TpciFrame::Ack { sequence: 11 }.encode()
    );

    let authorizing = tokio::spawn(async move {
        let response = p2p.authorize(0x1234_5678, Duration::from_secs(2)).await?;
        Ok::<_, KnxError>((p2p, response))
    });
    let authorize_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::AuthorizeRequest::decode(&authorize_request.payload)
            .unwrap()
            .key,
        0x1234_5678
    );
    let mut authorize_ack = Telegram::individual_data(client_address, Vec::new());
    authorize_ack.source = device_address;
    authorize_ack.telegram_type = TelegramType::IndividualControl;
    authorize_ack.tpci = TpciFrame::Ack { sequence: 3 }.encode();
    server.send(authorize_ack).await.unwrap();
    let mut authorize_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::AuthorizeResponse { level: 2 }.encode(),
    );
    authorize_response.source = device_address;
    authorize_response.tpci = TpciFrame::DataConnected { sequence: 12 }.encode();
    server.send(authorize_response).await.unwrap();
    let (mut p2p, authorize_response) = authorizing.await.unwrap().unwrap();
    assert_eq!(authorize_response.level, 2);
    let authorize_response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        authorize_response_ack.tpci,
        TpciFrame::Ack { sequence: 12 }.encode()
    );

    let authorizing_best = tokio::spawn(async move {
        let level = p2p
            .authorize_best(0x8765_4321, Duration::from_secs(2))
            .await?;
        Ok::<_, KnxError>((p2p, level))
    });
    let free_key_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::AuthorizeRequest::decode(&free_key_request.payload)
            .unwrap()
            .key,
        crate::protocol::management::FREE_ACCESS_KEY
    );
    let mut free_key_ack = Telegram::individual_data(client_address, Vec::new());
    free_key_ack.source = device_address;
    free_key_ack.telegram_type = TelegramType::IndividualControl;
    free_key_ack.tpci = TpciFrame::Ack { sequence: 4 }.encode();
    server.send(free_key_ack).await.unwrap();
    let mut free_key_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::AuthorizeResponse { level: 3 }.encode(),
    );
    free_key_response.source = device_address;
    free_key_response.tpci = TpciFrame::DataConnected { sequence: 13 }.encode();
    server.send(free_key_response).await.unwrap();
    let free_key_response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        free_key_response_ack.tpci,
        TpciFrame::Ack { sequence: 13 }.encode()
    );

    let client_key_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::protocol::management::AuthorizeRequest::decode(&client_key_request.payload)
            .unwrap()
            .key,
        0x8765_4321
    );
    let mut client_key_ack = Telegram::individual_data(client_address, Vec::new());
    client_key_ack.source = device_address;
    client_key_ack.telegram_type = TelegramType::IndividualControl;
    client_key_ack.tpci = TpciFrame::Ack { sequence: 5 }.encode();
    server.send(client_key_ack).await.unwrap();
    let mut client_key_response = Telegram::individual_data(
        client_address,
        crate::protocol::management::AuthorizeResponse { level: 1 }.encode(),
    );
    client_key_response.source = device_address;
    client_key_response.tpci = TpciFrame::DataConnected { sequence: 14 }.encode();
    server.send(client_key_response).await.unwrap();
    let (mut p2p, best_level) = authorizing_best.await.unwrap().unwrap();
    assert_eq!(best_level, 1);
    let client_key_response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        client_key_response_ack.tpci,
        TpciFrame::Ack { sequence: 14 }.encode()
    );

    p2p.close().await.unwrap();
    let disconnect = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disconnect.tpci, TpciFrame::Disconnect.encode());
    knx.shutdown().await.unwrap();
    server.shutdown();
}

#[test]
fn individual_apdus_preserve_connectionless_and_connected_tpci() {
    use crate::protocol::knxip::{KnxIpFrame, TunnellingRequest};
    let destination = crate::protocol::IndividualAddress::new(1, 2, 3);
    let source = crate::protocol::IndividualAddress::new(1, 2, 4);
    let apdu = vec![0x02, 0xC8, 1, 2, 0x42];
    let telegram = Telegram::individual_data(destination, apdu.clone());
    let connection: Arc<dyn Connection> =
        Arc::new(Tunnel::new_udp("127.0.0.1:3671".parse().unwrap()));
    let wire = Knx::build_outgoing_frame_for_connection(&telegram, &connection).unwrap();
    let frame = KnxIpFrame::parse(&wire).unwrap();
    let request = TunnellingRequest::parse(&frame.body).unwrap();
    let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
    assert_eq!(cemi.dest_addr, Address::Individual(destination));
    assert_eq!(cemi.tpci, 0);
    assert_eq!(cemi.apci_data, apdu);
    let received = Knx::parse_cemi_to_telegram(&request.raw_cemi).unwrap();
    assert_eq!(received.telegram_type, TelegramType::IndividualData);
    assert_eq!(received.tpci, 0);
    assert_eq!(received.payload, apdu);

    let mut connected = cemi;
    connected.tpci = 0x44;
    let received = Telegram::from_cemi_frame(connected).unwrap();
    assert_eq!(received.tpci, 0x44);
    assert_eq!(received.payload, apdu);
    let wire = Knx::build_outgoing_frame_for_connection(&received, &connection).unwrap();
    let frame = KnxIpFrame::parse(&wire).unwrap();
    let request = TunnellingRequest::parse(&frame.body).unwrap();
    let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
    assert_eq!(cemi.tpci, 0x44);
    assert_eq!(cemi.apci_data, apdu);

    for tpci in [0x80, 0x81, 0xC2, 0xC3] {
        let mut control = CemiFrame::new(
            crate::protocol::cemi::MessageCode::LDataInd,
            source,
            Address::Individual(destination),
            Vec::new(),
        );
        control.tpci = tpci;
        let received = Telegram::from_cemi_frame(control).unwrap();
        assert_eq!(received.telegram_type, TelegramType::IndividualControl);
        assert_eq!(received.tpci, tpci);
        let wire = Knx::build_outgoing_frame_for_connection(&received, &connection).unwrap();
        let frame = KnxIpFrame::parse(&wire).unwrap();
        let request = TunnellingRequest::parse(&frame.body).unwrap();
        let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
        assert_eq!(cemi.tpci, tpci);
        assert!(cemi.apci_data.is_empty());
    }
}

#[cfg(feature = "dpt")]
#[test]
fn octet_telegrams_preserve_full_octets_and_service_on_wire() {
    use crate::protocol::dpt::{DPTSwitch, DptValue, Scaling};
    use crate::protocol::knxip::{KnxIpFrame, TunnellingRequest};
    let address = RawGroupAddress::try_from_raw(0x1234).unwrap();
    let connection: Arc<dyn Connection> =
        Arc::new(Tunnel::new_udp("127.0.0.1:3671".parse().unwrap()));
    for byte in 0..=u8::MAX {
        let value = Scaling::from_bytes(&[byte]).unwrap();
        for (telegram, service_bits) in [
            (
                Telegram::group_write_value(address, value.clone()).unwrap(),
                0x80,
            ),
            (
                Telegram::group_response_value(address, value).unwrap(),
                0x40,
            ),
        ] {
            let wire = Knx::build_outgoing_frame_for_connection(&telegram, &connection).unwrap();
            let frame = KnxIpFrame::parse(&wire).unwrap();
            let request = TunnellingRequest::parse(&frame.body).unwrap();
            let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
            assert_eq!(cemi.message_code, MessageCode::LDataReq);
            assert_eq!(cemi.apci_data, [service_bits, byte]);
            let received = Knx::parse_cemi_to_telegram(&request.raw_cemi).unwrap();
            assert_eq!(received.payload, [byte]);
            assert!(!received.compact_payload);
            assert_eq!(received.telegram_type, telegram.telegram_type);
        }
    }
    let switch = Telegram::group_write_value::<DPTSwitch>(address, true).unwrap();
    assert!(switch.compact_payload);
    assert_eq!(switch.encode_apdu(), [0x81]);
    assert_eq!(Telegram::group_read(address).encode_apdu(), [0]);
}

#[cfg(feature = "secure")]
#[tokio::test]
async fn data_secure_routing_drops_replayed_group_writes() {
    use crate::protocol::dpt::DPTSwitch;
    use crate::protocol::knxip::KnxIpFrame;
    use crate::security::{KeyRing, SecurityKey};
    use std::net::Ipv4Addr;
    use tokio::time::timeout;

    let local = Ipv4Addr::LOCALHOST;
    let address: RawGroupAddress = "13/7/226".parse().unwrap();
    let ordinary: RawGroupAddress = "13/7/225".parse().unwrap();
    let source: IndividualAddress = "14.15.226".parse().unwrap();
    let keys = || {
        let mut keys = KeyRing::new();
        keys.add_group_key(
            address,
            SecurityKey::from_hex("00112233445566778899aabbccddeeff").unwrap(),
        );
        keys.set_sequence_number(source, 0);
        keys
    };

    let sender = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .local_ip(local.into())
        .individual_address(source)
        .build()
        .await
        .unwrap();
    let receiver = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .local_ip(local.into())
        .individual_address("14.15.227".parse().unwrap())
        .build()
        .await
        .unwrap();
    sender.set_data_secure_keyring(keys()).await;
    receiver.set_data_secure_keyring(keys()).await;
    let observer = RoutingConnection::new_with_interface(local).await.unwrap();
    let (events, mut callbacks) = mpsc::channel(4);
    receiver
        .register_telegram_callback_filtered(
            LoopbackCallback(events),
            TelegramFilter::GroupAddresses(vec![address, ordinary]),
            false,
        )
        .await;
    for knx in [&sender, &receiver] {
        knx.connect().await.unwrap();
        knx.start().await.unwrap();
    }

    let switch = sender.group_address::<DPTSwitch>(address).unwrap();
    switch.write(true).await.unwrap();
    let secured = timeout(Duration::from_secs(2), async {
        loop {
            let datagram = observer.recv().await.unwrap();
            let frame = KnxIpFrame::parse(&datagram).unwrap();
            let cemi = CemiFrame::parse(&frame.body).unwrap();
            if cemi.source_addr == source && cemi.dest_addr == Address::Group(address) {
                assert_eq!(&cemi.apci_data[..2], &[0xF1, 0x10]);
                break datagram;
            }
        }
    })
    .await
    .unwrap();
    let first = timeout(Duration::from_secs(2), callbacks.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.source, source);
    assert_eq!(first.payload, [1]);

    receiver.set_data_secure_keyring(keys()).await;
    observer.send(&secured).await.unwrap();
    assert!(
        timeout(Duration::from_millis(100), callbacks.recv())
            .await
            .is_err()
    );

    for destination in [address, ordinary] {
        let cemi = CemiFrame::new(
            MessageCode::LDataInd,
            source,
            Address::Group(destination),
            vec![0x81],
        );
        observer
            .send_routing_indication(&cemi.serialize().unwrap())
            .await
            .unwrap();
    }
    assert_eq!(
        timeout(Duration::from_secs(2), callbacks.recv())
            .await
            .unwrap()
            .unwrap()
            .destination,
        Address::Group(ordinary)
    );

    switch.write(false).await.unwrap();
    let next = timeout(Duration::from_secs(2), callbacks.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.payload, [0]);

    sender.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
    observer.close().await.unwrap();
}

#[cfg(feature = "secure")]
#[tokio::test]
async fn data_secure_group_telegrams_are_wrapped_and_replay_checked() {
    use crate::protocol::knxip::{KnxIpFrame, TunnellingRequest};
    use crate::security::{KeyRing, SecurityKey};
    use std::sync::atomic::AtomicU64;

    let destination = RawGroupAddress::try_from_raw(0x0400).unwrap();
    let source = crate::protocol::IndividualAddress::new(5, 0, 1);
    let connection: Arc<dyn Connection> =
        Arc::new(Tunnel::new_udp("127.0.0.1:3671".parse().unwrap()));
    let mut keyring = KeyRing::new();
    keyring.add_group_key(
        destination,
        SecurityKey::from_hex("dfdf23a59fbb40404091d1c162087e8b").unwrap(),
    );
    keyring.set_sequence_number(source, 160170101606);
    let keyring = RwLock::new(Some(keyring));
    let sequence = AtomicU64::new(160170101607);
    let mut telegram = Telegram::group_read(destination);
    telegram.source = source;

    let wire = Knx::build_outgoing_frame_with_security(&telegram, &connection, &keyring, &sequence)
        .await
        .unwrap();
    let knxip = KnxIpFrame::parse(&wire).unwrap();
    let request = TunnellingRequest::parse(&knxip.body).unwrap();
    let secure = CemiFrame::parse(&request.raw_cemi).unwrap();
    assert_eq!(secure.tpci, 0x03);
    assert_eq!(sequence.load(Ordering::SeqCst), 160170101608);
    assert_eq!(
        secure.apci_data,
        [
            0xF1, 0x10, 0x00, 0x25, 0x4A, 0xE1, 0xCB, 0x67, 0xCD, 0x18, 0x4A, 0xFE, 0x57, 0x44
        ]
    );

    let mut unknown_sender_keys = KeyRing::new();
    unknown_sender_keys.add_group_key(
        destination,
        SecurityKey::from_hex("dfdf23a59fbb40404091d1c162087e8b").unwrap(),
    );
    assert!(
        Knx::parse_cemi_to_telegram_with_security(
            &request.raw_cemi,
            &RwLock::new(Some(unknown_sender_keys)),
        )
        .await
        .is_err()
    );

    let telegram = Knx::parse_cemi_to_telegram_with_security(&request.raw_cemi, &keyring)
        .await
        .unwrap();
    assert_eq!(telegram.source, source);
    assert_eq!(telegram.destination, Address::Group(destination));
    assert_eq!(telegram.telegram_type, TelegramType::GroupValueRead);

    assert!(
        Knx::parse_cemi_to_telegram_with_security(&request.raw_cemi, &keyring)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn test_knx_telegram_callback_integration() {
    // Create Knx instance with telegram callback
    let callback_counter = Arc::new(AtomicUsize::new(0));
    let received_telegrams = Arc::new(tokio::sync::RwLock::new(Vec::new()));

    let counter_clone = callback_counter.clone();
    let telegrams_clone = received_telegrams.clone();

    // Create Knx instance first, then register callback at runtime
    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    // Register telegram callback using sync method
    let _handle = knx
        .event_handler
        .register_telegram_callback_sync(move |telegram: &Telegram| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            let telegrams_clone = telegrams_clone.clone();
            let owned_telegram = telegram.clone();
            tokio::spawn(async move {
                telegrams_clone.write().await.push(owned_telegram);
            });
        })
        .await;

    // Verify callback was registered
    assert_eq!(knx.telegram_callback_count().await, 1);

    // Create test telegram
    let telegram = Telegram {
        source: IndividualAddress::new(1, 1, 1),
        destination: Address::Group(RawGroupAddress::new(
            MainGroup::new(0),
            MiddleGroup::new(1),
            1,
        )),
        payload: vec![0x01, 0x02],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        compact_payload: true,
        tpci: 0,
        timestamp: std::time::SystemTime::now(),
    };

    // Trigger telegram notification
    knx.test_notify_telegram_received(&telegram).await;

    // Give async tasks time to complete
    sleep(Duration::from_millis(50)).await;

    // Verify callback was invoked
    assert_eq!(callback_counter.load(Ordering::SeqCst), 1);

    let telegrams = received_telegrams.read().await;
    assert_eq!(telegrams.len(), 1);
    assert_eq!(telegrams[0].source, telegram.source);
    assert_eq!(telegrams[0].destination, telegram.destination);
    assert_eq!(telegrams[0].payload, telegram.payload);
}

/// Incoming telegrams for group addresses nobody registered a DPT for
/// must still flow through normally — the registry is opt-in and must
/// never gate delivery — and every decode entry point must fail soft
/// (`None`/`Err`, never a panic) rather than assume the sender agrees
/// with the registry.
#[cfg(feature = "dpt")]
#[tokio::test]
async fn test_incoming_telegram_for_unregistered_address_is_safe() {
    use crate::protocol::dpt::DPTSwitch;

    let callback_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = callback_counter.clone();

    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    // Only address 0/1/1 gets a DPT binding; 0/2/2 never does.
    let registered = RawGroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);
    let unregistered = RawGroupAddress::new(MainGroup::new(0), MiddleGroup::new(2), 2);
    let switch = knx.group_address::<DPTSwitch>(registered).unwrap();

    let _handle = knx
        .event_handler
        .register_telegram_callback_sync(move |_telegram: &Telegram| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    let telegram_for_unregistered = Telegram {
        source: IndividualAddress::new(1, 1, 1),
        destination: Address::Group(unregistered),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        compact_payload: true,
        tpci: 0,
        timestamp: std::time::SystemTime::now(),
    };

    // Delivery to callbacks doesn't consult the registry at all, so an
    // unregistered destination changes nothing about receiving it.
    knx.test_notify_telegram_received(&telegram_for_unregistered)
        .await;
    sleep(Duration::from_millis(50)).await;
    assert_eq!(callback_counter.load(Ordering::SeqCst), 1);

    // The generic decode path fails soft instead of panicking.
    assert!(knx.group_address_dpt(unregistered).is_none());
    assert!(knx.decode_group_value(&telegram_for_unregistered).is_none());

    // A handle for a *different, registered* address also just returns
    // `None` for a telegram it doesn't recognize — it doesn't panic
    // trying to decode bytes that were never meant for its DPT.
    assert!(switch.decode(&telegram_for_unregistered).is_none());

    // Meanwhile the registered address keeps working normally.
    let telegram_for_registered = Telegram {
        source: IndividualAddress::new(1, 1, 1),
        destination: Address::Group(registered),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        compact_payload: true,
        tpci: 0,
        timestamp: std::time::SystemTime::now(),
    };
    assert!(
        switch
            .decode(&telegram_for_registered)
            .unwrap()
            .unwrap()
            .value()
    );
    assert!(
        knx.decode_group_value(&telegram_for_registered)
            .unwrap()
            .unwrap()
            .as_bool()
            .unwrap()
    );
}

/// A real `GroupValueWrite`/`GroupValueResponse` for a registered
/// address updates its cached state and clears its staleness; a
/// `GroupValueRead` (no value) does neither.
#[cfg(feature = "dpt")]
#[tokio::test]
async fn incoming_group_value_clears_staleness_and_caches_value() {
    use crate::protocol::dpt::DPTSwitch;

    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    let address = RawGroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);
    knx.group_address::<DPTSwitch>(address).unwrap();
    knx.set_group_address_refresh(address, Duration::from_millis(10))
        .unwrap();

    // Never seen yet — stale as soon as the TTL is set.
    assert!(
        knx.group_registry
            .next_stale(std::time::Instant::now())
            .is_some()
    );
    assert!(
        knx.group_address_state(address)
            .unwrap()
            .last_seen
            .is_none()
    );

    // A GroupValueRead carries no value and must not clear staleness.
    let read_telegram = Telegram::received(
        knx.config.individual_address,
        address,
        TelegramType::GroupValueRead,
        Vec::new(),
    );
    Knx::record_group_value_if_present(&knx.group_registry, &read_telegram);
    assert!(
        knx.group_address_state(address)
            .unwrap()
            .last_seen
            .is_none()
    );

    // A real write does clear it and caches the payload.
    let write_telegram = Telegram::received(
        knx.config.individual_address,
        address,
        TelegramType::GroupValueWrite,
        vec![0x01],
    );
    Knx::record_group_value_if_present(&knx.group_registry, &write_telegram);

    let state = knx.group_address_state(address).unwrap();
    assert_eq!(state.last_seen_value, Some(vec![0x01]));
    assert!(state.last_seen.is_some());
    assert!(
        knx.group_registry
            .next_stale(std::time::Instant::now())
            .is_none()
    );

    // Once the TTL elapses again, it's back to stale.
    sleep(Duration::from_millis(30)).await;
    assert!(
        knx.group_registry
            .next_stale(std::time::Instant::now())
            .is_some()
    );
}

/// A responder callback (the `read_responder.rs` example's pattern):
/// answer a `GroupValueRead` for a known address with a
/// `GroupValueResponse`, but only once it actually has a value — never
/// a stale default — and never react to a `GroupValueWrite` or its own
/// outgoing response.
#[cfg(feature = "dpt")]
#[tokio::test]
async fn read_responder_answers_reads_only_once_it_has_a_value() {
    use crate::protocol::dpt::DPTSwitch;

    struct Responder {
        address: RawGroupAddress,
        knx: Knx,
        value: bool,
    }

    #[async_trait::async_trait]
    impl TelegramCallbackFn for Responder {
        async fn call(&self, telegram: &Telegram) {
            if telegram.telegram_type != TelegramType::GroupValueRead {
                return;
            }
            let response =
                Telegram::group_response_value::<DPTSwitch>(self.address, self.value).unwrap();
            self.knx.send_telegram(&response).await.unwrap();
        }
    }

    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    let address = RawGroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);
    knx.register_telegram_callback_filtered(
        Responder {
            address,
            knx: knx.clone(),
            value: true,
        },
        TelegramFilter::GroupAddresses(vec![address]),
        false,
    )
    .await;

    // A write must not trigger a response.
    let write = Telegram::received(
        knx.config.individual_address,
        address,
        TelegramType::GroupValueWrite,
        vec![0x01],
    );
    knx.test_notify_telegram_received(&write).await;
    assert_eq!(knx.telegram_queue_size().await, 0);

    // A read does — exactly one response, carrying the DPT-encoded value.
    let read = Telegram::received(
        knx.config.individual_address,
        address,
        TelegramType::GroupValueRead,
        Vec::new(),
    );
    knx.test_notify_telegram_received(&read).await;
    assert_eq!(knx.telegram_queue_size().await, 1);
}

/// A `GroupValueRead` that arrived too long before the responder
/// actually processes it (queue backlog, a slow callback ahead of it,
/// ...) must not be answered — `Telegram::timestamp` marks when it
/// arrived, not when the callback happens to run, and a response to a
/// requester that's long since given up (or that overwrites a fresher
/// answer from another device on the same address) is worse than no
/// response at all.
#[cfg(feature = "dpt")]
#[tokio::test]
async fn read_responder_ignores_reads_that_arrived_too_late() {
    use crate::protocol::dpt::DPTSwitch;

    const MAX_RESPONSE_DELAY: Duration = Duration::from_millis(500);

    struct Responder {
        address: RawGroupAddress,
        knx: Knx,
    }

    #[async_trait::async_trait]
    impl TelegramCallbackFn for Responder {
        async fn call(&self, telegram: &Telegram) {
            if telegram.telegram_type != TelegramType::GroupValueRead {
                return;
            }
            if telegram.is_older_than(MAX_RESPONSE_DELAY) {
                return;
            }
            let response = Telegram::group_response_value::<DPTSwitch>(self.address, true).unwrap();
            self.knx.send_telegram(&response).await.unwrap();
        }
    }

    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    let address = RawGroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);
    knx.register_telegram_callback_filtered(
        Responder {
            address,
            knx: knx.clone(),
        },
        TelegramFilter::GroupAddresses(vec![address]),
        false,
    )
    .await;

    // A stale read (arrived well past the delay budget) is ignored.
    let mut stale_read = Telegram::received(
        knx.config.individual_address,
        address,
        TelegramType::GroupValueRead,
        Vec::new(),
    );
    stale_read.timestamp -= MAX_RESPONSE_DELAY * 2;
    knx.test_notify_telegram_received(&stale_read).await;
    assert_eq!(knx.telegram_queue_size().await, 0);

    // A fresh read for the same address still gets answered.
    let fresh_read = Telegram::received(
        knx.config.individual_address,
        address,
        TelegramType::GroupValueRead,
        Vec::new(),
    );
    knx.test_notify_telegram_received(&fresh_read).await;
    assert_eq!(knx.telegram_queue_size().await, 1);
}

#[tokio::test]
async fn test_knx_connection_callback_integration() {
    // Create Knx instance with connection callback
    let callback_counter = Arc::new(AtomicUsize::new(0));
    let received_states = Arc::new(tokio::sync::RwLock::new(Vec::new()));

    let counter_clone = callback_counter.clone();
    let states_clone = received_states.clone();

    // Create Knx instance first, then register callback at runtime
    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    // Register connection callback using sync method
    let _handle = knx
        .event_handler
        .register_connection_callback_sync(move |state: ConnectionState| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            let states_clone = states_clone.clone();
            tokio::spawn(async move {
                states_clone.write().await.push(state);
            });
        })
        .await;

    // Verify callback was registered
    assert_eq!(knx.connection_callback_count().await, 1);

    // Trigger connection state notifications
    knx.test_notify_connection_state_changed(ConnectionState::Connecting)
        .await;
    knx.test_notify_connection_state_changed(ConnectionState::Connected)
        .await;
    knx.test_notify_connection_state_changed(ConnectionState::Disconnected)
        .await;

    // Give async tasks time to complete
    sleep(Duration::from_millis(50)).await;

    // Verify callbacks were invoked
    assert_eq!(callback_counter.load(Ordering::SeqCst), 3);

    let states = received_states.read().await;
    assert_eq!(states.len(), 3);
    assert_eq!(states[0], ConnectionState::Connecting);
    assert_eq!(states[1], ConnectionState::Connected);
    assert_eq!(states[2], ConnectionState::Disconnected);
}

#[tokio::test]
async fn test_knx_mixed_callbacks_integration() {
    // Create Knx instance with multiple callback types
    let telegram_counter = Arc::new(AtomicUsize::new(0));
    let connection_counter = Arc::new(AtomicUsize::new(0));

    let telegram_counter_clone = telegram_counter.clone();
    let connection_counter_clone = connection_counter.clone();

    // Create Knx instance first, then register callbacks at runtime
    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    // Register callbacks using sync methods
    let _telegram_handle = knx
        .event_handler
        .register_telegram_callback_sync(move |_telegram: &Telegram| {
            telegram_counter_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    let _connection_handle = knx
        .event_handler
        .register_connection_callback_sync(move |_state: ConnectionState| {
            connection_counter_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    // Verify all callbacks were registered
    assert_eq!(knx.total_callback_count().await, 2);
    assert_eq!(knx.telegram_callback_count().await, 1);
    assert_eq!(knx.connection_callback_count().await, 1);

    // Create test data
    let telegram = Telegram {
        source: IndividualAddress::new(1, 1, 1),
        destination: Address::Group(RawGroupAddress::new(
            MainGroup::new(0),
            MiddleGroup::new(1),
            1,
        )),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        compact_payload: true,
        tpci: 0,
        timestamp: std::time::SystemTime::now(),
    };

    // Trigger all notifications
    knx.test_notify_telegram_received(&telegram).await;
    knx.test_notify_connection_state_changed(ConnectionState::Connected)
        .await;

    // Verify all callbacks were invoked
    assert_eq!(telegram_counter.load(Ordering::SeqCst), 1);
    assert_eq!(connection_counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_knx_telegram_filtering_integration() {
    // Create Knx instance with filtered telegram callback
    let callback_counter = Arc::new(AtomicUsize::new(0));
    let target_address = RawGroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);

    let counter_clone = callback_counter.clone();

    // Create Knx instance first, then register callback at runtime
    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    // Register filtered telegram callback using sync method
    let _handle = knx
        .event_handler
        .register_telegram_callback_sync_filtered(
            move |_telegram: &Telegram| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            },
            TelegramFilter::GroupAddresses(vec![target_address]),
            false,
        )
        .await;

    // Create telegrams - one matching, one not matching
    let matching_telegram = Telegram {
        source: IndividualAddress::new(1, 1, 1),
        destination: Address::Group(target_address),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        compact_payload: true,
        tpci: 0,
        timestamp: std::time::SystemTime::now(),
    };

    let non_matching_telegram = Telegram {
        source: IndividualAddress::new(1, 1, 1),
        destination: Address::Group(RawGroupAddress::new(
            MainGroup::new(0),
            MiddleGroup::new(2),
            2,
        )),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        compact_payload: true,
        tpci: 0,
        timestamp: std::time::SystemTime::now(),
    };

    // Trigger notifications
    knx.test_notify_telegram_received(&matching_telegram).await;
    knx.test_notify_telegram_received(&non_matching_telegram)
        .await;

    // Only the matching telegram should trigger the callback
    assert_eq!(callback_counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_knx_callback_unregistration_integration() {
    // Create Knx instance
    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    // Register callbacks at runtime
    let callback_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = callback_counter.clone();

    let handle = knx
        .event_handler
        .register_telegram_callback_sync(move |_telegram: &Telegram| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    assert_eq!(knx.telegram_callback_count().await, 1);

    // Create test telegram
    let telegram = Telegram {
        source: IndividualAddress::new(1, 1, 1),
        destination: Address::Group(RawGroupAddress::new(
            MainGroup::new(0),
            MiddleGroup::new(1),
            1,
        )),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        compact_payload: true,
        tpci: 0,
        timestamp: std::time::SystemTime::now(),
    };

    // Trigger notification - should invoke callback
    knx.test_notify_telegram_received(&telegram).await;
    assert_eq!(callback_counter.load(Ordering::SeqCst), 1);

    // Unregister callback
    assert!(knx.event_handler.unregister_callback(handle).await);
    assert_eq!(knx.telegram_callback_count().await, 0);

    // Trigger notification again - should not invoke callback
    knx.test_notify_telegram_received(&telegram).await;
    assert_eq!(callback_counter.load(Ordering::SeqCst), 1); // Still 1, not incremented
}

#[tokio::test]
async fn test_knx_callback_error_handling_integration() {
    // Create Knx instance with callbacks that might have issues
    let successful_counter = Arc::new(AtomicUsize::new(0));
    let slow_counter = Arc::new(AtomicUsize::new(0));

    let successful_counter_clone = successful_counter.clone();
    let slow_counter_clone = slow_counter.clone();

    // Create Knx instance first, then register callbacks at runtime
    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await
        .unwrap();

    // Register successful callback using sync method
    let _successful_handle = knx
        .event_handler
        .register_telegram_callback_sync(move |_telegram: &Telegram| {
            successful_counter_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    // Register a slow async callback at runtime
    let _slow_handle = knx
        .event_handler
        .register_telegram_callback(SlowCallback {
            counter: slow_counter_clone,
        })
        .await;

    assert_eq!(knx.telegram_callback_count().await, 2);

    // Create test telegram
    let telegram = Telegram {
        source: IndividualAddress::new(1, 1, 1),
        destination: Address::Group(RawGroupAddress::new(
            MainGroup::new(0),
            MiddleGroup::new(1),
            1,
        )),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        compact_payload: true,
        tpci: 0,
        timestamp: std::time::SystemTime::now(),
    };

    // Trigger notification
    knx.test_notify_telegram_received(&telegram).await;

    // The fast callback should complete successfully
    assert_eq!(successful_counter.load(Ordering::SeqCst), 1);

    // The slow callback should be attempted (it increments before sleeping)
    assert_eq!(slow_counter.load(Ordering::SeqCst), 1);
}
