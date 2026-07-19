//! Modern type-safe Data Point Type (DPT) system with const generics and compile-time validation.

use crate::error::{ProtocolError, Result};
use std::collections::HashMap;
use std::marker::PhantomData;

pub mod decode;
pub mod dpt_type;
pub mod payload;
pub mod unit;
pub mod view;

// DPT type modules - organized by DPT number like Python implementation
pub mod dpt1; // Boolean
pub mod dpt10; // Time
pub mod dpt11; // Date
pub mod dpt12; // Unsigned 32-bit
pub mod dpt13; // Signed 32-bit
pub mod dpt14; // 4-byte float
pub mod dpt16; // String
pub mod dpt17; // Scene Number
pub mod dpt18; // Scene Control
pub mod dpt19; // Date and Time
pub mod dpt2; // 2-bit control
pub mod dpt20; // HVAC
pub mod dpt232; // RGB Color
pub mod dpt235; // Tariff Active Energy
pub mod dpt242; // XYY Color
pub mod dpt251;
pub mod dpt29; // Signed 64-bit
pub mod dpt3; // Control
pub mod dpt5; // Unsigned 8-bit
pub mod dpt6; // Signed 8-bit
pub mod dpt7; // Unsigned 16-bit
pub mod dpt8; // Signed 16-bit
pub mod dpt9; // 2-byte float // RGBW Color

#[cfg(test)]
mod tests;

// Re-export all DPT types for convenience
pub use decode::{DecodedTelegram, decode_telegram};
pub use dpt_type::DptType;
pub use dpt1::*;
pub use dpt2::*;
pub use dpt3::*;
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
pub use dpt16::*;
pub use dpt17::*;
pub use dpt18::*;
pub use dpt19::*;
pub use dpt20::*;
pub use dpt29::*;
pub use dpt232::*;
pub use dpt235::*;
pub use dpt242::*;
pub use dpt251::*;
pub use payload::DptPayload;
pub use unit::Unit;
pub use view::{
    BoolView, Control2View, ControlView, DateTimeView, DateView, DptView, EnumView, Float2ByteView,
    Float4ByteView, I8View, I16View, I32View, I64View, RgbView, RgbwView, SceneView, StrView,
    TimeView, U8View, U16View, U32View, XyyView,
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

/// DPT metadata for runtime introspection
#[derive(Debug, Clone, PartialEq)]
pub struct DptMetadata {
    pub dpt_number: &'static str,
    pub value_type: &'static str,
    pub unit: Option<&'static str>,
    pub ha_device_class: Option<&'static str>,
    pub byte_length: usize,
}

/// Type-safe DPT registry
pub struct DptRegistry {
    decoders: HashMap<&'static str, Box<dyn DptDecoder>>,
    value_types: HashMap<&'static str, &'static str>,
}

/// Modern trait for type-safe DPT decoding
pub trait DptDecoder: Send + Sync {
    /// # Errors
    ///
    /// Returns an error if `data` doesn't match the wrapped DPT's expected
    /// length or encoding.
    fn decode_bytes(&self, data: &[u8]) -> Result<Box<dyn std::any::Any + Send + Sync>>;

    /// # Errors
    ///
    /// Returns an error if `value` doesn't downcast to the wrapped DPT's
    /// concrete type, or if that type's `encode` fails.
    fn encode_any(&self, value: &dyn std::any::Any) -> Result<Vec<u8>>;
    fn dpt_number(&self) -> &'static str;
    fn metadata(&self) -> DptMetadata;
    fn clone_decoder(&self) -> Box<dyn DptDecoder>;
}

/// Generic decoder implementation for any `DptValue` type
#[derive(Clone)]
pub struct GenericDptDecoder<T: DptValue> {
    _phantom: PhantomData<T>,
}

impl<T: DptValue> GenericDptDecoder<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<T: DptValue> Default for GenericDptDecoder<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: DptValue> DptDecoder for GenericDptDecoder<T> {
    fn decode_bytes(&self, data: &[u8]) -> Result<Box<dyn std::any::Any + Send + Sync>> {
        let value = T::decode(data)?;
        Ok(Box::new(value))
    }

    fn encode_any(&self, value: &dyn std::any::Any) -> Result<Vec<u8>> {
        let typed_value = value
            .downcast_ref::<T>()
            .ok_or_else(|| ProtocolError::DptError {
                dpt_type: T::DPT_NUMBER.to_string(),
                details: "Type mismatch in encoding".to_string(),
            })?;

        Ok(typed_value.as_bytes().to_vec())
    }

    fn dpt_number(&self) -> &'static str {
        T::DPT_NUMBER
    }

    fn metadata(&self) -> DptMetadata {
        T::metadata()
    }

    fn clone_decoder(&self) -> Box<dyn DptDecoder> {
        Box::new(self.clone())
    }
}

