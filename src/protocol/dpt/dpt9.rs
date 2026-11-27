//! DPT 9.xxx - 2-byte float values (true zero-copy)

use super::{DptType, DptValue, Result, WriteValue};
use crate::error::ProtocolError;

/// DPT 9.001 - Temperature (°C) - stores raw 2-byte representation
#[derive(Debug, Clone, PartialEq)]
pub struct Temperature([u8; 2]);

impl Temperature {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `value` is not finite or outside the DPT 9 wire range.
    pub fn new(value: f32) -> Result<Self> {
        let mut temp = Temperature([0, 0]);
        let value = if value.to_bits() == 670_433.28_f32.to_bits() {
            670_433.28
        } else if value.to_bits() == (-671_088.6_f32).to_bits() {
            -671_088.64
        } else {
            f64::from(value)
        };
        encode_2byte_float_to_bytes(value, &mut temp.0)?;
        Ok(temp)
    }

    #[must_use]
    pub fn value(&self) -> f32 {
        decode_2byte_float_from_bytes(self.0)
    }
}

impl DptValue for Temperature {
    const DPT_NUMBER: &'static str = "9.001";
    const VALUE_TYPE: &'static str = "temperature";
    const UNIT: Option<&'static str> = Some("°C");
    const HA_DEVICE_CLASS: Option<&'static str> = Some("temperature");
    const BYTE_LENGTH: usize = 2;

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::BYTE_LENGTH {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!(
                    "Invalid length: expected {}, got {}",
                    Self::BYTE_LENGTH,
                    bytes.len()
                ),
            }
            .into());
        }
        if bytes == [0x7F, 0xFF] {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: "Reserved invalid-data value 0x7FFF".to_string(),
            }
            .into());
        }
        Ok(Temperature([bytes[0], bytes[1]]))
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn validate(&self) -> Result<()> {
        let value = self.value();
        if !(-273.0..=670_433.28).contains(&value) {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Temperature {value} out of range [-273.0, 670433.28]"),
            }
            .into());
        }
        Ok(())
    }

    fn value_range() -> (f64, f64) {
        (-273.0, 670_433.28)
    }
}

/// Encode a float as 2-byte KNX format directly to byte array
pub(super) fn encode_2byte_float_to_bytes(value: f64, bytes: &mut [u8; 2]) -> Result<()> {
    if !(-671_088.64..=670_433.28).contains(&value) {
        return Err(ProtocolError::DptError {
            dpt_type: "9.xxx".to_string(),
            details: "Value outside finite DPT 9 range [-671088.64, 670433.28]".to_string(),
        }
        .into());
    }
    let mut mantissa = value * 100.0;
    let mut exponent = 0u16;
    while !(-2048.0..=2047.0).contains(&mantissa) {
        mantissa /= 2.0;
        exponent += 1;
    }
    let mantissa = mantissa.round_ties_even() as i16;
    let encoded = (u16::from(mantissa < 0) << 15) | (exponent << 11) | ((mantissa as u16) & 0x07FF);
    *bytes = encoded.to_be_bytes();
    Ok(())
}

/// Decode 2-byte KNX float format directly from byte array
fn decode_2byte_float_from_bytes(bytes: [u8; 2]) -> f32 {
    let raw = u16::from_be_bytes(bytes);

    let sign = (raw >> 15) & 1;
    let exponent = (raw >> 11) & 0x0F;
    let mut mantissa = i32::from(raw & 0x07FF);
    if sign == 1 {
        mantissa -= 2048;
    }

    ((mantissa << exponent) as f32) / 100.0
}

// Helper trait for aliases

// Python-style aliases

pub(super) fn subtype_min(sub: u16) -> f64 {
    match sub {
        1 => -273.0,
        4..=8 | 28..=30 => 0.0,
        27 => -459.6,
        _ => -671_088.64,
    }
}

pub(super) fn decode_wire_f64(bytes: &[u8]) -> f64 {
    let raw = u16::from_be_bytes([bytes[0], bytes[1]]);
    let mantissa = i32::from(raw & 0x07ff) - if raw & 0x8000 != 0 { 2048 } else { 0 };
    f64::from(mantissa << ((raw >> 11) & 15)) * 0.01
}

pub(super) fn validate_subtype(sub: u16, value: f64) -> Result<()> {
    let min = subtype_min(sub);
    if !(min..=670_433.28).contains(&value) {
        return Err(ProtocolError::DptError {
            dpt_type: format!("9.{sub:03}"),
            details: format!("Value {value} outside {min}..=670433.28"),
        }
        .into());
    }
    Ok(())
}

const fn subtype_number(text: &str) -> u16 {
    let bytes = text.as_bytes();
    let mut value = 0;
    let mut i = 0;
    while i < bytes.len() {
        value = value * 10 + (bytes[i] - b'0') as u16;
        i += 1;
    }
    value
}

