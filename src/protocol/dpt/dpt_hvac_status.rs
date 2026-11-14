//! Combined room-control and valve actuator status bitsets.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

fn bad(number: &str) -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: number.into(),
        details: "Invalid status bitset".into(),
    }
    .into()
}
macro_rules! status {
    ($value:ident,$dpt:ident,$length:expr,$number:literal,$kind:literal,$($field:ident:$bit:expr),+) => {
        #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $value { $(pub $field: bool,)+ }
        impl $value {
            pub(super) fn encode(self) -> [u8; $length] {
                let mut raw = 0u16; $(if self.$field { raw |= 1 << $bit; })+
                let bytes = raw.to_be_bytes(); let mut result = [0; $length]; result.copy_from_slice(&bytes[2-$length..]); result
            }
            pub(super) fn decode(bytes: &[u8]) -> Self {
                let raw = if $length == 1 { u16::from(bytes[0]) } else { u16::from_be_bytes([bytes[0], bytes[1]]) };
                Self { $($field: raw & (1 << $bit) != 0,)+ }
            }
        }
        impl std::fmt::Display for $value {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { $(write!(f, "{}", u8::from(self.$field))?;)+ Ok(()) }
        }
        impl std::str::FromStr for $value {
            type Err = crate::error::KnxError;
            fn from_str(s: &str) -> Result<Self> {
                let mut bytes = s.bytes();
                let mut next = || -> Result<bool> { match bytes.next() { Some(b'0') => Ok(false), Some(b'1') => Ok(true), _ => Err(bad($number)) } };
                let result = Self { $($field: next()?,)+ };
                if bytes.next().is_some() { return Err(bad($number)); } Ok(result)
            }
        }
        #[derive(Debug, Clone, PartialEq, Eq, Hash)] pub struct $dpt([u8; $length]);
        impl $dpt {
            #[must_use] pub fn new(value: $value) -> Self { Self(value.encode()) }
            #[must_use] pub fn value(&self) -> $value { $value::decode(&self.0) }
        }
        impl DptValue for $dpt {
            const DPT_NUMBER: &'static str = $number; const VALUE_TYPE: &'static str = $kind; const BYTE_LENGTH: usize = $length;
            fn from_bytes(bytes: &[u8]) -> Result<Self> { Ok(Self(bytes.try_into().map_err(|_| bad($number))?)) }
            fn as_bytes(&self) -> &[u8] { &self.0 }
            fn validate(&self) -> Result<()> { Ok(()) }
            fn value_range() -> (f64,f64) { (0.0,0.0) }
        }
        impl WriteValue<$dpt> for $value { fn resolve(self, _: DptType) -> Result<$dpt> { Ok($dpt::new(self)) } }
    };
}
status!(CombinedStatusRTSM,DPTCombinedStatusRTSM,1,"21.107","combined_status_rtsm",window_open:0,presence:1,comfort_push_button:2,comfort_prolongation_user:3,hvac_mode_user:4);
status!(CombinedStatusHVA,DPTCombinedStatusHVA,2,"22.102","combined_status_hva",valve_open:0,short_circuit:1,overload:2,valve_kick:3,service_mode:4,manual_operation:5,forced_position:6,locked_position:7,calibration_mode:8);
// In KNX heat/cool encoding, true selects heating and false selects cooling.
status!(CombinedStatusRTC,DPTCombinedStatusRTC,2,"22.103","combined_status_rtc",fault:0,heating_mode:1,dew_point_status:2,frost_alarm:3,overheat_alarm:4,controller_inactive:5,additional_stage:6,heating_enabled:7,cooling_enabled:8);
