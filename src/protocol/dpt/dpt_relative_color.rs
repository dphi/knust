//! Relative colour controls built from DPT 3 dimming steps.
use super::{ControlDimming, DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "relative color".into(),
        details: "Invalid relative colour control".into(),
    }
    .into()
}
trait Component: Sized {
    fn raw(&self) -> (u8, bool);
    fn decode(raw: u8, valid: bool) -> Result<Self>;
    fn parse(value: u8) -> Result<Self>;
    fn display(&self) -> char;
}
impl Component for ControlDimming {
    fn raw(&self) -> (u8, bool) {
        ((u8::from(self.control()) << 3) | self.step_code(), true)
    }
    fn decode(raw: u8, _: bool) -> Result<Self> {
        ControlDimming::new(raw & 8 != 0, raw & 7)
    }
    fn parse(value: u8) -> Result<Self> {
        let raw = char::from(value).to_digit(16).ok_or_else(bad)? as u8;
        <Self as Component>::decode(raw, true)
    }
    fn display(&self) -> char {
        char::from_digit(u32::from(self.raw().0), 16).expect("nibble")
    }
}
impl Component for Option<ControlDimming> {
    fn raw(&self) -> (u8, bool) {
        self.as_ref().map_or((0, false), Component::raw)
    }
    fn decode(raw: u8, valid: bool) -> Result<Self> {
        if valid {
            Ok(Some(<ControlDimming as Component>::decode(raw, true)?))
        } else {
            Ok(None)
        }
    }
    fn parse(value: u8) -> Result<Self> {
        if value == b'-' {
            Ok(None)
        } else {
            Ok(Some(ControlDimming::parse(value)?))
        }
    }
    fn display(&self) -> char {
        self.as_ref().map_or('-', Component::display)
    }
}
macro_rules! relative {
    ($value:ident,$dpt:ident,$length:expr,$number:literal,$kind:literal,$masked:expr,$($field:ident:$ft:ty => $index:expr => $bit:expr),+) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)] pub struct $value { $(pub $field: $ft,)+ }
        impl $value {
            pub(super) fn encode(&self) -> [u8;$length] {
                let mut raw = [0;$length]; $(let (v,valid) = self.$field.raw(); raw[$index] = v; if $masked && valid { raw[$length-1] |= 1 << $bit; })+ raw
            }
            pub(super) fn decode(raw: &[u8]) -> Result<Self> {
                Ok(Self { $($field: <$ft as Component>::decode(raw[$index],!$masked || raw[$length-1] & (1 << $bit) != 0)?,)+ })
            }
        }
        impl std::fmt::Display for $value {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { $(write!(f,"{}",self.$field.display())?;)+ Ok(()) }
        }
        impl std::str::FromStr for $value {
            type Err = crate::error::KnxError;
            fn from_str(s: &str) -> Result<Self> {
                let mut bytes = s.bytes(); let value = Self { $($field: <$ft as Component>::parse(bytes.next().ok_or_else(bad)?)?,)+ };
                if bytes.next().is_some() { return Err(bad()); } Ok(value)
            }
        }
        #[derive(Debug, Clone, PartialEq, Eq, Hash)] pub struct $dpt([u8;$length]);
        impl $dpt {
            #[must_use] pub fn new(value: $value) -> Self { Self(value.encode()) }
            #[must_use] pub fn value(&self) -> $value { $value::decode(&self.0).expect("validated length") }
        }
        impl DptValue for $dpt {
            const DPT_NUMBER: &'static str = $number; const VALUE_TYPE: &'static str = $kind; const BYTE_LENGTH: usize = $length;
            fn from_bytes(bytes: &[u8]) -> Result<Self> { Ok(Self(bytes.try_into().map_err(|_|bad())?)) }
            fn as_bytes(&self) -> &[u8] { &self.0 }
            fn validate(&self) -> Result<()> { Ok(()) }
            fn value_range() -> (f64,f64) { (0.0,0.0) }
        }
        impl WriteValue<$dpt> for $value { fn resolve(self, _: DptType) -> Result<$dpt> { Ok($dpt::new(self)) } }
    };
}

pub(super) fn decode_payload(raw: &[u8], main: u16) -> Result<super::DptPayload> {
    use super::DptPayload;
    match main {
        250 => Ok(DptPayload::ColorTemperatureControl(
            ColorTemperatureControl::decode(raw)?,
        )),
        252 => Ok(DptPayload::RelativeControlRGBW(
            RelativeControlRGBW::decode(raw)?,
        )),
        253 => Ok(DptPayload::RelativeControlXYY(RelativeControlXYY::decode(
            raw,
        )?)),
        254 => Ok(DptPayload::RelativeControlRGB(RelativeControlRGB::decode(
            raw,
        )?)),
        _ => Err(bad()),
    }
}
pub(super) fn parse_payload(s: &str, main: u16) -> Result<super::DptPayload> {
    use super::DptPayload;
    match main {
        250 => s.parse().map(DptPayload::ColorTemperatureControl),
        252 => s.parse().map(DptPayload::RelativeControlRGBW),
        253 => s.parse().map(DptPayload::RelativeControlXYY),
        254 => s.parse().map(DptPayload::RelativeControlRGB),
        _ => Err(bad()),
    }
}
relative!(ColorTemperatureControl,DPTColorTemperatureControl,3,"250.600","color_temperature_control",true,color_temperature:Option<ControlDimming> => 0 => 1,brightness:Option<ControlDimming> => 1 => 0);
relative!(RelativeControlRGBW,DPTRelativeControlRGBW,5,"252.600","relative_control_rgbw",true,red:Option<ControlDimming> => 0 => 3,green:Option<ControlDimming> => 1 => 2,blue:Option<ControlDimming> => 2 => 1,white:Option<ControlDimming> => 3 => 0);
relative!(RelativeControlXYY,DPTRelativeControlXYY,4,"253.600","relative_control_xyy",true,saturation:Option<ControlDimming> => 0 => 2,colour:Option<ControlDimming> => 1 => 1,brightness:Option<ControlDimming> => 2 => 0);
relative!(RelativeControlRGB,DPTRelativeControlRGB,3,"254.600","relative_control_rgb",false,red:ControlDimming => 0 => 0,green:ControlDimming => 1 => 0,blue:ControlDimming => 2 => 0);

#[derive(Debug, Clone, Copy)]
pub struct RelativeColorView<'a>(pub(super) &'a [u8], pub(super) u16);
impl<'a> RelativeColorView<'a> {
    #[must_use]
    pub fn value(&self) -> super::DptPayload {
        decode_payload(self.0, self.1).expect("validated length")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
