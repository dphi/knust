//! Runtime lookup and type-erased dispatch for registered datapoint codecs.

use super::DptValue;
use crate::error::{ProtocolError, Result};
use std::any::Any;
use std::collections::HashMap;

/// DPT metadata for runtime introspection
#[derive(Debug, Clone, PartialEq)]
pub struct DptMetadata {
    pub dpt_number: &'static str,
    pub value_type: &'static str,
    pub unit: Option<&'static str>,
    pub ha_device_class: Option<&'static str>,
    pub byte_length: usize,
}

/// Registry of typed codecs keyed by KNX datapoint number.
pub struct DptRegistry {
    codecs: HashMap<&'static str, Codec>,
    value_types: HashMap<&'static str, &'static str>,
}

type OwnedValue = Box<dyn Any + Send + Sync>;

struct Codec {
    metadata: fn() -> DptMetadata,
    decode: fn(&[u8]) -> Result<OwnedValue>,
    encode: fn(&dyn Any) -> Result<Vec<u8>>,
}

impl DptRegistry {
    /// Create a new type-safe DPT registry
    #[must_use]
    pub fn new() -> Self {
        use super::{
            CharacterAscii, CharacterLatin1, ColorRGB, ColorRGBW, ColorXYY, ControlBlinds,
            ControlDimming, DPT2Ucount, DPTAbsoluteHumidity, DPTAbsoluteTemperature,
            DPTAcceleration, DPTAccelerationAngular, DPTAccessData, DPTAck, DPTActPosDemAbs,
            DPTActivationEnergy, DPTActiveEnergy, DPTActiveEnergy8Byte, DPTActiveEnergyMWh,
            DPTActiveEnergykWh, DPTActivity, DPTAirFlow, DPTAlarm, DPTAlarmControl, DPTAlarmInfo,
            DPTAmplitude, DPTAngle, DPTAngleDeg, DPTAngleRad, DPTAngularFrequency,
            DPTAngularMomentum, DPTAngularVelocity, DPTApparantEnergy, DPTApparantEnergy8Byte,
            DPTApparantEnergykVAh, DPTApparentPower, DPTArea, DPTBatteryInfo, DPTBinaryValue,
            DPTBinaryValueControl, DPTBool, DPTBoolControl, DPTBrightness, DPTBuildingModeNext,
            DPTCapacitance, DPTChangeoverMode, DPTChannelActivation8, DPTChannelActivation16,
            DPTChannelActivation24, DPTChargeDensitySurface, DPTChargeDensityVolume,
            DPTColorTemperature, DPTColorTemperatureControl, DPTColorTemperatureTransition,
            DPTColorXYYTransition, DPTCombinedInfoOnOff, DPTCombinedPosition, DPTCombinedStatusHVA,
            DPTCombinedStatusRTC, DPTCombinedStatusRTSM, DPTCommonTemperature, DPTCompressibility,
            DPTConcentrationUGM3, DPTConductance, DPTConsumerProducer, DPTConverterInfo,
            DPTConverterInfoFix, DPTConverterStatus, DPTConverterTestInfo, DPTConverterTestResult,
            DPTCurrent, DPTDALIControlGearDiagnostic, DPTDALIDiagnostics, DPTDHWModeNext,
            DPTDayNight, DPTDecimalFactor, DPTDeltaTime10Msec, DPTDeltaTime100Msec,
            DPTDeltaTimeHrs, DPTDeltaTimeMin, DPTDeltaTimeMsec, DPTDeltaTimeSec,
            DPTDeltaVolumeLiquidLitre, DPTDeltaVolumeM3, DPTDensity, DPTDeviceControl,
            DPTDimSendStyle, DPTDirection1Control, DPTDirection2Control, DPTDoubleNibble,
            DPTElectricCharge, DPTElectricCurrent, DPTElectricCurrentDensity,
            DPTElectricDipoleMoment, DPTElectricDisplacement, DPTElectricFieldStrength,
            DPTElectricFlux, DPTElectricFluxDensity, DPTElectricPolarization, DPTElectricPotential,
            DPTElectricPotentialDifference, DPTElectricalConductivity, DPTElectromagneticMoment,
            DPTElectromotiveForce, DPTEnable, DPTEnableControl, DPTEnableHCStage, DPTEnergy,
            DPTEnergyDirection, DPTEnthalpy, DPTErrorClassSystem, DPTFanStage, DPTFlowRateM3H,
            DPTForce, DPTForceSign, DPTForceSignCool, DPTForecastAbsoluteHumidity,
            DPTForecastAirPollutants, DPTForecastCO2, DPTForecastRelativeHumidity,
            DPTForecastSunIntensity, DPTForecastTemperature, DPTForecastWindSpeed, DPTFrequency,
            DPTFuelTypeSet, DPTGeographicalLocation, DPTHVACContrMode, DPTHVACMode,
            DPTHVACModeNext, DPTHVACStatus, DPTHeatCapacity, DPTHeatCool, DPTHeatFlowRate,
            DPTHeatQuantity, DPTHumidity, DPTImpedance, DPTInputSource, DPTInvert,
            DPTInvertControl, DPTKelvinPerPercent, DPTLanguageCodeAlpha2ASCII, DPTLength,
            DPTLengthM, DPTLengthMm, DPTLightActuatorErrorInfo, DPTLightQuantity, DPTLockSign,
            DPTLogicalFunction, DPTLongDeltaTimeSec, DPTLongTimePeriodHrs, DPTLongTimePeriodMin,
            DPTLongTimePeriodSec, DPTLuminance, DPTLuminousFlux, DPTLuminousIntensity, DPTLux,
            DPTMBusAddress, DPTMagneticFieldStrength, DPTMagneticFlux, DPTMagneticFluxDensity,
            DPTMagneticMoment, DPTMagneticPolarization, DPTMagnetization, DPTMagnetomotiveForce,
            DPTMass, DPTMassFlux, DPTMedia, DPTMeteringValue, DPTMol, DPTMomentum, DPTOccModeNext,
            DPTOccupancy, DPTOpenClose, DPTPartsPerMillion, DPTPercentU8, DPTPercentV8,
            DPTPercentV16, DPTPhaseAngleDeg, DPTPhaseAngleRad, DPTPhasesStatus, DPTPower,
            DPTPower2Byte, DPTPowerDensity, DPTPowerFactor, DPTPressure, DPTPressure2Byte,
            DPTPrioritisedModeControl, DPTPropDataType, DPTRFFilterInfo, DPTRFModeInfo,
            DPTRainAmount, DPTRamp, DPTRampControl, DPTReactance, DPTReactiveEnergy,
            DPTReactiveEnergy8Byte, DPTReactiveEnergykVARh, DPTRegionCodeAlpha2ASCII,
            DPTRelativeControlRGB, DPTRelativeControlRGBW, DPTRelativeControlXYY, DPTReset,
            DPTResistance, DPTResistivity, DPTRoomTemperatureSetpointShifts3,
            DPTRoomTemperatureSetpointShifts4, DPTRoomTemperatureSetpoints3,
            DPTRoomTemperatureSetpoints4, DPTRotationAngle, DPTScaling, DPTScalingSpeed,
            DPTScalingStepTime, DPTSceneAB, DPTSceneConfig, DPTSceneInfo, DPTSceneNumber,
            DPTSecurityReport, DPTSelfInductance, DPTShutterBlindsMode, DPTSolidAngle,
            DPTSoundIntensity, DPTSpeed, DPTStart, DPTStartControl, DPTState, DPTStateControl,
            DPTStatusAHU, DPTStatusAct, DPTStatusBUC, DPTStatusDHWC, DPTStatusGen,
            DPTStatusLightingActuator, DPTStatusRCC, DPTStatusRHC, DPTStatusRHCC, DPTStatusSAB,
            DPTStatusSDHWC, DPTStep, DPTStepControl, DPTStress, DPTSurfaceTension, DPTSwitch,
            DPTSwitchControl, DPTTariff, DPTTemperature, DPTTemperatureA, DPTTemperatureDifference,
            DPTTemperatureDifference2Byte, DPTTemperatureF, DPTThermalCapacity,
            DPTThermalConductivity, DPTThermoelectricPower, DPTTime1, DPTTime2,
            DPTTimePeriod10Msec, DPTTimePeriod100Msec, DPTTimePeriodHrs, DPTTimePeriodMin,
            DPTTimePeriodMsec, DPTTimePeriodSec, DPTTimeSeconds, DPTTorque, DPTTrigger,
            DPTUElCurrentmA, DPTUpDown, DPTValue1Count, DPTValue1Ucount, DPTValue2Count,
            DPTValue4Count, DPTValue4Ucount, DPTValueDemBOC, DPTVirtualDryContact, DPTVoltage,
            DPTVolume, DPTVolumeFlow, DPTVolumeFlux, DPTVolumeFluxLs, DPTVolumeFluxMeter,
            DPTVolumeLiquidLitre, DPTVolumeM3, DPTWeight, DPTWindowDoor, DPTWork, DPTWsp,
            DPTWspKmh, Date, DateTime, SceneControl, StringAscii, StringLatin1, TariffActiveEnergy,
            TimeOfDay, Version,
        };

        let mut registry = Self {
            codecs: HashMap::new(),
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

        // DPT 4.xxx - Character
        registry.register::<CharacterAscii>();
        registry.register::<CharacterLatin1>();

        // DPT 5.xxx - Unsigned 8-bit
        registry.register::<DPTScaling>();
        registry.register::<DPTAngle>();
        registry.register::<DPTPercentU8>();
        registry.register::<DPTDecimalFactor>();
        registry.register::<DPTTariff>();
        registry.register::<DPTValue1Ucount>();
        registry.register::<DPTFanStage>();

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
        registry.register::<DPTErrorClassSystem>();
        registry.register::<DPTHVACContrMode>();
        registry.register::<DPTChangeoverMode>();
        registry.register::<DPTEnableHCStage>();
        registry.register::<DPTHVACStatus>();
        registry.register::<DPTDoubleNibble>();
        registry.register::<DPTSceneInfo>();
        registry.register::<DPTCombinedInfoOnOff>();
        registry.register::<DPTChannelActivation24>();
        registry.register::<DPTMBusAddress>();
        registry.register::<DPTPrioritisedModeControl>();
        registry.register::<DPTDALIControlGearDiagnostic>();
        registry.register::<DPTGeographicalLocation>();
        registry.register::<DPTCombinedPosition>();
        registry.register::<DPTStatusSAB>();
        registry.register::<DPTAccessData>();
        registry.register::<DPTHVACModeNext>();
        registry.register::<DPTDHWModeNext>();
        registry.register::<DPTOccModeNext>();
        registry.register::<DPTBuildingModeNext>();
        registry.register::<DPTStatusBUC>();
        registry.register::<DPTLockSign>();
        registry.register::<DPTValueDemBOC>();
        registry.register::<DPTActPosDemAbs>();
        registry.register::<DPTStatusAct>();
        registry.register::<DPTStatusLightingActuator>();
        registry.register::<DPTScalingSpeed>();
        registry.register::<DPTScalingStepTime>();
        registry.register::<DPTMeteringValue>();
        registry.register::<DPTAlarmInfo>();
        registry.register::<DPTBatteryInfo>();
        registry.register::<DPTConverterStatus>();
        registry.register::<DPTConverterInfoFix>();
        registry.register::<DPTConverterInfo>();
        registry.register::<DPTConverterTestResult>();
        registry.register::<DPTConverterTestInfo>();
        registry.register::<DPTColorTemperatureTransition>();
        registry.register::<DPTColorXYYTransition>();
        registry.register::<DPTColorTemperatureControl>();
        registry.register::<DPTRelativeControlRGBW>();
        registry.register::<DPTRelativeControlXYY>();
        registry.register::<DPTRelativeControlRGB>();
        registry.register::<DPTLightActuatorErrorInfo>();
        registry.register::<DPTRFFilterInfo>();
        registry.register::<DPTSecurityReport>();
        registry.register::<DPTChannelActivation8>();
        registry.register::<DPTVirtualDryContact>();
        registry.register::<DPTPhasesStatus>();
        registry.register::<DPTRFModeInfo>();
        registry.register::<DPTCombinedStatusRTSM>();
        registry.register::<DPTForceSign>();
        registry.register::<DPTForceSignCool>();
        registry.register::<DPTStatusRHC>();
        registry.register::<DPTStatusSDHWC>();
        registry.register::<DPTFuelTypeSet>();
        registry.register::<DPTStatusRCC>();
        registry.register::<DPTStatusAHU>();
        registry.register::<DPTStatusGen>();
        registry.register::<DPTDeviceControl>();
        registry.register::<DPTStatusDHWC>();
        registry.register::<DPTMedia>();
        registry.register::<DPTChannelActivation16>();
        registry.register::<DPTStatusRHCC>();
        registry.register::<DPTCombinedStatusHVA>();
        registry.register::<DPTCombinedStatusRTC>();
        registry.register::<DPTForecastTemperature>();
        registry.register::<DPTForecastWindSpeed>();
        registry.register::<DPTForecastRelativeHumidity>();
        registry.register::<DPTForecastAbsoluteHumidity>();
        registry.register::<DPTForecastCO2>();
        registry.register::<DPTForecastAirPollutants>();
        registry.register::<DPTForecastSunIntensity>();
        registry.register::<DPTRoomTemperatureSetpoints3>();
        registry.register::<DPTRoomTemperatureSetpointShifts3>();
        registry.register::<DPTRoomTemperatureSetpoints4>();
        registry.register::<DPTRoomTemperatureSetpointShifts4>();
        registry.register::<DPTSceneConfig>();
        registry.register::<DPTDALIDiagnostics>();
        registry.register::<DPTLanguageCodeAlpha2ASCII>();
        registry.register::<DPTRegionCodeAlpha2ASCII>();
        registry.register::<DPTActiveEnergy8Byte>();
        registry.register::<DPTApparantEnergy8Byte>();
        registry.register::<DPTReactiveEnergy8Byte>();
        registry.register::<Version>();
        registry.register::<ColorRGB>();
        registry.register::<TariffActiveEnergy>();
        registry.register::<ColorXYY>();
        registry.register::<ColorRGBW>();

        registry
    }

    /// Register a DPT type
    pub fn register<T: DptValue>(&mut self) {
        let codec = Codec {
            metadata: T::metadata,
            decode: |bytes| Ok(Box::new(T::decode(bytes)?)),
            encode: |value| {
                let value = value
                    .downcast_ref::<T>()
                    .ok_or_else(|| ProtocolError::DptError {
                        dpt_type: T::DPT_NUMBER.to_string(),
                        details: "Type mismatch in encoding".to_string(),
                    })?;
                value.encode()
            },
        };
        self.codecs.insert(T::DPT_NUMBER, codec);
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
        Ok((self.codec(dpt)?.metadata)())
    }

    /// Decode bytes using the specified DPT number
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `dpt` was never registered, or
    /// if `data` doesn't match that DPT's expected encoding.
    pub fn decode(&self, dpt: &str, data: &[u8]) -> Result<Box<dyn Any + Send + Sync>> {
        (self.codec(dpt)?.decode)(data)
    }

    /// Encode value using the specified DPT number
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `dpt` was never registered, or
    /// if `value` doesn't downcast to that DPT's concrete type.
    pub fn encode(&self, dpt: &str, value: &dyn Any) -> Result<Vec<u8>> {
        (self.codec(dpt)?.encode)(value)
    }

    fn codec(&self, dpt: &str) -> Result<&Codec> {
        self.codecs.get(dpt).ok_or_else(|| {
            ProtocolError::DptError {
                dpt_type: dpt.to_string(),
                details: "Unknown DPT type".to_string(),
            }
            .into()
        })
    }
}

impl Default for DptRegistry {
    fn default() -> Self {
        Self::new()
    }
}
