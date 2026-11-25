//! Typed codecs and runtime decoding for KNX datapoint types.
//!
//! Family modules such as [`dpt246`] contain the wire codec and borrowed view.
//! Common values such as [`BatteryInfo`] are also available directly in this
//! module. Use [`DptType::decode_ref`] when the datapoint number is known at runtime.

use crate::error::{ProtocolError, Result};

mod decode;
mod dpt_type;
mod group_registry;
mod parse;
mod payload;
mod registry;
mod unit;
mod view;

// DPT type modules - organized by DPT number like Python implementation
pub mod dpt1; // Boolean
pub mod dpt10; // Time
pub mod dpt11; // Date
pub mod dpt12; // Unsigned 32-bit
pub mod dpt13; // Signed 32-bit
pub mod dpt14;
pub mod dpt15; // 4-byte float
pub mod dpt16; // String
pub mod dpt17; // Scene Number
pub mod dpt18; // Scene Control
pub mod dpt19; // Date and Time
pub mod dpt2; // 2-bit control
pub mod dpt20; // HVAC
pub mod dpt206;
pub mod dpt207;
mod dpt20_modes;
pub mod dpt217;
pub mod dpt219; // Version
pub mod dpt225;
pub mod dpt229;
pub mod dpt230;
pub mod dpt232; // RGB Color
pub mod dpt234;
pub mod dpt235;
pub mod dpt236;
pub mod dpt237;
pub mod dpt238;
pub mod dpt240;
pub mod dpt241; // Tariff Active Energy
pub mod dpt242;
pub mod dpt243; // XYY Color
pub mod dpt244;
pub mod dpt245;
pub mod dpt246;
pub mod dpt247;
pub mod dpt248;
pub mod dpt249;
pub mod dpt25;
pub mod dpt251;
pub mod dpt255;
pub mod dpt26;
pub mod dpt27;
pub mod dpt272;
pub mod dpt273;
pub mod dpt29; // Signed 64-bit
pub mod dpt3; // Control
pub mod dpt30;
pub mod dpt4; // Character
pub mod dpt5; // Unsigned 8-bit
pub mod dpt6; // Signed 8-bit
pub mod dpt7; // Unsigned 16-bit
pub mod dpt8; // Signed 16-bit
pub mod dpt9;
mod dpt_hvac_status;
mod dpt_relative_color;
mod dpt_setpoints; // 2-byte float // RGBW Color

#[cfg(test)]
mod tests;

// Re-export all DPT types for convenience
pub use decode::{DecodedTelegram, decode_telegram};
pub use dpt_hvac_status::*;
pub use dpt_relative_color::*;
pub use dpt_setpoints::*;
pub use dpt_type::DptType;
pub use dpt1::*;
pub use dpt2::*;
pub use dpt3::*;
pub use dpt4::*;
pub use dpt5::*;
pub use dpt6::*;
pub use dpt7::*;
pub use dpt8::*;
pub use dpt9::*;
pub use dpt10::*;
pub use dpt11::*;
pub use dpt12::*;
pub use dpt13::*;
pub use dpt14::*;
pub use dpt15::*;
pub use dpt16::*;
pub use dpt17::*;
pub use dpt18::*;
pub use dpt19::*;
pub use dpt20::*;
pub use dpt20_modes::*;
pub use dpt25::*;
pub use dpt26::*;
pub use dpt27::*;
pub use dpt29::*;
pub use dpt30::*;
pub use dpt206::*;
pub use dpt207::*;
pub use dpt217::*;
pub use dpt219::*;
pub use dpt225::*;
pub use dpt229::*;
pub use dpt230::*;
pub use dpt232::*;
pub use dpt234::*;
pub use dpt235::*;
pub use dpt236::*;
pub use dpt237::*;
pub use dpt238::*;
pub use dpt240::*;
pub use dpt241::*;
pub use dpt242::*;
pub use dpt243::*;
pub use dpt244::*;
pub use dpt245::*;
pub use dpt246::{BatteryInfo, DPTBatteryInfo};
pub use dpt247::*;
pub use dpt248::*;
pub use dpt249::*;
pub use dpt251::*;
pub use dpt255::*;
pub use dpt272::*;
pub use dpt273::*;
pub(crate) use group_registry::GroupValueUpdate;
pub use group_registry::{GroupAddressRegistry, GroupAddressState};
pub use payload::DptPayload;
pub use registry::{DptMetadata, DptRegistry};
pub use unit::Unit;
pub use view::{
    BoolView, Control2View, ControlView, DateTimeView, DateView, DptView, EnumView, Float2ByteView,
    Float4ByteView, I8View, I16View, I32View, I64View, RgbView, RgbwView, SceneView, TimeView,
    U8View, U16View, U32View, XyyView,
};

