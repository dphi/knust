//! DPT 238 scene configuration and DALI diagnostics.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneConfig {
    pub scene_number: u8,
    pub scene_active: bool,
    pub storage_enabled: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DALIDiagnostics {
    pub address: u8,
    pub lamp_failure: bool,
    pub ballast_failure: bool,
}

fn error(number: &str, details: &str) -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: number.into(),
        details: details.into(),
    }
    .into()
}
impl SceneConfig {
    pub(super) fn encode(self) -> Result<u8> {
        if !(1..=64).contains(&self.scene_number) {
            return Err(error("238.001", "Scene number must be 1..=64"));
        }
        Ok((self.scene_number - 1)
            | (u8::from(!self.scene_active) << 6)
            | (u8::from(!self.storage_enabled) << 7))
    }
    pub(super) fn decode(raw: u8) -> Self {
        Self {
            scene_number: (raw & 63) + 1,
            scene_active: raw & 64 == 0,
            storage_enabled: raw & 128 == 0,
        }
    }
}
impl DALIDiagnostics {
    pub(super) fn encode(self) -> Result<u8> {
        if self.address > 63 {
            return Err(error("238.600", "DALI address must be 0..=63"));
        }
        Ok(self.address
            | (u8::from(self.lamp_failure) << 6)
            | (u8::from(self.ballast_failure) << 7))
    }
    pub(super) fn decode(raw: u8) -> Self {
        Self {
            address: raw & 63,
            lamp_failure: raw & 64 != 0,
            ballast_failure: raw & 128 != 0,
        }
    }
}
macro_rules! text_value {
    ($type:ident, $number:literal, $a:ident, $b:ident, $c:ident) => {
        impl std::fmt::Display for $type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{},{},{}", self.$a, self.$b, self.$c)
            }
        }
        impl std::str::FromStr for $type {
            type Err = crate::error::KnxError;
            fn from_str(s: &str) -> Result<Self> {
                let bad = || error($number, "Expected number,flag,flag");
                let mut p = s.split(',').map(str::trim);
                let value = Self {
                    $a: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
                    $b: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
                    $c: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
                };
                if p.next().is_some() {
                    return Err(bad());
                }
                value.encode()?;
                Ok(value)
            }
        }
    };
}
text_value!(
    SceneConfig,
    "238.001",
    scene_number,
    scene_active,
    storage_enabled
);
text_value!(
    DALIDiagnostics,
    "238.600",
    address,
    lamp_failure,
    ballast_failure
);
macro_rules! datapoint {
    ($name:ident, $value:ty, $number:literal, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name([u8; 1]);
        impl $name {
            pub fn new(value: $value) -> Result<Self> {
                Ok(Self([value.encode()?]))
            }
            #[must_use]
            pub fn value(&self) -> $value {
                <$value>::decode(self.0[0])
            }
        }
        impl DptValue for $name {
            const DPT_NUMBER: &'static str = $number;
            const VALUE_TYPE: &'static str = $kind;
            const BYTE_LENGTH: usize = 1;
            fn from_bytes(bytes: &[u8]) -> Result<Self> {
                let [raw] = bytes else {
                    return Err(error($number, "Expected one byte"));
                };
                Ok(Self([*raw]))
            }
            fn as_bytes(&self) -> &[u8] {
                &self.0
            }
            fn validate(&self) -> Result<()> {
                Ok(())
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
datapoint!(DPTSceneConfig, SceneConfig, "238.001", "scene_config");
datapoint!(
    DPTDALIDiagnostics,
    DALIDiagnostics,
    "238.600",
    "dali_diagnostics"
);

#[derive(Debug, Clone, Copy)]
pub struct SceneConfigView<'a>(pub(super) &'a [u8]);
impl<'a> SceneConfigView<'a> {
    #[must_use]
    pub fn value(&self) -> SceneConfig {
        SceneConfig::decode(self.0[0])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DALIDiagnosticsView<'a>(pub(super) &'a [u8]);
impl<'a> DALIDiagnosticsView<'a> {
    #[must_use]
    pub fn value(&self) -> DALIDiagnostics {
        DALIDiagnostics::decode(self.0[0])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
