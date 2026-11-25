//! DPT 207 actuator status and demand values.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

fn bad(number: &str) -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: number.into(),
        details: "Invalid percentage or status fields".into(),
    }
    .into()
}
trait Percent: Sized + Copy {
    fn encode(self, scaled: bool, number: &str) -> Result<(u8, bool)>;
    fn decode(raw: u8, valid: bool, scaled: bool, number: &str) -> Result<Self>;
    fn parse(text: &str, number: &str) -> Result<Self>;
    fn display(self) -> String;
}
impl Percent for u8 {
    fn encode(self, scaled: bool, number: &str) -> Result<(u8, bool)> {
        if self > 100 {
            return Err(bad(number));
        }
        Ok((
            if scaled {
                (f64::from(self) / 100.0 * 255.0).round_ties_even() as u8
            } else {
                self
            },
            true,
        ))
    }
    fn decode(raw: u8, _: bool, scaled: bool, number: &str) -> Result<Self> {
        if !scaled && raw > 100 {
            return Err(bad(number));
        }
        Ok(if scaled {
            (f64::from(raw) / 255.0 * 100.0).round_ties_even() as u8
        } else {
            raw
        })
    }
    fn parse(text: &str, number: &str) -> Result<Self> {
        text.parse().map_err(|_| bad(number))
    }
    fn display(self) -> String {
        self.to_string()
    }
}
impl Percent for Option<u8> {
    fn encode(self, scaled: bool, number: &str) -> Result<(u8, bool)> {
        match self {
            None => Ok((0, false)),
            Some(value) => value.encode(scaled, number),
        }
    }
    fn decode(raw: u8, valid: bool, scaled: bool, number: &str) -> Result<Self> {
        if valid {
            Ok(Some(u8::decode(raw, true, scaled, number)?))
        } else {
            Ok(None)
        }
    }
    fn parse(text: &str, number: &str) -> Result<Self> {
        if text == "none" {
            Ok(None)
        } else {
            Ok(Some(<u8 as Percent>::parse(text, number)?))
        }
    }
    fn display(self) -> String {
        self.map_or_else(|| "none".into(), |value| value.to_string())
    }
}
macro_rules! actuator {
    ($value:ident,$dpt:ident,$sub:literal,$number:literal,$kind:literal,$percent:ident:$pt:ty,$optional:expr,$scaled:expr,$($field:ident:$bit:expr),+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub struct $value { pub $percent: $pt, $(pub $field: bool,)+ }
        impl $value {
            pub(super) fn encode(self) -> Result<[u8;2]> {
                let (percent,valid) = self.$percent.encode($scaled,$number)?;
                let mut flags = if $optional && valid { 1 } else { 0 }; $(if self.$field { flags |= 1 << $bit; })+
                Ok([percent,flags])
            }
            pub(super) fn decode(raw: [u8;2]) -> Result<Self> {
                Ok(Self { $percent: <$pt as Percent>::decode(raw[0], !$optional || raw[1]&1 != 0, $scaled, $number)?, $($field: raw[1] & (1 << $bit) != 0,)+ })
            }
        }
        impl std::fmt::Display for $value {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{},",self.$percent.display())?; $(write!(f,"{}",u8::from(self.$field))?;)+ Ok(()) }
        }
        impl std::str::FromStr for $value {
            type Err = crate::error::KnxError;
            fn from_str(s: &str) -> Result<Self> {
                let (percent,flags) = s.split_once(',').ok_or_else(||bad($number))?; let mut flags = flags.bytes();
                let mut next = || -> Result<bool> { match flags.next() { Some(b'0')=>Ok(false),Some(b'1')=>Ok(true),_=>Err(bad($number)) } };
                let value = Self { $percent: <$pt as Percent>::parse(percent.trim(),$number)?, $($field: next()?,)+ };
                if flags.next().is_some() { return Err(bad($number)); } value.encode()?; Ok(value)
            }
        }
        #[derive(Debug, Clone, PartialEq, Eq, Hash)] pub struct $dpt([u8;2]);
        impl $dpt {
            pub fn new(value: $value) -> Result<Self> { Ok(Self(value.encode()?)) }
            #[must_use] pub fn value(&self) -> $value { $value::decode(self.0).expect("validated percentage") }
        }
        impl DptValue for $dpt {
            const DPT_NUMBER: &'static str = $number; const VALUE_TYPE: &'static str = $kind; const BYTE_LENGTH: usize = 2;
            fn from_bytes(bytes: &[u8]) -> Result<Self> { let raw = bytes.try_into().map_err(|_|bad($number))?; $value::decode(raw)?; Ok(Self(raw)) }
            fn as_bytes(&self) -> &[u8] { &self.0 }
            fn validate(&self) -> Result<()> { $value::decode(self.0).map(|_|()) }
            fn value_range() -> (f64,f64) { (0.0,0.0) }
        }
        impl WriteValue<$dpt> for $value { fn resolve(self, _: DptType) -> Result<$dpt> { $dpt::new(self) } }
    };
}

