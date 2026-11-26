#![cfg(feature = "cli")]

use std::process::Command;

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
