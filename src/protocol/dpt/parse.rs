//! Human-readable values accepted by the runtime DPT API.

use super::view::{bool_label, control2_underlying, weekday_name};
use super::{DptPayload, DptType};
use crate::error::{ProtocolError, Result};

impl DptType {
    /// Parse a human-readable value into raw bytes for this DPT type.
    ///
    /// This is the reverse of [`super::DptView::formatted`]: it accepts the same
    /// formats that method produces (e.g. `"On"`, `"23.5 °C"`,
    /// `"2026-06-11"`), plus common synonyms for booleans (`true`/`false`,
    /// `1`/`0`).
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `input` doesn't parse into a
    /// value this DPT can encode, or if the parsed value is out of range.
    pub fn parse(&self, input: &str) -> Result<Vec<u8>> {
        let input = if matches!(self.main(), 4 | 16 | 234) {
            input
        } else {
            input.trim()
        };
        let payload = (*self).parse_payload(input)?;
        self.encode(&payload)
    }

    fn parse_payload(self, s: &str) -> Result<DptPayload> {
        let bad = |details: String| -> crate::error::KnxError {
            ProtocolError::DptError {
                dpt_type: self.number_str(),
                details,
            }
            .into()
        };
        match self.main() {
            1 => parse_bool(s, self)
                .map(DptPayload::Bool)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a boolean"))),
            2 => {
                let value = parse_bool(s, control2_underlying(self))
                    .ok_or_else(|| bad(format!("cannot parse {s:?} as a boolean")))?;
                Ok(DptPayload::BinaryControl {
                    control: true,
                    value,
                })
            }
            3 => parse_control3(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as a control step (try \"increase 3\", \"decrease 3\", or \"stop\")"
                ))
            }),
            5 | 6 | 7 | 8 | 12 | 13 => {
                let value = parse_number(s, self)
                    .ok_or_else(|| bad(format!("cannot parse {s:?} as a number")))?;
                let (min, max) = int_range(self.main());
                if value < min || value > max {
                    return Err(bad(format!("{value} out of range {min}..={max}")));
                }
                if matches!(self.main(), 6 | 8 | 13) {
                    Ok(DptPayload::SignedInt(value as i64))
                } else {
                    Ok(DptPayload::UnsignedInt(value as u64))
                }
            }
            9 => {
                const MIN: f64 = -671_088.64;
                const MAX: f64 = 670_433.28;
                let value = parse_number(s, self)
                    .ok_or_else(|| bad(format!("cannot parse {s:?} as a number")))?;
                if !(MIN..=MAX).contains(&value) {
                    return Err(bad(format!("{value} out of range {MIN}..={MAX}")));
                }
                Ok(DptPayload::Float(value))
            }
            14 => {
                let value = parse_number(s, self)
                    .ok_or_else(|| bad(format!("cannot parse {s:?} as a number")))?;
                if value.is_finite() && !(value as f32).is_finite() {
                    return Err(bad(format!("{value} out of range for a 4-byte float")));
                }
                Ok(DptPayload::Float(value))
            }
            29 => s
                .parse::<i64>()
                .map(DptPayload::SignedInt)
                .map_err(|_| bad(format!("cannot parse {s:?} as an integer"))),
            10 => parse_time(s)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a time (expected \"HH:MM:SS\")"))),
            11 => parse_date(
                s,
            )
            .ok_or_else(|| bad(format!("cannot parse {s:?} as a date (expected \"YYYY-MM-DD\")"))),
            19 => parse_datetime(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as a date/time (expected \"YYYY-MM-DD HH:MM:SS\")"
                ))
            }),
            4 => Ok(DptPayload::String(s.to_string())),
            16 => Ok(DptPayload::String(s.to_string())),
            17 => parse_scene_number(s)
                .map(DptPayload::Scene)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a scene number (1-64)"))),
            18 => parse_scene_control(s)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a scene control value"))),
            20 if self.sub() == 60_102 => s
                .parse::<u8>()
                .map(u64::from)
                .map(DptPayload::UnsignedInt)
                .map_err(|_| bad(format!("cannot parse {s:?} as an HVAC status byte"))),
            20 => parse_enum(s, self)
                .map(DptPayload::Enum)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as an enum value"))),
            217 => s.parse().map(DptPayload::Version),
            25 => s.parse().map(DptPayload::DoubleNibble),
            26 => s.parse().map(DptPayload::SceneInfo),
            27 => s.parse().map(DptPayload::CombinedInfoOnOff),
            30 => s.parse().map(DptPayload::ChannelActivation24),
            230 => s.parse().map(DptPayload::MBusAddress),
            236 => s.parse().map(DptPayload::PrioritisedModeControl),
            237 => s.parse().map(DptPayload::DALIControlGearDiagnostic),
            255 => s.parse().map(DptPayload::GeographicalLocation),
            240 => s.parse().map(DptPayload::CombinedPosition),
            241 => s.parse().map(DptPayload::StatusSAB),
            15 => s.parse().map(DptPayload::AccessData),
            206 => s.parse().map(DptPayload::ModeNext),
            225 => s.parse().map(DptPayload::ScalingTime),
            207 => super::dpt207::parse_payload(s,self.sub()),
            229 => s.parse().map(DptPayload::MeteringValue),
            219 => s.parse().map(DptPayload::AlarmInfo),
            246 => s.parse().map(DptPayload::BatteryInfo),
            244 => s.parse().map(DptPayload::ConverterStatus),
            248 => s.parse().map(DptPayload::ConverterInfoFix),
            272 => s.parse().map(DptPayload::ConverterInfo),
            245 => s.parse().map(DptPayload::ConverterTestResult),
            247 => s.parse().map(DptPayload::ConverterTestInfo),
            249 => s.parse().map(DptPayload::ColorTemperatureTransition),
            250 | 252..=254 => super::dpt_relative_color::parse_payload(s,self.main()),
            243 => s.parse().map(DptPayload::XYYColorTransition),
            21 if self.sub() == 1000 => s.parse().map(DptPayload::RFModeInfo),
            21 if self.sub() == 1 => s.parse().map(DptPayload::GeneralStatus),
            21 if self.sub() == 1200 => s.parse().map(DptPayload::VirtualDryContact),
            21 if self.sub() == 1201 => s.parse().map(DptPayload::PhasesStatus),
            21 if self.sub() == 1001 => s.parse().map(DptPayload::RFFilterInfo),
            21 if self.sub() == 1002 => s.parse().map(DptPayload::SecurityReport),
            21 if self.sub() == 1010 => s.parse().map(DptPayload::ChannelActivation8),
            21 if self.sub() == 601 => s.parse().map(DptPayload::LightActuatorErrorInfo),
            21 if self.sub() == 100 => s.parse().map(DptPayload::ForcingSignal),
            21 if self.sub() == 101 => s.parse().map(DptPayload::ForcingSignalCool),
            21 if self.sub() == 102 => s.parse().map(DptPayload::RoomHeatingControllerStatus),
            21 if self.sub() == 103 => s.parse().map(DptPayload::SolarDHWControllerStatus),
            21 if self.sub() == 104 => s.parse().map(DptPayload::FuelTypeSet),
            21 if self.sub() == 105 => s.parse().map(DptPayload::RoomCoolingControllerStatus),
            21 if self.sub() == 106 => s.parse().map(DptPayload::VentilationControllerStatus),
            21 if self.sub() == 2 => s.parse().map(DptPayload::DeviceControl),
            21 => s.parse().map(DptPayload::StatusRTSM),
            22 if self.sub() == 100 => s.parse().map(DptPayload::DHWControllerStatus),
            22 if self.sub() == 1000 => s.parse().map(DptPayload::SupportedMedia),
            22 if self.sub() == 1010 => s.parse().map(DptPayload::ChannelActivation16),
            22 if self.sub() == 101 => s.parse().map(DptPayload::RoomHeatingCoolingControllerStatus),
            22 if self.sub() == 102 => s.parse().map(DptPayload::StatusHVA),
            22 => s.parse().map(DptPayload::StatusRTC),
            273 => s.parse().map(DptPayload::Forecast),
            222 => s.parse().map(DptPayload::Setpoints3),
            275 => s.parse().map(DptPayload::Setpoints4),
            238 if self.sub() == 1 => s.parse().map(DptPayload::SceneConfig),
            238 => s.parse().map(DptPayload::DALIDiagnostics),
            234 => Ok(DptPayload::String(s.to_string())),
            232 => parse_rgb(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as RGB (try \"RGB(r, g, b)\" or \"#RRGGBB\")"
                ))
            }),
            242 => parse_xyy(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as XYY (try \"XYY(x, y, brightness)\")"
                ))
            }),
            251 => parse_rgbw(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as RGBW (try \"RGBW(r, g, b, w)\" or \"#RRGGBBWW\")"
                ))
            }),
            _ => Err(bad("unsupported DPT main number".to_string())),
        }
    }
}

