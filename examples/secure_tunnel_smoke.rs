//! Read-only smoke test for `Tunnel::connect_secure` (KNX IP Secure) against
//! a real gateway. Connects, probes the heartbeat, listens briefly for
//! telegrams (acknowledging them), then disconnects gracefully. It NEVER
//! sends a `GroupValueWrite`, so it cannot change any device state.
//!
//! Usage:
//!   cargo run --example `secure_tunnel_smoke` -- <gateway-ip>:3671 <device-auth-password> [user-password] [user-id]
//!
//! The device-auth-password is typically the FDSK printed on the device —
//! strip the grouping hyphens/spaces before passing it here. If no
//! user-password is given, the device-auth-password is reused for it too
//! (many interfaces default the free-tunneling user's password to the FDSK
//! until ETS sets up a separate one).

use std::net::SocketAddr;

use knust::protocol::knxip::{ConnectionstateResponse, ServiceType};
use knust::transport::SecurityConfig;
use knust::transport::tunnel::Tunnel;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .filter_module("transport", log::LevelFilter::Debug)
        .filter_module("security", log::LevelFilter::Debug)
        .format_timestamp_millis()
        .init();

    let mut args = std::env::args().skip(1);
    let addr: SocketAddr = args
        .next()
        .expect("usage: secure_tunnel_smoke <gateway-ip>:3671 <device-auth-password> [user-password] [user-id]")
        .parse()?;
    let device_auth_password = args
        .next()
        .expect("device-auth-password required (e.g. the FDSK, hyphens/spaces stripped)");
    let user_password = args.next().unwrap_or_else(|| device_auth_password.clone());

    println!("→ Connecting (UDP, secure) to {addr} ...");
    let mut tunnel = Tunnel::new_udp(addr);
    let security = SecurityConfig {
        device_auth_password,
        user_password: Some(user_password),
        keyring_path: None,
        session_timeout: 60,
    };
    tunnel.connect_secure(&security).await?;
    println!(
        "✓ Secure handshake + tunnel connect OK: channel_id={}",
        tunnel.channel_id()
    );

    println!("→ Probing heartbeat (ConnectionState_Request) ...");
    let rx = tunnel
        .router()
        .register(ServiceType::ConnectionstateResponse);
    tunnel.send_connectionstate_request().await?;
    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
        Ok(Ok(frame)) => {
            let ok = ConnectionstateResponse::parse(&frame.body)
                .is_ok_and(|r| r.status == ConnectionstateResponse::STATUS_OK);
            println!("✓ Heartbeat response (status_ok={ok})");
        }
        _ => println!("✗ Heartbeat timed out (no ConnectionState_Response)"),
    }

    println!("→ Sending graceful DisconnectRequest ...");
    tunnel.send_disconnect().await?;
    println!("✓ Done.");
    Ok(())
}