pub(super) fn decode_payload(raw: [u8; 2], sub: u16) -> Result<super::DptPayload> {
    use super::DptPayload;
    match sub {
        100 => Ok(DptPayload::BurnerControllerStatus(
            BurnerControllerStatus::decode(raw)?,
        )),
        101 => Ok(DptPayload::LockingSignal(LockingSignal::decode(raw)?)),
        102 => Ok(DptPayload::BoilerControllerDemand(
            BoilerControllerDemand::decode(raw)?,
        )),
        104 => Ok(DptPayload::ActuatorPositionDemand(
            ActuatorPositionDemand::decode(raw)?,
        )),
        105 => Ok(DptPayload::ActuatorPositionStatus(
            ActuatorPositionStatus::decode(raw)?,
        )),
        600 => Ok(DptPayload::LightingActuatorStatus(
            LightingActuatorStatus::decode(raw)?,
        )),
        _ => Err(bad("207")),
    }
}
pub(super) fn parse_payload(value: &str, sub: u16) -> Result<super::DptPayload> {
    use super::DptPayload;
    match sub {
        100 => value.parse().map(DptPayload::BurnerControllerStatus),
        101 => value.parse().map(DptPayload::LockingSignal),
        102 => value.parse().map(DptPayload::BoilerControllerDemand),
        104 => value.parse().map(DptPayload::ActuatorPositionDemand),
        105 => value.parse().map(DptPayload::ActuatorPositionStatus),
        600 => value.parse().map(DptPayload::LightingActuatorStatus),
        _ => Err(bad("207")),
    }
}
actuator!(BurnerControllerStatus,DPTStatusBUC,100,"207.100","burner_controller_status",relative_power:Option<u8>,true,false,fault:1,stage_1_active:2,stage_2_active:3);
actuator!(LockingSignal,DPTLockSign,101,"207.101","locking_signal",power_reduction:Option<u8>,true,false,critical:1);
actuator!(BoilerControllerDemand,DPTValueDemBOC,102,"207.102","boiler_controller_demand",relative_demand:u8,false,false,stage_1_control:0,stage_2_control:1);
actuator!(ActuatorPositionDemand,DPTActPosDemAbs,104,"207.104","actuator_position_demand",position:Option<u8>,true,false,absolute_load_priority:1,shift_load_priority:2,emergency_demand:3);
actuator!(ActuatorPositionStatus,DPTStatusAct,105,"207.105","actuator_position_status",position:u8,false,false,failure:0,manual_override:1,calibration_mode:2,valve_kick:3,synchronization_mode:4);
actuator!(LightingActuatorStatus,DPTStatusLightingActuator,600,"207.600","lighting_actuator_status",lighting_level:Option<u8>,true,true,locked:1,forced:2,night_mode_active:3,staircase_lighting_function:4,dimming:5,local_override:6,failure:7);

#[derive(Debug, Clone, Copy)]
pub struct ActuatorView<'a>(pub(super) &'a [u8], pub(super) u16);
impl<'a> ActuatorView<'a> {
    #[must_use]
    pub fn value(&self) -> super::DptPayload {
        decode_payload([self.0[0], self.0[1]], self.1).expect("validated actuator")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