impl DptRegistry {
    /// Create a new type-safe DPT registry
    #[must_use]
    pub fn new() -> Self {
        let mut registry = Self {
            decoders: HashMap::new(),
            value_types: HashMap::new(),
        };

        // Register all modern type-safe DPT implementations
        // DPT 1.xxx - Boolean (register aliases, not base types to avoid conflicts)
        registry.register::<DPTSwitch>();
        registry.register::<DPTBool>();
        registry.register::<DPTEnable>();
        registry.register::<DPTRamp>();
        registry.register::<DPTAlarm>();
        registry.register::<DPTBinaryValue>();
        registry.register::<DPTStep>();
        registry.register::<DPTUpDown>();
        registry.register::<DPTOpenClose>();
        registry.register::<DPTStart>();
        registry.register::<DPTState>();
        registry.register::<DPTInvert>();
        registry.register::<DPTDimSendStyle>();
        registry.register::<DPTInputSource>();
        registry.register::<DPTReset>();
        registry.register::<DPTAck>();
        registry.register::<DPTTrigger>();
        registry.register::<DPTOccupancy>();
        registry.register::<DPTWindowDoor>();
        registry.register::<DPTLogicalFunction>();
        registry.register::<DPTSceneAB>();
        registry.register::<DPTShutterBlindsMode>();
        registry.register::<DPTDayNight>();
        registry.register::<DPTHeatCool>();
        registry.register::<DPTConsumerProducer>();
        registry.register::<DPTEnergyDirection>();

        // DPT 2.xxx - 2-bit control
        registry.register::<DPTSwitchControl>();
        registry.register::<DPTBoolControl>();
        registry.register::<DPTEnableControl>();
        registry.register::<DPTRampControl>();
        registry.register::<DPTAlarmControl>();
        registry.register::<DPTBinaryValueControl>();
        registry.register::<DPTStepControl>();
        registry.register::<DPTDirection1Control>();
        registry.register::<DPTDirection2Control>();
        registry.register::<DPTStartControl>();
        registry.register::<DPTStateControl>();
        registry.register::<DPTInvertControl>();

        // DPT 3.xxx - Control
        registry.register::<ControlDimming>();
        registry.register::<ControlBlinds>();

        // DPT 5.xxx - Unsigned 8-bit
        registry.register::<DPTScaling>();
        registry.register::<DPTAngle>();
        registry.register::<DPTPercentU8>();
        registry.register::<DPTDecimalFactor>();
        registry.register::<DPTTariff>();
        registry.register::<DPTValue1Ucount>();

        // DPT 6.xxx - Signed 8-bit
        registry.register::<DPTPercentV8>();
        registry.register::<DPTValue1Count>();

        // DPT 7.xxx - Unsigned 16-bit
        registry.register::<DPT2Ucount>();
        registry.register::<DPTTimePeriodMsec>();
        registry.register::<DPTTimePeriod10Msec>();
        registry.register::<DPTTimePeriod100Msec>();
        registry.register::<DPTTimePeriodSec>();
        registry.register::<DPTTimePeriodMin>();
        registry.register::<DPTTimePeriodHrs>();
        registry.register::<DPTPropDataType>();
        registry.register::<DPTLengthMm>();
        registry.register::<DPTUElCurrentmA>();
        registry.register::<DPTBrightness>();
        registry.register::<DPTColorTemperature>();

        // DPT 8.xxx - Signed 16-bit
        registry.register::<DPTValue2Count>();
        registry.register::<DPTDeltaTimeMsec>();
        registry.register::<DPTDeltaTime10Msec>();
        registry.register::<DPTDeltaTime100Msec>();
        registry.register::<DPTDeltaTimeSec>();
        registry.register::<DPTDeltaTimeMin>();
        registry.register::<DPTDeltaTimeHrs>();
        registry.register::<DPTPercentV16>();
        registry.register::<DPTRotationAngle>();
        registry.register::<DPTLengthM>();

        // DPT 9.xxx - 2-byte float
        registry.register::<DPTTemperature>();
        registry.register::<DPTTemperatureDifference2Byte>();
        registry.register::<DPTTemperatureA>();
        registry.register::<DPTLux>();
        registry.register::<DPTWsp>();
        registry.register::<DPTPressure2Byte>();
        registry.register::<DPTHumidity>();
        registry.register::<DPTPartsPerMillion>();
        registry.register::<DPTAirFlow>();
        registry.register::<DPTTime1>();
        registry.register::<DPTTime2>();
        registry.register::<DPTVoltage>();
        registry.register::<DPTCurrent>();
        registry.register::<DPTPowerDensity>();
        registry.register::<DPTKelvinPerPercent>();
        registry.register::<DPTPower2Byte>();
        registry.register::<DPTVolumeFlow>();
        registry.register::<DPTRainAmount>();
        registry.register::<DPTTemperatureF>();
        registry.register::<DPTWspKmh>();
        registry.register::<DPTAbsoluteHumidity>();
        registry.register::<DPTConcentrationUGM3>();
        registry.register::<DPTEnthalpy>();

        // DPT 12.xxx - Unsigned 32-bit
        registry.register::<DPTValue4Ucount>();
        registry.register::<DPTLongTimePeriodSec>();
        registry.register::<DPTLongTimePeriodMin>();
        registry.register::<DPTLongTimePeriodHrs>();
        registry.register::<DPTVolumeLiquidLitre>();
        registry.register::<DPTVolumeM3>();

        // DPT 13.xxx - Signed 32-bit
        registry.register::<DPTValue4Count>();
        registry.register::<DPTFlowRateM3H>();
        registry.register::<DPTActiveEnergy>();
        registry.register::<DPTApparantEnergy>();
        registry.register::<DPTReactiveEnergy>();
        registry.register::<DPTActiveEnergykWh>();
        registry.register::<DPTApparantEnergykVAh>();
        registry.register::<DPTReactiveEnergykVARh>();
        registry.register::<DPTActiveEnergyMWh>();
        registry.register::<DPTLongDeltaTimeSec>();
        registry.register::<DPTDeltaVolumeLiquidLitre>();
        registry.register::<DPTDeltaVolumeM3>();

        // DPT 14.xxx - 4-byte float
        registry.register::<DPTAcceleration>();
        registry.register::<DPTAccelerationAngular>();
        registry.register::<DPTActivationEnergy>();
        registry.register::<DPTActivity>();
        registry.register::<DPTMol>();
        registry.register::<DPTAmplitude>();
        registry.register::<DPTAngleRad>();
        registry.register::<DPTAngleDeg>();
        registry.register::<DPTAngularMomentum>();
        registry.register::<DPTAngularVelocity>();
        registry.register::<DPTArea>();
        registry.register::<DPTCapacitance>();
        registry.register::<DPTChargeDensitySurface>();
        registry.register::<DPTChargeDensityVolume>();
        registry.register::<DPTCompressibility>();
        registry.register::<DPTConductance>();
        registry.register::<DPTElectricalConductivity>();
        registry.register::<DPTDensity>();
        registry.register::<DPTElectricCharge>();
        registry.register::<DPTElectricCurrent>();
        registry.register::<DPTElectricCurrentDensity>();
        registry.register::<DPTElectricDipoleMoment>();
        registry.register::<DPTElectricDisplacement>();
        registry.register::<DPTElectricFieldStrength>();
        registry.register::<DPTElectricFlux>();
        registry.register::<DPTElectricFluxDensity>();
        registry.register::<DPTElectricPolarization>();
        registry.register::<DPTElectricPotential>();
        registry.register::<DPTElectricPotentialDifference>();
        registry.register::<DPTElectromagneticMoment>();
        registry.register::<DPTElectromotiveForce>();
        registry.register::<DPTEnergy>();
        registry.register::<DPTForce>();
        registry.register::<DPTFrequency>();
        registry.register::<DPTAngularFrequency>();
        registry.register::<DPTHeatCapacity>();
        registry.register::<DPTHeatFlowRate>();
        registry.register::<DPTHeatQuantity>();
        registry.register::<DPTImpedance>();
        registry.register::<DPTLength>();
        registry.register::<DPTLightQuantity>();
        registry.register::<DPTLuminance>();
        registry.register::<DPTLuminousFlux>();
        registry.register::<DPTLuminousIntensity>();
        registry.register::<DPTMagneticFieldStrength>();
        registry.register::<DPTMagneticFlux>();
        registry.register::<DPTMagneticFluxDensity>();
        registry.register::<DPTMagneticMoment>();
        registry.register::<DPTMagneticPolarization>();
        registry.register::<DPTMagnetization>();
        registry.register::<DPTMagnetomotiveForce>();
        registry.register::<DPTMass>();
        registry.register::<DPTMassFlux>();
        registry.register::<DPTMomentum>();
        registry.register::<DPTPhaseAngleRad>();
        registry.register::<DPTPhaseAngleDeg>();
        registry.register::<DPTPower>();
        registry.register::<DPTPowerFactor>();
        registry.register::<DPTPressure>();
        registry.register::<DPTReactance>();
        registry.register::<DPTResistance>();
        registry.register::<DPTResistivity>();
        registry.register::<DPTSelfInductance>();
        registry.register::<DPTSolidAngle>();
        registry.register::<DPTSoundIntensity>();
        registry.register::<DPTSpeed>();
        registry.register::<DPTStress>();
        registry.register::<DPTSurfaceTension>();
        registry.register::<DPTCommonTemperature>();
        registry.register::<DPTAbsoluteTemperature>();
        registry.register::<DPTTemperatureDifference>();
        registry.register::<DPTThermalCapacity>();
        registry.register::<DPTThermalConductivity>();
        registry.register::<DPTThermoelectricPower>();
        registry.register::<DPTTimeSeconds>();
        registry.register::<DPTTorque>();
        registry.register::<DPTVolume>();
        registry.register::<DPTVolumeFlux>();
        registry.register::<DPTWeight>();
        registry.register::<DPTWork>();
        registry.register::<DPTApparentPower>();
        registry.register::<DPTVolumeFluxMeter>();
        registry.register::<DPTVolumeFluxLs>();

        registry.register::<TimeOfDay>();
        registry.register::<Date>();
        registry.register::<StringAscii>();
        registry.register::<StringLatin1>();
        registry.register::<DPTSceneNumber>();
        registry.register::<SceneControl>();
        registry.register::<DateTime>();
        registry.register::<DPTHVACMode>();
        registry.register::<DPTHVACContrMode>();
        registry.register::<DPTActiveEnergy8Byte>();
        registry.register::<DPTApparantEnergy8Byte>();
        registry.register::<DPTReactiveEnergy8Byte>();
        registry.register::<ColorRGB>();
        registry.register::<TariffActiveEnergy>();
        registry.register::<ColorXYY>();
        registry.register::<ColorRGBW>();

        registry
    }