/// Parse a boolean, accepting the subtype's own semantic label pair (e.g.
/// "Open"/"Closed" for `OpenClose`) case-insensitively, plus generic
/// true/false, 1/0, on/off, yes/no synonyms.
fn parse_bool(s: &str, dpt: DptType) -> Option<bool> {
    let lower = s.to_ascii_lowercase();
    if lower == bool_label(true, dpt).to_ascii_lowercase() {
        return Some(true);
    }
    if lower == bool_label(false, dpt).to_ascii_lowercase() {
        return Some(false);
    }
    match lower.as_str() {
        "true" | "1" | "on" | "yes" => Some(true),
        "false" | "0" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// Parse a DPT 3.xxx control value: "stop"/"break", "increase N"/"decrease
/// N" (also "up"/"down"), or signed shorthand like "+3"/"-3".
fn parse_control3(s: &str) -> Option<DptPayload> {
    let lower = s.trim().to_ascii_lowercase();
    if lower == "stop" || lower == "break" {
        return Some(DptPayload::Control {
            step: false,
            step_code: 0,
        });
    }
    if let Some((word, rest)) = lower.split_once(char::is_whitespace) {
        let step = match word {
            "increase" | "up" => true,
            "decrease" | "down" => false,
            _ => return None,
        };
        let step_code: u8 = rest.trim().parse().ok()?;
        return (step_code <= 7).then_some(DptPayload::Control { step, step_code });
    }
    let n: i8 = lower.parse().ok()?;
    let step_code = n.unsigned_abs();
    (step_code <= 7).then_some(DptPayload::Control {
        step: n >= 0,
        step_code,
    })
}

/// Valid (min, max) range for the raw integer encoding of a numeric DPT
/// main number. Unbounded mains (e.g. DPT 29's i64) return the full f64
/// range.
fn int_range(main: u16) -> (f64, f64) {
    match main {
        5 => (0.0, 255.0),
        6 => (-128.0, 127.0),
        7 => (0.0, 65535.0),
        8 => (-32768.0, 32767.0),
        12 => (0.0, 4_294_967_295.0),
        13 => (-2_147_483_648.0, 2_147_483_647.0),
        _ => (f64::MIN, f64::MAX),
    }
}

/// Parse a plain or unit-suffixed number, stripping the DPT's own unit
/// symbol if present (e.g. `"23.5 °C"` for a `Temperature` DPT).
fn parse_number(s: &str, dpt: DptType) -> Option<f64> {
    let s = match dpt.unit() {
        Some(unit) => s.strip_suffix(unit.symbol()).map_or(s, str::trim),
        None => s,
    };
    s.trim().parse::<f64>().ok()
}

/// Split a trailing `" (Weekday)"` suffix off, returning the day-of-week
/// number (0 if absent or unrecognized).
fn split_weekday_suffix(s: &str) -> (&str, u8) {
    if let Some(open) = s.rfind('(') {
        if let Some(close_offset) = s[open..].find(')') {
            let name = s[open + 1..open + close_offset].trim();
            if let Some(day) = weekday_number(name) {
                return (s[..open].trim(), day);
            }
        }
    }
    (s.trim(), 0)
}

fn weekday_number(name: &str) -> Option<u8> {
    (1..=7).find(|&d| weekday_name(d) == Some(name))
}

fn parse_time(s: &str) -> Option<DptPayload> {
    let (time_part, day) = split_weekday_suffix(s);
    let mut it = time_part.splitn(3, ':');
    let hour: u8 = it.next()?.trim().parse().ok()?;
    let minute: u8 = it.next()?.trim().parse().ok()?;
    let second: u8 = it.next()?.trim().parse().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(DptPayload::Time {
        day,
        hour,
        minute,
        second,
    })
}

fn parse_date(s: &str) -> Option<DptPayload> {
    let mut it = s.trim().splitn(3, '-');
    let year: u16 = it.next()?.parse().ok()?;
    let month: u8 = it.next()?.parse().ok()?;
    let day: u8 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(DptPayload::Date { day, month, year })
}

fn parse_datetime(s: &str) -> Option<DptPayload> {
    let (main_part, day_of_week) = split_weekday_suffix(s);
    let (date_part, time_part) = main_part.split_once(' ')?;
    let Some(DptPayload::Date { day, month, year }) = parse_date(date_part) else {
        return None;
    };
    let mut it = time_part.trim().splitn(3, ':');
    let hour: u8 = it.next()?.trim().parse().ok()?;
    let minute: u8 = it.next()?.trim().parse().ok()?;
    let second: u8 = it.next()?.trim().parse().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(DptPayload::DateTime {
        year,
        month,
        day,
        day_of_week,
        hour,
        minute,
        second,
    })
}

fn parse_scene_number(s: &str) -> Option<u8> {
    let s = s.trim();
    let digits = s
        .strip_prefix("Scene ")
        .or_else(|| s.strip_prefix("scene "))
        .unwrap_or(s);
    let n: u8 = digits.trim().parse().ok()?;
    (1..=64).contains(&n).then_some(n)
}

/// Parse a DPT 18 scene control value: `"Scene N (learn)"`,
/// `"Scene N (activate)"`, or a bare scene number (defaults to activate).
fn parse_scene_control(s: &str) -> Option<DptPayload> {
    let s = s.trim();
    let (num_part, learn) = match s.strip_suffix(')').and_then(|s| {
        let idx = s.rfind('(')?;
        Some((&s[..idx], s[idx + 1..].trim().to_ascii_lowercase()))
    }) {
        Some((num_part, mode)) => (num_part.trim(), mode == "learn"),
        None => (s, false),
    };
    let scene = parse_scene_number(num_part)?;
    Some(DptPayload::SceneControl { scene, learn })
}

fn parse_enum(s: &str, dpt: DptType) -> Option<u8> {
    if let Ok(n) = s.trim().parse::<u8>() {
        return Some(n);
    }
    let lower = s.trim().to_ascii_lowercase();
    match dpt {
        DptType::ErrorClassSystem => {
            let normalized = lower.replace([' ', '-'], "_");
            crate::protocol::dpt::ErrorClassSystem::ALL
                .into_iter()
                .find(|class| {
                    class.label().to_ascii_lowercase().replace([' ', '-'], "_") == normalized
                })
                .map(|class| class as u8)
        }
        DptType::ChangeoverMode => match lower.as_str() {
            "auto" => Some(0),
            "cooling only" | "cooling_only" => Some(1),
            "heating only" | "heating_only" => Some(2),
            _ => None,
        },
        DptType::EnableHCStage => match lower.as_str() {
            "disabled" => Some(0),
            "stage a" | "stage_a" => Some(1),
            "stage b" | "stage_b" => Some(2),
            "both stages" | "both_stages" => Some(3),
            _ => None,
        },
        DptType::HVACMode => match lower.as_str() {
            "auto" => Some(0),
            "comfort" => Some(1),
            "standby" => Some(2),
            "economy" => Some(3),
            "building protection" => Some(4),
            _ => None,
        },
        DptType::HVACContrMode => match lower.as_str() {
            "auto" => Some(0),
            "heat" => Some(1),
            "morning warmup" => Some(2),
            "cool" => Some(3),
            "night purge" => Some(4),
            "precool" => Some(5),
            "off" => Some(6),
            "test" => Some(7),
            "emergency heat" => Some(8),
            "fan only" => Some(9),
            "free cool" => Some(10),
            "ice" => Some(11),
            "maximum heating mode" => Some(12),
            "economic heat/cool mode" => Some(13),
            "dehumidification" => Some(14),
            "calibration mode" => Some(15),
            "emergency cool mode" => Some(16),
            "emergency steam mode" => Some(17),
            "no demand" => Some(20),
            _ => None,
        },
        _ => None,
    }
}

/// Strip a `"Name("`/`")"` wrapper if present, otherwise return the input
/// trimmed and unwrapped (so bare `"r, g, b"` also works).
fn strip_wrapper<'a>(s: &'a str, prefix: &str) -> &'a str {
    let s = s.trim();
    s.strip_prefix(prefix)
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(s)
}

