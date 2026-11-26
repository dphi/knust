use clap::{Parser, Subcommand};
use std::net::Ipv4Addr;
use std::process::ExitCode;
use std::time::Duration;

mod group;
mod scan;

#[derive(Parser)]
#[command(version, about = "Discover and work with KNX/IP gateways")]
struct Options {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Discover KNX/IP gateways.
    Scan {
        /// Scan only this local IPv4 interface.
        #[arg(long, env = "KNX_LOCAL_IP")]
        local_ip: Option<Ipv4Addr>,
        /// Discovery timeout in seconds.
        #[arg(long, default_value = "3", value_parser = positive_seconds)]
        timeout: Duration,
    },
    /// Read or write group addresses on the KNX bus.
    Group(group::Options),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let result = match Options::parse().command {
        Command::Scan { local_ip, timeout } => scan::run(local_ip, timeout).await,
        Command::Group(options) => group::run(options).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn positive_seconds(value: &str) -> Result<Duration, &'static str> {
    value
        .parse::<f64>()
        .ok()
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .filter(|duration| !duration.is_zero())
        .ok_or("expected a finite, positive number of seconds")
}