    /// Register a DPT type
    pub fn register<T: DptValue>(&mut self) {
        let decoder = GenericDptDecoder::<T>::new();
        self.decoders.insert(T::DPT_NUMBER, Box::new(decoder));
        self.value_types.insert(T::VALUE_TYPE, T::DPT_NUMBER);
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `value_type` was never registered.
    pub fn dpt_number_for_value_type(&self, value_type: &str) -> Result<&'static str> {
        self.value_types.get(value_type).copied().ok_or_else(|| {
            ProtocolError::DptError {
                dpt_type: value_type.to_string(),
                details: "Unknown DPT value type".to_string(),
            }
            .into()
        })
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `dpt` was never registered.
    pub fn metadata(&self, dpt: &str) -> Result<DptMetadata> {
        let decoder = self
            .decoders
            .get(dpt)
            .ok_or_else(|| ProtocolError::DptError {
                dpt_type: dpt.to_string(),
                details: "Unknown DPT type".to_string(),
            })?;

        Ok(decoder.metadata())
    }

    /// Decode bytes using the specified DPT number
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `dpt` was never registered, or
    /// if `data` doesn't match that DPT's expected encoding.
    pub fn decode(&self, dpt: &str, data: &[u8]) -> Result<Box<dyn std::any::Any + Send + Sync>> {
        let decoder = self
            .decoders
            .get(dpt)
            .ok_or_else(|| ProtocolError::DptError {
                dpt_type: dpt.to_string(),
                details: "Unknown DPT type".to_string(),
            })?;

        decoder.decode_bytes(data)
    }

    /// Encode value using the specified DPT number
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `dpt` was never registered, or
    /// if `value` doesn't downcast to that DPT's concrete type.
    pub fn encode(&self, dpt: &str, value: &dyn std::any::Any) -> Result<Vec<u8>> {
        let decoder = self
            .decoders
            .get(dpt)
            .ok_or_else(|| ProtocolError::DptError {
                dpt_type: dpt.to_string(),
                details: "Unknown DPT type".to_string(),
            })?;

        decoder.encode_any(value)
    }
}

impl Default for DptRegistry {
    fn default() -> Self {
        Self::new()
    }
}
