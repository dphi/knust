use knust::transport::{GatewayInfo, GatewayScanner};
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
use tokio::task::JoinSet;

pub(super) async fn run(
    local_ip: Option<Ipv4Addr>,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let interfaces = match local_ip {
        Some(ip) => BTreeSet::from([ip]),
        None => if_addrs::get_if_addrs()?
            .into_iter()
            .filter_map(|interface| match interface.ip() {
                IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
                _ => None,
            })
            .collect(),
    };
    if interfaces.is_empty() {
        return Err("no usable IPv4 interface found".into());
    }

    let mut scans = JoinSet::new();
    for interface in interfaces {
        scans.spawn(async move {
            let result = async {
                let scanner = GatewayScanner::new_with_interface(interface).await?;
                scanner.discover(timeout).await
            }
            .await;
            (interface, result)
        });
    }

    let mut gateways = BTreeMap::new();
    let mut failures = Vec::new();
    while let Some(scan) = scans.join_next().await {
        let (interface, result) = scan?;
        match result {
            Ok(found) => {
                for gateway in found {
                    gateways.insert(gateway.addr, gateway);
                }
            }
            Err(error) => failures.push(format!("scan failed on {interface}: {error}")),
        }
    }
    if gateways.is_empty() {
        for failure in failures {
            eprintln!("{failure}");
        }
        return Err("no gateways found".into());
    }
    for gateway in gateways.values() {
        print_gateway(gateway);
    }
    Ok(())
}

fn print_gateway(gateway: &GatewayInfo) {
    let address = gateway
        .individual_address
        .map_or_else(|| "unknown".to_owned(), |address| address.to_string());
    let capabilities = &gateway.capabilities;
    let tunneling = match (
        capabilities.tunneling_requires_secure,
        capabilities.supports_tcp_tunneling,
        capabilities.supports_tunneling,
    ) {
        (Some(true), _, _) => "Secure",
        (_, true, _) => "TCP",
        (_, _, true) => "UDP",
        _ => "No",
    };
    let routing = match (
        capabilities.routing_requires_secure,
        capabilities.supports_routing,
    ) {
        (Some(true), _) => "Secure",
        (_, true) => "Yes",
        _ => "No",
    };
    println!(
        "{address} {} {:?} tunneling: {tunneling} routing: {routing}",
        gateway.addr, gateway.name
    );
}
