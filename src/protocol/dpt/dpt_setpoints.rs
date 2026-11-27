//! DPT 222 and 275 room temperature setpoints and shifts.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomTemperatureSetpoints3 {
    pub comfort: Option<f64>,
    pub standby: Option<f64>,
    pub economy: Option<f64>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomTemperatureSetpoints4 {
    pub comfort: Option<f64>,
    pub standby: Option<f64>,
    pub economy: Option<f64>,
    pub building_protection: Option<f64>,
}

/// Temperature shifts for comfort, standby and economy.
pub type RoomTemperatureSetpointShifts3 = RoomTemperatureSetpoints3;
/// Temperature shifts including building protection.
pub type RoomTemperatureSetpointShifts4 = RoomTemperatureSetpoints4;

fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "setpoints".into(),
        details: "Expected comma-separated numbers or none".into(),
    }
    .into()
}
pub(super) fn pack(value: Option<f64>, sub: u16) -> Result<[u8; 2]> {
    let Some(value) = value else {
        return Ok([0x7f, 0xff]);
    };
    super::dpt9::validate_subtype(sub, value)?;
    let mut raw = [0; 2];
    super::dpt9::encode_2byte_float_to_bytes(value, &mut raw)?;
    super::dpt9::validate_subtype(sub, super::dpt9::decode_wire_f64(&raw))?;
    Ok(raw)
}
pub(super) fn unpack(raw: &[u8], sub: u16) -> Result<Option<f64>> {
    if raw == [0x7f, 0xff] {
        return Ok(None);
    }
    let value = super::dpt9::decode_wire_f64(raw);
    super::dpt9::validate_subtype(sub, value)?;
    Ok(Some(value))
}
fn parse_field(text: &str) -> Result<Option<f64>> {
    if text.trim() == "none" {
        Ok(None)
    } else {
        Ok(Some(text.trim().parse().map_err(|_| bad())?))
    }
}
fn print_field(value: Option<f64>) -> String {
    value.map_or_else(|| "none".into(), |v| v.to_string())
}
macro_rules! setpoints {
    ($type:ident, $count:expr, $($field:ident),+) => {
        impl $type {
            pub(super) fn encode(self, shift: bool) -> Result<Vec<u8>> {
                let mut raw = Vec::with_capacity($count * 2);
                $(raw.extend(pack(self.$field, if shift { 2 } else { 1 })?);)+
                Ok(raw)
            }
            pub(super) fn decode(raw: &[u8], shift: bool) -> Result<Self> {
                let mut fields = raw.chunks_exact(2);
                Ok(Self { $($field: unpack(fields.next().expect("validated length"), if shift { 2 } else { 1 })?,)+ })
            }
        }
        impl std::fmt::Display for $type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let parts = [$(print_field(self.$field)),+]; write!(f, "{}", parts.join(","))
            }
        }
        impl std::str::FromStr for $type {
            type Err = crate::error::KnxError;
            fn from_str(s: &str) -> Result<Self> {
                let mut parts = s.split(',');
                let value = Self { $($field: parse_field(parts.next().ok_or_else(bad)?)?,)+ };
                if parts.next().is_some() { return Err(bad()); } Ok(value)
            }
        }
    };
}
setpoints!(RoomTemperatureSetpoints3, 3, comfort, standby, economy);
setpoints!(
    RoomTemperatureSetpoints4,
    4,
    comfort,
    standby,
    economy,
    building_protection
);

macro_rules! datapoint {
    ($name:ident, $value:ident, $length:expr, $shift:expr, $number:literal, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name([u8; $length]);
        impl $name {
            ///
            /// # Errors
            ///
            /// Returns an error if the value is outside the valid range or cannot be encoded.
            pub fn new(value: $value) -> Result<Self> {
                Ok(Self(value.encode($shift)?.try_into().expect("fixed size")))
            }
            #[must_use]
            pub fn value(&self) -> $value {
                $value::decode(&self.0, $shift).expect("validated fields")
            }
        }
        impl DptValue for $name {
            const DPT_NUMBER: &'static str = $number;
            const VALUE_TYPE: &'static str = $kind;
            const BYTE_LENGTH: usize = $length;
            fn from_bytes(bytes: &[u8]) -> Result<Self> {
                let raw: [u8; $length] = bytes.try_into().map_err(|_| bad())?;
                $value::decode(&raw, $shift)?;
                Ok(Self(raw))
            }
            fn as_bytes(&self) -> &[u8] {
                &self.0
            }
            fn validate(&self) -> Result<()> {
                $value::decode(&self.0, $shift).map(|_| ())
            }
            fn value_range() -> (f64, f64) {
                (0.0, 0.0)
            }
        }
        impl WriteValue<$name> for $value {
            fn resolve(self, _: DptType) -> Result<$name> {
                $name::new(self)
            }
        }
    };
}
datapoint!(
    DPTRoomTemperatureSetpoints3,
    RoomTemperatureSetpoints3,
    6,
    false,
    "222.100",
    "room_temperature_setpoints_3"
);
datapoint!(
    DPTRoomTemperatureSetpointShifts3,
    RoomTemperatureSetpoints3,
    6,
    true,
    "222.101",
    "room_temperature_setpoint_shifts_3"
);
datapoint!(
    DPTRoomTemperatureSetpoints4,
    RoomTemperatureSetpoints4,
    8,
    false,
    "275.100",
    "room_temperature_setpoints_4"
);
datapoint!(
    DPTRoomTemperatureSetpointShifts4,
    RoomTemperatureSetpoints4,
    8,
    true,
    "275.101",
    "room_temperature_setpoint_shifts_4"
);

#[derive(Debug, Clone, Copy)]
pub struct Setpoints3View<'a>(pub(super) &'a [u8], pub(super) bool);
impl<'a> Setpoints3View<'a> {
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> RoomTemperatureSetpoints3 {
        RoomTemperatureSetpoints3::decode(self.0, self.1).expect("validated fields")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Setpoints4View<'a>(pub(super) &'a [u8], pub(super) bool);
impl<'a> Setpoints4View<'a> {
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> RoomTemperatureSetpoints4 {
        RoomTemperatureSetpoints4::decode(self.0, self.1).expect("validated fields")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
