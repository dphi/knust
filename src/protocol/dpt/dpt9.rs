//! DPT 9.xxx - 2-byte float values (true zero-copy)

use super::{DptValue, Result};
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
        let value = if value == 670_433.28_f32 {
            670_433.28
        } else if value == -671_088.6_f32 {
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
use super::DptInnerType;

impl DptInnerType for Temperature {
    type InnerType = f32;
    fn new(value: f32) -> Self {
        Temperature::new(value).unwrap_or(Temperature([0, 0]))
    }
    fn into_inner(self) -> f32 {
        self.value()
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(
    DPTTemperature,
    9,
    001,
    Temperature,
    "temperature",
    Some("°C"),
    Some("temperature")
);
dpt_alias!(
    DPTTemperatureDifference2Byte,
    9,
    002,
    Temperature,
    "temperature_difference_2byte",
    Some("K"),
    Some("temperature")
);
dpt_alias!(
    DPTTemperatureA,
    9,
    003,
    Temperature,
    "temperature_a",
    Some("K/h"),
    None
);
dpt_alias!(
    DPTLux,
    9,
    004,
    Temperature,
    "illuminance",
    Some("lx"),
    Some("illuminance")
);
dpt_alias!(
    DPTWsp,
    9,
    005,
    Temperature,
    "wind_speed_ms",
    Some("m/s"),
    Some("wind_speed")
);
dpt_alias!(
    DPTPressure2Byte,
    9,
    006,
    Temperature,
    "pressure_2byte",
    Some("Pa"),
    Some("pressure")
);
dpt_alias!(
    DPTHumidity,
    9,
    007,
    Temperature,
    "humidity",
    Some("%"),
    Some("humidity")
);
dpt_alias!(
    DPTPartsPerMillion,
    9,
    008,
    Temperature,
    "ppm",
    Some("ppm"),
    None
);
dpt_alias!(
    DPTAirFlow,
    9,
    009,
    Temperature,
    "air_flow",
    Some("m³/h"),
    None
);
dpt_alias!(DPTTime1, 9, 010, Temperature, "time_1", Some("s"), None);
dpt_alias!(DPTTime2, 9, 011, Temperature, "time_2", Some("ms"), None);
dpt_alias!(
    DPTVoltage,
    9,
    020,
    Temperature,
    "voltage",
    Some("mV"),
    Some("voltage")
);
dpt_alias!(
    DPTCurrent,
    9,
    021,
    Temperature,
    "curr",
    Some("mA"),
    Some("current")
);
dpt_alias!(
    DPTPowerDensity,
    9,
    022,
    Temperature,
    "power_density",
    Some("W/m²"),
    None
);
dpt_alias!(
    DPTKelvinPerPercent,
    9,
    023,
    Temperature,
    "kelvin_per_percent",
    Some("K/%"),
    None
);
dpt_alias!(
    DPTPower2Byte,
    9,
    024,
    Temperature,
    "power_2byte",
    Some("kW"),
    Some("power")
);
dpt_alias!(
    DPTVolumeFlow,
    9,
    025,
    Temperature,
    "volume_flow",
    Some("L/h"),
    None
);
dpt_alias!(
    DPTRainAmount,
    9,
    026,
    Temperature,
    "rain_amount",
    Some("L/m²"),
    None
);
dpt_alias!(
    DPTTemperatureF,
    9,
    027,
    Temperature,
    "temperature_f",
    Some("°F"),
    Some("temperature")
);
dpt_alias!(
    DPTWspKmh,
    9,
    028,
    Temperature,
    "wind_speed_kmh",
    Some("km/h"),
    Some("wind_speed")
);
dpt_alias!(
    DPTAbsoluteHumidity,
    9,
    029,
    Temperature,
    "absolute_humidity",
    Some("g/m³"),
    None
);
dpt_alias!(
    DPTConcentrationUGM3,
    9,
    030,
    Temperature,
    "concentration_ugm3",
    Some("μg/m³"),
    None
);
dpt_alias!(
    DPTEnthalpy,
    9,
    60000,
    Temperature,
    "enthalpy",
    Some("H"),
    None
);