macro_rules! dpt9_alias {
    ($name:ident, $main:literal, $sub:literal, $base:ty, $value_type:literal, $unit:expr, $ha_class:expr) => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name($base);
        impl $name {
            /// Construct a subtype value without silently replacing invalid input.
            ///
            /// # Errors
            ///
            /// Returns an error for non-finite or out-of-range values.
            pub fn new(value: f32) -> Result<Self> {
                let min = subtype_min(subtype_number(stringify!($sub)));
                if !value.is_finite() || value < min as f32 {
                    return Err(ProtocolError::DptError {
                        dpt_type: Self::DPT_NUMBER.into(),
                        details: "Value outside subtype range".into(),
                    }
                    .into());
                }
                let raw = <$base>::new(value)?;
                validate_subtype(
                    subtype_number(stringify!($sub)),
                    decode_wire_f64(raw.as_bytes()),
                )?;
                Ok(Self(raw))
            }
            #[must_use]
            pub fn value(&self) -> f32 {
                self.0.value()
            }
        }
        impl TryFrom<f32> for $name {
            type Error = crate::error::KnxError;
            fn try_from(value: f32) -> Result<Self> {
                Self::new(value)
            }
        }
        impl WriteValue<$name> for f32 {
            fn resolve(self, _: DptType) -> Result<$name> {
                $name::new(self)
            }
        }
        impl DptValue for $name {
            const DPT_NUMBER: &'static str = concat!(stringify!($main), ".", stringify!($sub));
            const VALUE_TYPE: &'static str = $value_type;
            const UNIT: Option<&'static str> = $unit;
            const HA_DEVICE_CLASS: Option<&'static str> = $ha_class;
            const BYTE_LENGTH: usize = 2;
            fn from_bytes(bytes: &[u8]) -> Result<Self> {
                let value = <$base>::from_bytes(bytes)?;
                validate_subtype(
                    subtype_number(stringify!($sub)),
                    decode_wire_f64(value.as_bytes()),
                )?;
                Ok(Self(value))
            }
            fn as_bytes(&self) -> &[u8] {
                self.0.as_bytes()
            }
            fn validate(&self) -> Result<()> {
                validate_subtype(
                    subtype_number(stringify!($sub)),
                    decode_wire_f64(self.0.as_bytes()),
                )
            }
            fn value_range() -> (f64, f64) {
                (subtype_min(subtype_number(stringify!($sub))), 670_433.28)
            }
        }
    };
}

dpt9_alias!(
    DPTTemperature,
    9,
    001,
    Temperature,
    "temperature",
    Some("°C"),
    Some("temperature")
);
dpt9_alias!(
    DPTTemperatureDifference2Byte,
    9,
    002,
    Temperature,
    "temperature_difference_2byte",
    Some("K"),
    Some("temperature")
);
dpt9_alias!(
    DPTTemperatureA,
    9,
    003,
    Temperature,
    "temperature_a",
    Some("K/h"),
    None
);
dpt9_alias!(
    DPTLux,
    9,
    004,
    Temperature,
    "illuminance",
    Some("lx"),
    Some("illuminance")
);
dpt9_alias!(
    DPTWsp,
    9,
    005,
    Temperature,
    "wind_speed_ms",
    Some("m/s"),
    Some("wind_speed")
);
dpt9_alias!(
    DPTPressure2Byte,
    9,
    006,
    Temperature,
    "pressure_2byte",
    Some("Pa"),
    Some("pressure")
);
dpt9_alias!(
    DPTHumidity,
    9,
    007,
    Temperature,
    "humidity",
    Some("%"),
    Some("humidity")
);
dpt9_alias!(
    DPTPartsPerMillion,
    9,
    008,
    Temperature,
    "ppm",
    Some("ppm"),
    None
);
dpt9_alias!(
    DPTAirFlow,
    9,
    009,
    Temperature,
    "air_flow",
    Some("m³/h"),
    None
);
dpt9_alias!(DPTTime1, 9, 010, Temperature, "time_1", Some("s"), None);
dpt9_alias!(DPTTime2, 9, 011, Temperature, "time_2", Some("ms"), None);
dpt9_alias!(
    DPTVoltage,
    9,
    020,
    Temperature,
    "voltage",
    Some("mV"),
    Some("voltage")
);
dpt9_alias!(
    DPTCurrent,
    9,
    021,
    Temperature,
    "curr",
    Some("mA"),
    Some("current")
);
dpt9_alias!(
    DPTPowerDensity,
    9,
    022,
    Temperature,
    "power_density",
    Some("W/m²"),
    None
);
dpt9_alias!(
    DPTKelvinPerPercent,
    9,
    023,
    Temperature,
    "kelvin_per_percent",
    Some("K/%"),
    None
);
dpt9_alias!(
    DPTPower2Byte,
    9,
    024,
    Temperature,
    "power_2byte",
    Some("kW"),
    Some("power")
);
dpt9_alias!(
    DPTVolumeFlow,
    9,
    025,
    Temperature,
    "volume_flow",
    Some("L/h"),
    None
);
dpt9_alias!(
    DPTRainAmount,
    9,
    026,
    Temperature,
    "rain_amount",
    Some("L/m²"),
    None
);
dpt9_alias!(
    DPTTemperatureF,
    9,
    027,
    Temperature,
    "temperature_f",
    Some("°F"),
    Some("temperature")
);
dpt9_alias!(
    DPTWspKmh,
    9,
    028,
    Temperature,
    "wind_speed_kmh",
    Some("km/h"),
    Some("wind_speed")
);
dpt9_alias!(
    DPTAbsoluteHumidity,
    9,
    029,
    Temperature,
    "absolute_humidity",
    Some("g/m³"),
    None
);
dpt9_alias!(
    DPTConcentrationUGM3,
    9,
    030,
    Temperature,
    "concentration_ugm3",
    Some("μg/m³"),
    None
);
dpt9_alias!(
    DPTEnthalpy,
    9,
    60000,
    Temperature,
    "enthalpy",
    Some("H"),
    None
);
