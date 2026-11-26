use clap::{Args, Subcommand};
use knust::Knx;
use knust::protocol::dpt::{DptRegistry, DptType};
use knust::protocol::{RawGroupAddress, Telegram};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

#[derive(Args)]
pub(super) struct Options {
    /// Gateway host or address, with an optional port (default 3671).
    #[arg(long, global = true, env = "KNX_GATEWAY")]
    gateway: Option<String>,
    /// Local address to bind the connection to.
    #[arg(long, global = true, env = "KNX_LOCAL_IP")]
    local_ip: Option<IpAddr>,
    /// Use TCP tunneling.
    #[arg(long, global = true)]
    tcp: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read a group address and print the response.
    Read {
        address: RawGroupAddress,
        /// DPT number or value type, such as 9.001 or temperature.
        #[arg(long, value_parser = parse_dpt)]
        r#type: Option<DptType>,
        /// Response timeout in seconds, starting at transmission.
        #[arg(long, default_value = "2", value_parser = super::positive_seconds)]
        timeout: Duration,
    },
    /// Write a group address and wait for confirmation.
    Write {
        address: RawGroupAddress,
        /// DPT value, raw 6-bit integer, on/off, or octet payload such as 0x0123.
        #[arg(allow_negative_numbers = true)]
        value: String,
        /// DPT number or value type, such as 9.001 or temperature.
        #[arg(long, value_parser = parse_dpt)]
        r#type: Option<DptType>,
    },
}

pub(super) async fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    match &options.command {
        Command::Read {
            address,
            r#type,
            timeout,
        } => {
            let client = options.connect().await?;
            let response = client.read_group_response(*address, *timeout).await;
            let shutdown = client.shutdown().await;
            let response = response?;
            let value = match r#type {
                Some(dpt) => dpt.decode_ref(&response.payload)?.formatted(*dpt),
                None => raw_value(&response),
            };
            shutdown?;
            println!("{value}");
        }
        Command::Write {
            address,
            value,
            r#type,
        } => {
            let (payload, compact) = match r#type {
                Some(dpt) => (dpt.parse(value)?, matches!(dpt.main(), 1..=3)),
                None => parse_raw_payload(value)?,
            };
            let mut telegram = Telegram::group_write(*address, payload);
            telegram.compact_payload = compact;
            let client = options.connect().await?;
            let written = client.send_telegram_confirmed(&telegram).await;
            let shutdown = client.shutdown().await;
            written?;
            shutdown?;
        }
    }
    Ok(())
}

impl Options {
    async fn connect(&self) -> Result<Knx, Box<dyn std::error::Error>> {
        let gateway = match &self.gateway {
            Some(value) => resolve_gateway(value, self.tcp, self.local_ip).await?,
            None => {
                let interface = match self.local_ip {
                    Some(IpAddr::V4(ip)) => Some(ip),
                    Some(IpAddr::V6(_)) => {
                        return Err("IPv4 discovery needs an IPv4 local address".into());
                    }
                    None => None,
                };
                let mut gateways = super::scan::discover(interface, Duration::from_secs(3))
                    .await?
                    .into_iter()
                    .filter(|gateway| {
                        let caps = &gateway.capabilities;
                        caps.tunneling_requires_secure != Some(true)
                            && if self.tcp {
                                caps.supports_tcp_tunneling
                            } else {
                                caps.supports_tunneling
                            }
                    });
                let gateway = gateways.next().ok_or("no compatible gateway found")?;
                if gateways.next().is_some() {
                    return Err(
                        "multiple compatible gateways found, select one with --gateway".into(),
                    );
                }
                gateway.addr
            }
        };
        let builder = if self.tcp {
            Knx::builder().tcp_tunnel(gateway)
        } else {
            Knx::builder().tunnel(gateway)
        };
        let builder = match self.local_ip {
            Some(ip) => builder.local_ip(ip),
            None => builder,
        };
        Ok(builder.auto_reconnect(false).connect().await?)
    }
}

async fn resolve_gateway(
    value: &str,
    tcp: bool,
    local: Option<IpAddr>,
) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Ok(address);
    }
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, 3671));
    }
    let (host, port) = match value.split_once(':') {
        Some((host, port)) => (host, port.parse::<u16>()?),
        None => (value, 3671),
    };
    Ok(tokio::net::lookup_host((host, port))
        .await?
        .find(|address| {
            (tcp || address.is_ipv4())
                && local.is_none_or(|local| local.is_ipv4() == address.is_ipv4())
        })
        .ok_or("gateway host has no address compatible with the selected transport")?)
}

fn parse_dpt(value: &str) -> knust::Result<DptType> {
    value.parse().or_else(|_| {
        DptRegistry::default()
            .dpt_number_for_value_type(value)?
            .parse()
    })
}

fn parse_raw_payload(value: &str) -> Result<(Vec<u8>, bool), Box<dyn std::error::Error>> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        let payload =
            hex::decode(hex).map_err(|error| format!("invalid hexadecimal payload: {error}"))?;
        if !(1..=254).contains(&payload.len()) {
            return Err("expected 1 to 254 octets".into());
        }
        return Ok((payload, false));
    }
    let raw = match value.to_ascii_lowercase().as_str() {
        "on" | "true" => 1,
        "off" | "false" => 0,
        value => value.parse::<u8>().ok().filter(|&value| value <= 63)
            .ok_or("expected on/off, a 6-bit integer, or hexadecimal octets; use --type for a DPT value")?,
    };
    Ok((vec![raw], true))
}

fn raw_value(telegram: &Telegram) -> String {
    if telegram.compact_payload
        && let [value] = telegram.payload.as_slice()
    {
        return value.to_string();
    }
    format!("0x{}", hex::encode(&telegram.payload))
}