fn parse_rgb(s: &str) -> Option<DptPayload> {
    let inner = strip_wrapper(s, "RGB(");
    if let Some(hex) = inner.strip_prefix('#') {
        let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
        let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
        let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
        return Some(DptPayload::ColorRGB { r, g, b });
    }
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let [r, g, b] = parts[..] else { return None };
    Some(DptPayload::ColorRGB {
        r: r.parse().ok()?,
        g: g.parse().ok()?,
        b: b.parse().ok()?,
    })
}

fn parse_rgbw(input: &str) -> Option<DptPayload> {
    let inner = strip_wrapper(input, "RGBW(");
    if let Some(hex) = inner.strip_prefix('#') {
        let red = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
        let green = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
        let blue = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
        let white = u8::from_str_radix(hex.get(6..8)?, 16).ok()?;
        return Some(DptPayload::ColorRGBW {
            r: red,
            g: green,
            b: blue,
            w: white,
        });
    }
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let [red, green, blue, white] = parts[..] else {
        return None;
    };
    Some(DptPayload::ColorRGBW {
        r: red.parse().ok()?,
        g: green.parse().ok()?,
        b: blue.parse().ok()?,
        w: white.parse().ok()?,
    })
}

fn parse_xyy(s: &str) -> Option<DptPayload> {
    let inner = strip_wrapper(s, "XYY(");
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let [x, y, brightness] = parts[..] else {
        return None;
    };
    Some(DptPayload::ColorXYY {
        x: x.parse().ok()?,
        y: y.parse().ok()?,
        brightness: brightness.parse().ok()?,
    })
}
