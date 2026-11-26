use knust::KnxBuilder;
use knust::application::callbacks::{TelegramCallbackFn, TelegramFilter};
use knust::protocol::{Address, RawGroupAddress, Telegram, TelegramType};
use std::ops::RangeInclusive;
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

pub(super) async fn run(
    builder: KnxBuilder,
    filters: Vec<GroupPattern>,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let (sender, mut receiver) = mpsc::channel(64);
    let matches_group = TelegramFilter::Custom(Arc::new(
        move |telegram| matches!(telegram.destination, Address::Group(address) if filters.is_empty() || filters.iter().any(|pattern| pattern.matches(address))),
    ));
    let client = builder
        .telegram_callback_filtered(MonitorOutput(sender), matches_group, true)?
        .connect()
        .await?;
    let output = async {
        let mut stdout = tokio::io::stdout();
        while let Some(telegram) = receiver.recv().await {
            let payload = match telegram.telegram_type {
                TelegramType::GroupValueWrite | TelegramType::GroupValueResponse => {
                    super::raw_value(&telegram)
                }
                TelegramType::GroupValueRead => "read".to_owned(),
                _ => continue,
            };
            let line = format!(
                "{:?} {} -> {} | {payload}\n",
                telegram.direction, telegram.source, telegram.destination
            );
            stdout.write_all(line.as_bytes()).await?;
            stdout.flush().await?;
        }
        Ok::<_, std::io::Error>(ExitCode::SUCCESS)
    };
    let result = tokio::select! {
        signal = tokio::signal::ctrl_c() => signal.map(|()| ExitCode::from(130)),
        result = output => result,
    };
    drop(receiver);
    let shutdown = client.shutdown().await;
    let exit = result?;
    shutdown?;
    Ok(exit)
}

struct MonitorOutput(mpsc::Sender<Telegram>);

#[async_trait::async_trait]
impl TelegramCallbackFn for MonitorOutput {
    async fn call(&self, telegram: &Telegram) {
        let _ = self.0.send(telegram.clone()).await;
    }
}

type Level = Vec<RangeInclusive<u16>>;

#[derive(Clone)]
pub(super) enum GroupPattern {
    Free(Level),
    TwoLevel(Level, Level),
    ThreeLevel(Level, Level, Level),
}

impl GroupPattern {
    fn matches(&self, address: RawGroupAddress) -> bool {
        let matches = |level: &Level, value| level.iter().any(|range| range.contains(&value));
        let main = u16::from(address.main());
        match self {
            Self::Free(raw) => matches(raw, address.raw()),
            Self::TwoLevel(a, b) => matches(a, main) && matches(b, address.raw() & 0x07ff),
            Self::ThreeLevel(a, b, c) => {
                matches(a, main)
                    && matches(b, u16::from(address.middle()))
                    && matches(c, u16::from(address.sub()))
            }
        }
    }
}

impl FromStr for GroupPattern {
    type Err = &'static str;

    fn from_str(pattern: &str) -> Result<Self, Self::Err> {
        match pattern.split('/').collect::<Vec<_>>().as_slice() {
            [raw] => Ok(Self::Free(parse_level(raw, u16::MAX)?)),
            [main, sub] => Ok(Self::TwoLevel(
                parse_level(main, 31)?,
                parse_level(sub, 2047)?,
            )),
            [main, middle, sub] => Ok(Self::ThreeLevel(
                parse_level(main, 31)?,
                parse_level(middle, 7)?,
                parse_level(sub, 255)?,
            )),
            _ => Err("expected a one-, two- or three-level group address pattern"),
        }
    }
}

fn parse_level(level: &str, max: u16) -> Result<Level, &'static str> {
    let number = |value: &str| {
        value
            .parse::<u16>()
            .ok()
            .filter(|&number| number <= max)
            .ok_or("invalid group address pattern number")
    };
    level
        .split(',')
        .map(|item| {
            if item == "*" {
                return Ok(0..=max);
            }
            if let Some((first, last)) = item.split_once('-') {
                let first = if first.is_empty() { 0 } else { number(first)? };
                let last = if last.is_empty() { max } else { number(last)? };
                return Ok(first.min(last)..=first.max(last));
            }
            let value = number(item)?;
            Ok(value..=value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_wildcards_ranges_and_alternative_address_layouts() {
        let group: RawGroupAddress = "1/4/8".parse().unwrap();
        for pattern in ["1/4/5-6,8", "1/4/*", "1/1032", "1/1032-", "3079-3081"] {
            assert!(
                pattern.parse::<GroupPattern>().unwrap().matches(group),
                "{pattern}"
            );
        }
        assert!(!"1/4/5-6".parse::<GroupPattern>().unwrap().matches(group));
        for pattern in ["1/4/8/0", "1/8/*", "1/4/foo", "1//8", "1/4/8,"] {
            assert!(pattern.parse::<GroupPattern>().is_err(), "{pattern}");
        }
    }
}
