//! DPT 273 forecast values.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Forecast {
    pub delay_time: Option<u16>,
    pub probability: Option<u8>,
    pub maximum_value: Option<f64>,
    pub minimum_value: Option<f64>,
}
fn bad(details: &str) -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "273".into(),
        details: details.into(),
    }
    .into()
}
pub(super) fn value_subtype(sub: u16) -> u16 {
    match sub {
        1 => 1,
        2 => 5,
        3 | 4 => 7,
        5 => 8,
        6 => 30,
        _ => 22,
    }
}
impl Forecast {
    pub(super) fn encode(self, sub: u16) -> Result<[u8; 8]> {
        if self.delay_time == Some(0)
            || self.probability.is_some_and(|p| p > 100)
            || matches!((self.minimum_value,self.maximum_value), (Some(min),Some(max)) if min > max)
        {
            return Err(bad("Invalid forecast range, delay or probability"));
        }
        let delay = self.delay_time.unwrap_or(0).to_be_bytes();
        let probability = self.probability.map_or(0, |p| {
            (f64::from(p) / 100.0 * 255.0).round_ties_even() as u8
        });
        let max = super::dpt_setpoints::pack(self.maximum_value, value_subtype(sub))?;
        let min = super::dpt_setpoints::pack(self.minimum_value, value_subtype(sub))?;
        let flags = u8::from(self.delay_time.is_some()) << 3
            | u8::from(self.probability.is_some()) << 2
            | u8::from(self.maximum_value.is_some()) << 1
            | u8::from(self.minimum_value.is_some());
        Ok([
            flags,
            delay[0],
            delay[1],
            probability,
            max[0],
            max[1],
            min[0],
            min[1],
        ])
    }
    pub(super) fn decode(raw: &[u8], sub: u16) -> Result<Self> {
        let flags = raw[0];
        let delay = u16::from_be_bytes([raw[1], raw[2]]);
        if flags & 8 != 0 && delay == 0 {
            return Err(bad("Delay must be at least one minute"));
        }
        let value = Self {
            delay_time: (flags & 8 != 0).then_some(delay),
            probability: (flags & 4 != 0)
                .then(|| (f64::from(raw[3]) / 255.0 * 100.0).round_ties_even() as u8),
            maximum_value: if flags & 2 != 0 {
                super::dpt_setpoints::unpack(&raw[4..6], value_subtype(sub))?
            } else {
                None
            },
            minimum_value: if flags & 1 != 0 {
                super::dpt_setpoints::unpack(&raw[6..8], value_subtype(sub))?
            } else {
                None
            },
        };
        if matches!((value.minimum_value,value.maximum_value), (Some(min),Some(max)) if min > max) {
            return Err(bad("Maximum is below minimum"));
        }
        Ok(value)
    }
}
impl std::fmt::Display for Forecast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn p<T: std::fmt::Display>(v: Option<T>) -> String {
            v.map_or_else(|| "none".into(), |x| x.to_string())
        }
        write!(
            f,
            "{},{},{},{}",
            p(self.delay_time),
            p(self.probability),
            p(self.maximum_value),
            p(self.minimum_value)
        )
    }
}
impl std::str::FromStr for Forecast {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        fn p<T: std::str::FromStr>(v: &str) -> Result<Option<T>> {
            if v.trim() == "none" {
                Ok(None)
            } else {
                Ok(Some(
                    v.trim()
                        .parse()
                        .map_err(|_| bad("Invalid forecast field"))?,
                ))
            }
        }
        let mut fields = s.split(',');
        let mut next = || {
            fields
                .next()
                .ok_or_else(|| bad("Expected four forecast fields"))
        };
        let value = Self {
            delay_time: p(next()?)?,
            probability: p(next()?)?,
            maximum_value: p(next()?)?,
            minimum_value: p(next()?)?,
        };
        if fields.next().is_some() {
            return Err(bad("Expected four forecast fields"));
        }
        Ok(value)
    }
}
macro_rules! forecast {
    ($name:ident, $sub:literal, $number:literal, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name([u8; 8]);
        impl $name {
            ///
            /// # Errors
            ///
            /// Returns an error if the value is outside the valid range or cannot be encoded.
            pub fn new(value: Forecast) -> Result<Self> {
                Ok(Self(value.encode($sub)?))
            }
            #[must_use]
            pub fn value(&self) -> Forecast {
                Forecast::decode(&self.0, $sub).expect("validated forecast")
            }
        }
        impl DptValue for $name {
            const DPT_NUMBER: &'static str = $number;
            const VALUE_TYPE: &'static str = $kind;
            const BYTE_LENGTH: usize = 8;
            fn from_bytes(bytes: &[u8]) -> Result<Self> {
                let raw: [u8; 8] = bytes.try_into().map_err(|_| bad("Expected eight bytes"))?;
                Forecast::decode(&raw, $sub)?;
                Ok(Self(raw))
            }
            fn as_bytes(&self) -> &[u8] {
                &self.0
            }
            fn validate(&self) -> Result<()> {
                Forecast::decode(&self.0, $sub).map(|_| ())
            }
            fn value_range() -> (f64, f64) {
                (0.0, 0.0)
            }
        }
        impl WriteValue<$name> for Forecast {
            fn resolve(self, _: DptType) -> Result<$name> {
                $name::new(self)
            }
        }
    };
}
forecast!(DPTForecastTemperature, 1, "273.001", "forecast_temperature");
forecast!(DPTForecastWindSpeed, 2, "273.002", "forecast_wind_speed");
forecast!(
    DPTForecastRelativeHumidity,
    3,
    "273.003",
    "forecast_relative_humidity"
);
forecast!(
    DPTForecastAbsoluteHumidity,
    4,
    "273.004",
    "forecast_absolute_humidity"
);
forecast!(DPTForecastCO2, 5, "273.005", "forecast_co2");
forecast!(
    DPTForecastAirPollutants,
    6,
    "273.006",
    "forecast_air_pollutants"
);
forecast!(
    DPTForecastSunIntensity,
    7,
    "273.007",
    "forecast_sun_intensity"
);

#[derive(Debug, Clone, Copy)]
pub struct ForecastView<'a>(pub(super) &'a [u8], pub(super) u16);
impl<'a> ForecastView<'a> {
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> Forecast {
        Forecast::decode(self.0, self.1).expect("validated forecast")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
