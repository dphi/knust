//! Read-only smoke test for the unified `Tunnel`.
//!
//! Connects to a KNX/IP gateway over UDP, validates the handshake, probes the
//! `ConnectionState` heartbeat (via the frame router, exactly as the bus does),
//! listens briefly for incoming telegrams (acknowledging them as a good client),
//! then disconnects gracefully. It NEVER sends a `GroupValueWrite`, so it cannot
//! change any device state.
//!
//! Usage: cargo run --example `tunnel_smoke` -- <gateway-ip>:3671

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use knust::protocol::knxip::{ConnectionstateResponse, KnxIpFrame, ServiceType, TunnellingRequest};
use knust::transport::tunnel::Tunnel;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .filter_module("transport", log::LevelFilter::Debug)
        .format_timestamp_millis()
        .init();

    let addr: SocketAddr = std::env::args()
        .nth(1)
        .expect("usage: tunnel_smoke <gateway-ip>:3671 [udp|tcp]")
        .parse()?;
    let transport = std::env::args().nth(2).unwrap_or_else(|| "udp".to_string());

    println!("→ Connecting ({}) to {addr} ...", transport.to_uppercase());
    let mut tunnel = match transport.as_str() {
        "tcp" => Tunnel::new_tcp(addr),
        _ => Tunnel::new_udp(addr),
    };
    tunnel.connect().await?;
    println!(
        "✓ Handshake OK: channel_id={}, local={}",
        tunnel.channel_id(),
        tunnel.local_addr()
    );

    let tunnel = Arc::new(tunnel);

    // Receive loop: dispatch responses to router waiters, ACK + print telegrams.
    let recv_tunnel = tunnel.clone();
    let recv_task = tokio::spawn(async move {
        loop {
            let data = match recv_tunnel.recv_frame().await {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("recv ended: {e}");
                    break;
                }
            };
            let Ok(frame) = KnxIpFrame::parse(&data) else {
                continue;
            };
            // consumed by a waiter (heartbeat) if None
            let Some(frame) = recv_tunnel.router().dispatch(frame) else {
                continue;
            };
            if frame.header.service_type == ServiceType::TunnellingRequest
                && let Ok(treq) = TunnellingRequest::parse(&frame.body)
            {
                let _ = recv_tunnel
                    .send_tunnelling_ack(treq.communication_channel_id, treq.sequence_counter, 0x00)
                    .await;
                println!(
                    "← telegram (seq={}, {} cEMI bytes)",
                    treq.sequence_counter,
                    treq.raw_cemi.len()
                );
            }
        }
    });

    // Heartbeat probe, correlated through the router (same path as the bus).
    println!("→ Probing heartbeat (ConnectionState_Request) ...");
    let rx = tunnel
        .router()
        .register(ServiceType::ConnectionstateResponse);
    let started = Instant::now();
    tunnel.send_connectionstate_request().await?;
    match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(frame)) => {
            let ok = ConnectionstateResponse::parse(&frame.body)
                .is_ok_and(|r| r.status == ConnectionstateResponse::STATUS_OK);
            println!(
                "✓ Heartbeat response in {} ms (status_ok={ok})",
                started.elapsed().as_millis()
            );
        }
        _ => println!("✗ Heartbeat timed out (no ConnectionState_Response)"),
    }

    println!("… Listening 8s for telegrams (read-only) ...");
    tokio::time::sleep(Duration::from_secs(8)).await;

    recv_task.abort();
    let _ = recv_task.await;

    println!("→ Sending graceful DisconnectRequest ...");
    tunnel.send_disconnect().await?;
    println!("✓ Done.");
    Ok(())
}
