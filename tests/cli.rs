#![cfg(feature = "cli")]

use std::process::Command;

#[test]
fn invalid_group_values_fail_before_connecting() {
    let output = Command::new(env!("CARGO_BIN_EXE_knust"))
        .args(["group", "write", "1/2/3", "64", "--gateway", "0.0.0.0:0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("6-bit integer"));
}

#[test]
fn scan_rejects_invalid_timeouts() {
    for value in ["0", "-1", "inf", "NaN"] {
        let output = Command::new(env!("CARGO_BIN_EXE_knust"))
            .args(["scan", &format!("--timeout={value}")])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("positive number of seconds"));
    }
}

#[cfg(feature = "server")]
#[tokio::test]
async fn scan_discovers_a_knust_server() {
    use knust::transport::TunnelServer;
    use std::net::Ipv4Addr;

    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), "1.1.241".parse().unwrap())
        .await
        .unwrap();
    server
        .enable_multicast_discovery(Ipv4Addr::LOCALHOST)
        .unwrap();
    let output = tokio::task::spawn_blocking(|| {
        Command::new(env!("CARGO_BIN_EXE_knust"))
            .args(["scan", "--local-ip", "127.0.0.1", "--timeout", "0.5"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(&format!("1.1.241 {}", server.local_addr())),
        "{stdout}"
    );
    server.shutdown();
}

#[cfg(feature = "server")]
#[tokio::test]
async fn group_commands_exchange_telegram_values_with_a_knust_server() {
    use knust::protocol::{Address, Telegram, TelegramType};
    use knust::transport::TunnelServer;
    use std::time::Duration;
    use tokio::time::timeout;

    let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), "1.1.241".parse().unwrap())
        .await
        .unwrap();
    let group = "1/2/3".parse().unwrap();
    let mut traffic = server.subscribe();

    let invoke = |arguments: &[&str]| {
        let arguments: Vec<_> = arguments.iter().map(ToString::to_string).collect();
        let gateway = server.local_addr().to_string();
        tokio::task::spawn_blocking(move || {
            Command::new(env!("CARGO_BIN_EXE_knust"))
                .args(["group", "--gateway", &gateway])
                .args(arguments)
                .output()
                .unwrap()
        })
    };
    let write = invoke(&["write", "1/2/3", "0x01"]);
    let received = timeout(Duration::from_secs(3), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.destination, Address::Group(group));
    assert_eq!(received.telegram_type, TelegramType::GroupValueWrite);
    assert_eq!(received.payload, [1]);
    assert!(!received.compact_payload);
    let write = timeout(Duration::from_secs(5), write)
        .await
        .unwrap()
        .unwrap();
    assert!(
        write.status.success(),
        "{}",
        String::from_utf8_lossy(&write.stderr)
    );

    let read = invoke(&["read", "1/2/3", "--tcp"]);
    let received = timeout(Duration::from_secs(3), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.destination, Address::Group(group));
    assert_eq!(received.telegram_type, TelegramType::GroupValueRead);
    let mut response = Telegram::group_write(group, vec![1]);
    response.telegram_type = TelegramType::GroupValueResponse;
    response.compact_payload = false;
    server.send(response).await.unwrap();
    let read = timeout(Duration::from_secs(5), read)
        .await
        .unwrap()
        .unwrap();
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert_eq!(String::from_utf8(read.stdout).unwrap().trim(), "0x01");

    let write = invoke(&["write", "1/2/3", "--type", "temperature", "-2.5", "--tcp"]);
    let received = timeout(Duration::from_secs(3), traffic.recv())
        .await
        .unwrap()
        .unwrap();
    let dpt: knust::protocol::dpt::DptType = "9.001".parse().unwrap();
    assert_eq!(received.payload, dpt.parse("-2.5").unwrap());
    let write = timeout(Duration::from_secs(5), write)
        .await
        .unwrap()
        .unwrap();
    assert!(
        write.status.success(),
        "{}",
        String::from_utf8_lossy(&write.stderr)
    );
    server.shutdown();
}