/// Macro for creating simple DPT aliases that delegate to existing types
#[macro_export]
macro_rules! dpt_alias {
    ($name:ident, $main:literal, $sub:literal, $base:ty, $value_type:literal, $unit:expr, $ha_class:expr) => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name($base);

        impl $name {
            pub fn new(value: <$base as $crate::protocol::dpt::DptInnerType>::InnerType) -> Self {
                Self(<$base as $crate::protocol::dpt::DptInnerType>::new(value))
            }

            pub fn value(&self) -> <$base as $crate::protocol::dpt::DptInnerType>::InnerType {
                self.0.clone().into_inner()
            }
        }

        impl From<<$base as $crate::protocol::dpt::DptInnerType>::InnerType> for $name {
            fn from(value: <$base as $crate::protocol::dpt::DptInnerType>::InnerType) -> Self {
                Self::new(value)
            }
        }

        impl $crate::protocol::dpt::WriteValue<$name>
            for <$base as $crate::protocol::dpt::DptInnerType>::InnerType
        {
            fn resolve(self, _dpt: $crate::protocol::dpt::DptType) -> $crate::error::Result<$name> {
                Ok($name::new(self))
            }
        }

        impl $crate::protocol::dpt::DptValue for $name {
            const DPT_NUMBER: &'static str = concat!(stringify!($main), ".", stringify!($sub));
            const VALUE_TYPE: &'static str = $value_type;
            const UNIT: Option<&'static str> = $unit;
            const HA_DEVICE_CLASS: Option<&'static str> = $ha_class;
            const BYTE_LENGTH: usize = <$base>::BYTE_LENGTH;

            fn from_bytes(bytes: &[u8]) -> $crate::error::Result<Self> {
                let base_value = <$base>::from_bytes(bytes)?;
                Ok(Self(base_value))
            }

            fn as_bytes(&self) -> &[u8] {
                self.0.as_bytes()
            }

            fn validate(&self) -> $crate::error::Result<()> {
                self.0.validate()
            }

            fn value_range() -> (f64, f64) {
                <$base>::value_range()
            }
        }
    };
}

// Helper trait to extract inner type from DPT types
pub trait DptInnerType {
    type InnerType;
    fn new(value: Self::InnerType) -> Self;
    fn into_inner(self) -> Self::InnerType;
}

/// True zero-copy DPT trait - works directly with byte slice views
pub trait DptValue: Sized + Send + Sync + std::fmt::Debug + Clone + PartialEq + 'static {
    /// DPT number as const generic identifier
    const DPT_NUMBER: &'static str;
    /// Value type identifier for API compatibility
    const VALUE_TYPE: &'static str;
    /// Unit of measurement (if applicable)
    const UNIT: Option<&'static str> = None;
    /// Home Assistant device class mapping (if applicable)
    const HA_DEVICE_CLASS: Option<&'static str> = None;
    /// Byte length of encoded value as const generic
    const BYTE_LENGTH: usize;

    /// Parse directly from byte slice - true zero-copy
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`]
    /// if `bytes` has the wrong length for this DPT or encodes an
    /// out-of-range value.
    fn from_bytes(bytes: &[u8]) -> Result<Self>;

    /// Get byte representation - returns view into internal data (true zero-copy)
    fn as_bytes(&self) -> &[u8];

    /// Validate value range and constraints
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`]
    /// if the value violates this DPT's range or format constraints.
    fn validate(&self) -> Result<()>;

    /// Get the valid value range for this DPT (min, max)
    fn value_range() -> (f64, f64);

    /// Convenience method for backward compatibility - allocates
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::from_bytes`].
    fn decode(bytes: &[u8]) -> Result<Self> {
        Self::from_bytes(bytes)
    }

    /// Convenience method for backward compatibility - allocates
    ///
    /// # Errors
    ///
    /// Infallible for the default implementation; overriding implementations
    /// may return [`ProtocolError::DptError`].
    fn encode(&self) -> Result<Vec<u8>> {
        Ok(self.as_bytes().to_vec())
    }

    /// The runtime [`DptType`] for [`Self::DPT_NUMBER`].
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if no runtime `DptType` is
    /// registered for [`Self::DPT_NUMBER`].
    fn dpt_type() -> Result<DptType> {
        Self::DPT_NUMBER.parse().map_err(|_| {
            ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: "no runtime DptType registered for this DPT number".to_string(),
            }
            .into()
        })
    }

    /// Get DPT metadata for runtime introspection
    #[must_use]
    fn metadata() -> DptMetadata {
        DptMetadata {
            dpt_number: Self::DPT_NUMBER,
            value_type: Self::VALUE_TYPE,
            unit: Self::UNIT,
            ha_device_class: Self::HA_DEVICE_CLASS,
            byte_length: Self::BYTE_LENGTH,
        }
    }
}

/// Something that can resolve into a concrete `T: DptValue` — either `T`
/// itself (or, for DPTs generated by [`dpt_alias`], its
/// plain inner value), or a human string parsed against `T`'s own DPT
/// number at runtime. This is what lets a single `write` call accept
/// `DPTSwitch::new(true)`, `true`, or `"on"` without losing the
/// compile-time guarantee that the *shape* of value matches the bound DPT.
/// Values can still fail runtime validation for their specific DPT.
pub trait WriteValue<T: DptValue> {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if this input doesn't resolve to
    /// a valid `T` for `dpt`.
    fn resolve(self, dpt: DptType) -> Result<T>;
}

impl<T: DptValue> WriteValue<T> for T {
    fn resolve(self, _dpt: DptType) -> Result<T> {
        Ok(self)
    }
}

impl<T: DptValue> WriteValue<T> for &str {
    fn resolve(self, dpt: DptType) -> Result<T> {
        let bytes = dpt.parse(self)?;
        T::decode(&bytes)
    }
}

/// Type-safe DPT wrapper with const generics for compile-time validation
#[derive(Debug, Clone, PartialEq)]
pub struct Dpt<T: DptValue> {
    value: T,
}

impl<T: DptValue> Dpt<T> {
    /// Create new DPT value with validation
    ///
    /// # Errors
    ///
    /// Returns an error if `value.validate()` fails.
    pub fn new(value: T) -> Result<Self> {
        value.validate()?;
        Ok(Self { value })
    }

    /// Get the inner value
    pub fn value(&self) -> &T {
        &self.value
    }

    /// Encode to bytes
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`DptValue::encode`].
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(self.value.as_bytes().to_vec())
    }

    /// Get byte slice - true zero-copy
    pub fn as_bytes(&self) -> &[u8] {
        self.value.as_bytes()
    }

    /// Decode from bytes
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`DptValue::decode`] and [`Self::new`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let value = T::decode(bytes)?;
        Self::new(value)
    }
}
