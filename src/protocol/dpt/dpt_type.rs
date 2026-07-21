use super::payload::DptPayload;
use super::unit::Unit;
use super::view::{
    BoolView, Control2View, ControlView, DateTimeView, DateView, DptView, EnumView, Float2ByteView,
    Float4ByteView, I8View, I16View, I32View, I64View, RgbView, RgbwView, SceneView, StrView,
    TimeView, U8View, U16View, U32View, XyyView,
};
use crate::error::{ProtocolError, Result};
use crate::log_protocol;
use crate::logging::LogLevel;

/// Runtime DPT type identity. Discriminant = main * `100_000` + sub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::FromRepr)]
#[repr(u32)]
pub enum DptType {
    // DPT 1.xxx - Boolean
    Switch = 100_001,
    Bool = 100_002,
    Enable = 100_003,
    Ramp = 100_004,
    Alarm = 100_005,
    BinaryValue = 100_006,
    Step = 100_007,
    UpDown = 100_008,
    OpenClose = 100_009,
    Start = 100_010,
    State = 100_011,
    Invert = 100_012,
    DimSendStyle = 100_013,
    InputSource = 100_014,
    Reset = 100_015,
    Ack = 100_016,
    Trigger = 100_017,
    Occupancy = 100_018,
    WindowDoor = 100_019,
    LogicalFunction = 100_021,
    SceneAB = 100_022,
    ShutterBlindsMode = 100_023,
    DayNight = 100_024,
    HeatCool = 100_100,
    ConsumerProducer = 101_200,
    EnergyDirection = 101_201,

    // DPT 2.xxx - 2-bit control (control bit + value bit)
    SwitchControl = 200_001,
    BoolControl = 200_002,
    EnableControl = 200_003,
    RampControl = 200_004,
    AlarmControl = 200_005,
    BinaryValueControl = 200_006,
    StepControl = 200_007,
    Direction1Control = 200_008,
    Direction2Control = 200_009,
    StartControl = 200_010,
    StateControl = 200_011,
    InvertControl = 200_012,

    // DPT 3.xxx - Control
    ControlDimming = 300_007,
    ControlBlinds = 300_008,

    // DPT 5.xxx - Unsigned 8-bit
    Scaling = 500_001,
    Angle = 500_003,
    PercentU8 = 500_004,
    DecimalFactor = 500_005,
    Tariff = 500_006,
    Value1Ucount = 500_010,

    // DPT 6.xxx - Signed 8-bit
    PercentV8 = 600_001,
    Value1Count = 600_010,

    // DPT 7.xxx - Unsigned 16-bit
    Value2Ucount = 700_001,
    TimePeriodMsec = 700_002,
    TimePeriod10Msec = 700_003,
    TimePeriod100Msec = 700_004,
    TimePeriodSec = 700_005,
    TimePeriodMin = 700_006,
    TimePeriodHrs = 700_007,
    PropDataType = 700_010,
    LengthMm = 700_011,
    UElCurrentmA = 700_012,
    Brightness = 700_013,
    ColorTemperature = 700_600,

    // DPT 8.xxx - Signed 16-bit
    Value2Count = 800_001,
    DeltaTimeMsec = 800_002,
    DeltaTime10Msec = 800_003,
    DeltaTime100Msec = 800_004,
    DeltaTimeSec = 800_005,
    DeltaTimeMin = 800_006,
    DeltaTimeHrs = 800_007,
    PercentV16 = 800_010,
    RotationAngle = 800_011,
    LengthM = 800_012,

    // DPT 9.xxx - 2-byte float
    Temperature = 900_001,
    TemperatureDifference2Byte = 900_002,
    TemperatureA = 900_003,
    Lux = 900_004,
    Wsp = 900_005,
    Pressure2Byte = 900_006,
    Humidity = 900_007,
    PartsPerMillion = 900_008,
    AirFlow = 900_009,
    Time1 = 900_010,
    Time2 = 900_011,
    Voltage = 900_020,
    Current = 900_021,
    PowerDensity = 900_022,
    KelvinPerPercent = 900_023,
    Power2Byte = 900_024,
    VolumeFlow = 900_025,
    RainAmount = 900_026,
    TemperatureF = 900_027,
    WspKmh = 900_028,
    AbsoluteHumidity = 900_029,
    ConcentrationUGM3 = 900_030,
    Enthalpy = 960_000,

    // DPT 10.xxx - Time
    TimeOfDay = 1_000_001,

    // DPT 11.xxx - Date
    Date = 1_100_001,

    // DPT 12.xxx - Unsigned 32-bit
    Value4Ucount = 1_200_001,
    LongTimePeriodSec = 1_200_100,
    LongTimePeriodMin = 1_200_101,
    LongTimePeriodHrs = 1_200_102,
    VolumeLiquidLitre = 1_201_200,
    VolumeM3 = 1_201_201,

    // DPT 13.xxx - Signed 32-bit
    Value4Count = 1_300_001,
    FlowRateM3H = 1_300_002,
    ActiveEnergy = 1_300_010,
    ApparantEnergy = 1_300_011,
    ReactiveEnergy = 1_300_012,
    ActiveEnergykWh = 1_300_013,
    ApparantEnergykVAh = 1_300_014,
    ReactiveEnergykVARh = 1_300_015,
    ActiveEnergyMWh = 1_300_016,
    LongDeltaTimeSec = 1_300_100,
    DeltaVolumeLiquidLitre = 1_301_200,
    DeltaVolumeM3 = 1_301_201,

    // DPT 14.xxx - 4-byte float
    Acceleration = 1_400_000,
    AccelerationAngular = 1_400_001,
    ActivationEnergy = 1_400_002,
    Activity = 1_400_003,
    Mol = 1_400_004,
    Amplitude = 1_400_005,
    AngleRad = 1_400_006,
    AngleDeg = 1_400_007,
    AngularMomentum = 1_400_008,
    AngularVelocity = 1_400_009,
    Area = 1_400_010,
    Capacitance = 1_400_011,
    ChargeDensitySurface = 1_400_012,
    ChargeDensityVolume = 1_400_013,
    Compressibility = 1_400_014,
    Conductance = 1_400_015,
    ElectricalConductivity = 1_400_016,
    Density = 1_400_017,
    ElectricCharge = 1_400_018,
    ElectricCurrent = 1_400_019,
    ElectricCurrentDensity = 1_400_020,
    ElectricDipoleMoment = 1_400_021,
    ElectricDisplacement = 1_400_022,
    ElectricFieldStrength = 1_400_023,
    ElectricFlux = 1_400_024,
    ElectricFluxDensity = 1_400_025,
    ElectricPolarization = 1_400_026,
    ElectricPotential = 1_400_027,
    ElectricPotentialDifference = 1_400_028,
    ElectromagneticMoment = 1_400_029,
    ElectromotiveForce = 1_400_030,
    Energy = 1_400_031,
    Force = 1_400_032,
    Frequency = 1_400_033,
    AngularFrequency = 1_400_034,
    HeatCapacity = 1_400_035,
    HeatFlowRate = 1_400_036,
    HeatQuantity = 1_400_037,
    Impedance = 1_400_038,
    Length = 1_400_039,
    LightQuantity = 1_400_040,
    Luminance = 1_400_041,
    LuminousFlux = 1_400_042,
    LuminousIntensity = 1_400_043,
    MagneticFieldStrength = 1_400_044,
    MagneticFlux = 1_400_045,
    MagneticFluxDensity = 1_400_046,
    MagneticMoment = 1_400_047,
    MagneticPolarization = 1_400_048,
    Magnetization = 1_400_049,
    MagnetomotiveForce = 1_400_050,
    Mass = 1_400_051,
    MassFlux = 1_400_052,
    Momentum = 1_400_053,
    PhaseAngleRad = 1_400_054,
    PhaseAngleDeg = 1_400_055,
    Power = 1_400_056,
    PowerFactor = 1_400_057,
    Pressure = 1_400_058,
    Reactance = 1_400_059,
    Resistance = 1_400_060,
    Resistivity = 1_400_061,
    SelfInductance = 1_400_062,
    SolidAngle = 1_400_063,
    SoundIntensity = 1_400_064,
    Speed = 1_400_065,
    Stress = 1_400_066,
    SurfaceTension = 1_400_067,
    CommonTemperature = 1_400_068,
    AbsoluteTemperature = 1_400_069,
    TemperatureDifference = 1_400_070,
    ThermalCapacity = 1_400_071,
    ThermalConductivity = 1_400_072,
    ThermoelectricPower = 1_400_073,
    TimeSeconds = 1_400_074,
    Torque = 1_400_075,
    Volume = 1_400_076,
    VolumeFlux = 1_400_077,
    Weight = 1_400_078,
    Work = 1_400_079,
    ApparentPower = 1_400_080,
    VolumeFluxMeter = 1_401_200,
    VolumeFluxLs = 1_401_201,

    // DPT 16.xxx - String
    StringAscii = 1_600_000,
    StringLatin1 = 1_600_001,

    // DPT 17.xxx - Scene Number
    SceneNumber = 1_700_001,

    // DPT 18.xxx - Scene Control
    SceneControl = 1_800_001,

    // DPT 19.xxx - Date and Time
    DateTime = 1_900_001,

    // DPT 20.xxx - HVAC
    HVACMode = 2_000_102,
    HVACContrMode = 2_000_105,

    // DPT 29.xxx - Signed 64-bit
    ActiveEnergy8Byte = 2_900_010,
    ApparantEnergy8Byte = 2_900_011,
    ReactiveEnergy8Byte = 2_900_012,

    // DPT 232.xxx - RGB Color
    ColorRGB = 23_200_600,

    // DPT 235.xxx - Tariff Active Energy
    TariffActiveEnergy = 23_500_001,

    // DPT 242.xxx - XYY Color
    ColorXYY = 24_200_600,

    // DPT 251.xxx - RGBW Color
    ColorRGBW = 25_100_600,
}

impl DptType {
    /// Get the main DPT number (e.g., 9 for DPT 9.001).
    #[must_use]
    pub const fn main(&self) -> u16 {
        (*self as u32 / 100_000) as u16
    }

    /// Get the sub DPT number (e.g., 1 for DPT 9.001).
    #[must_use]
    pub const fn sub(&self) -> u16 {
        (*self as u32 % 100_000) as u16
    }

    /// Format as "main.sub" string (e.g., "9.001").
    #[must_use]
    pub fn number_str(&self) -> String {
        format!("{}.{:03}", self.main(), self.sub())
    }

    /// Look up a `DptType` by main and sub numbers.
    #[must_use]
    pub fn from_number(main: u16, sub: u16) -> Option<Self> {
        Self::from_repr(u32::from(main) * 100_000 + u32::from(sub))
    }

    /// Parse a DPT number string like "9.001" or "9.1".
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        let (main_s, sub_s) = s.split_once('.')?;
        let main: u16 = main_s.parse().ok()?;
        let sub: u16 = sub_s.parse().ok()?;
        Self::from_number(main, sub)
    }

    /// Get the unit of measurement for this DPT, if any.
    // Spec mapping table: arms are grouped by DPT family, so identical bodies
    // across families stay separate for readability.
    #[allow(clippy::match_same_arms)]
    #[must_use]
    pub fn unit(&self) -> Option<Unit> {
        match self {
            // DPT 5
            Self::Scaling | Self::PercentU8 => Some(Unit::Percent),
            Self::Angle | Self::AngleDeg | Self::PhaseAngleDeg | Self::RotationAngle => {
                Some(Unit::Degree)
            }

            // DPT 7
            Self::TimePeriodMsec | Self::TimePeriod10Msec | Self::TimePeriod100Msec => {
                Some(Unit::Millisecond)
            }
            Self::TimePeriodSec => Some(Unit::Second),
            Self::TimePeriodMin => Some(Unit::Minute),
            Self::TimePeriodHrs => Some(Unit::Hour),
            Self::LengthMm => Some(Unit::Millimeter),
            Self::UElCurrentmA => Some(Unit::Milliampere),
            Self::Brightness => Some(Unit::Lux),
            Self::ColorTemperature => Some(Unit::Kelvin),

            // DPT 8
            Self::DeltaTimeMsec | Self::DeltaTime10Msec | Self::DeltaTime100Msec => {
                Some(Unit::Millisecond)
            }
            Self::DeltaTimeSec => Some(Unit::Second),
            Self::DeltaTimeMin => Some(Unit::Minute),
            Self::DeltaTimeHrs => Some(Unit::Hour),
            Self::PercentV16 | Self::PercentV8 => Some(Unit::Percent),
            Self::LengthM => Some(Unit::Meter),

            // DPT 9
            Self::Temperature | Self::CommonTemperature => Some(Unit::DegreeCelsius),
            Self::TemperatureDifference2Byte
            | Self::AbsoluteTemperature
            | Self::TemperatureDifference => Some(Unit::Kelvin),
            Self::TemperatureA => Some(Unit::KelvinPerHour),
            Self::Lux => Some(Unit::Lux),
            Self::Wsp | Self::Speed => Some(Unit::MeterPerSecond),
            Self::Pressure2Byte | Self::Pressure | Self::Stress => Some(Unit::Pascal),
            Self::Humidity => Some(Unit::Percent),
            Self::PartsPerMillion => Some(Unit::PartsPerMillion),
            Self::AirFlow | Self::VolumeFluxMeter => Some(Unit::CubicMeterPerHour),
            Self::Time1 | Self::TimeSeconds | Self::LongDeltaTimeSec | Self::LongTimePeriodSec => {
                Some(Unit::Second)
            }
            Self::Time2 => Some(Unit::Millisecond),
            Self::Voltage
            | Self::ElectricPotential
            | Self::ElectricPotentialDifference
            | Self::ElectromotiveForce => Some(Unit::Volt),
            Self::Current => Some(Unit::Milliampere),
            Self::PowerDensity | Self::SoundIntensity => Some(Unit::WattPerSquareMeter),
            Self::KelvinPerPercent => Some(Unit::KelvinPerPercent),
            Self::Power2Byte => Some(Unit::Kilowatt),
            Self::VolumeFlow => Some(Unit::LiterPerHour),
            Self::RainAmount => Some(Unit::LiterPerSquareMeter),
            Self::TemperatureF => Some(Unit::DegreeFahrenheit),
            Self::WspKmh => Some(Unit::KilometerPerHour),
            Self::AbsoluteHumidity => Some(Unit::GramPerCubicMeter),
            Self::ConcentrationUGM3 => Some(Unit::MicrogramPerCubicMeter),

            // DPT 12
            Self::LongTimePeriodMin => Some(Unit::Minute),
            Self::LongTimePeriodHrs => Some(Unit::Hour),
            Self::VolumeLiquidLitre | Self::DeltaVolumeLiquidLitre => Some(Unit::Liter),
            Self::VolumeM3 | Self::DeltaVolumeM3 | Self::Volume => Some(Unit::CubicMeter),

            // DPT 13
            Self::FlowRateM3H => Some(Unit::CubicMeterPerHour),
            Self::ActiveEnergy | Self::ActiveEnergy8Byte => Some(Unit::WattHour),
            Self::ApparantEnergy | Self::ApparantEnergy8Byte => Some(Unit::VoltAmpereHour),
            Self::ReactiveEnergy | Self::ReactiveEnergy8Byte => Some(Unit::VoltAmpereReactiveHour),
            Self::ActiveEnergykWh => Some(Unit::KilowattHour),
            Self::ApparantEnergykVAh => Some(Unit::KilovoltAmpereHour),
            Self::ReactiveEnergykVARh => Some(Unit::KilovoltAmpereReactiveHour),
            Self::ActiveEnergyMWh => Some(Unit::MegawattHour),

            // DPT 14
            Self::Acceleration => Some(Unit::MeterPerSecondSquared),
            Self::AccelerationAngular => Some(Unit::RadianPerSecondSquared),
            Self::ActivationEnergy => Some(Unit::JoulePerMol),
            Self::Activity => Some(Unit::InverseSecond),
            Self::Mol => Some(Unit::Mol),
            Self::AngleRad | Self::PhaseAngleRad => Some(Unit::Radian),
            Self::AngularMomentum => Some(Unit::JouleSecond),
            Self::AngularVelocity | Self::AngularFrequency => Some(Unit::RadianPerSecond),
            Self::Area => Some(Unit::SquareMeter),
            Self::Capacitance => Some(Unit::Farad),
            Self::ChargeDensitySurface
            | Self::ElectricFluxDensity
            | Self::ElectricPolarization
            | Self::ElectricDisplacement => Some(Unit::CoulombPerSquareMeter),
            Self::ChargeDensityVolume => Some(Unit::CoulombPerCubicMeter),
            Self::Compressibility => Some(Unit::SquareMeterPerNewton),
            Self::Conductance => Some(Unit::Siemens),
            Self::ElectricalConductivity => Some(Unit::SiemensPerMeter),
            Self::Density => Some(Unit::KilogramPerCubicMeter),
            Self::ElectricCharge | Self::ElectricFlux => Some(Unit::Coulomb),
            Self::ElectricCurrent | Self::MagnetomotiveForce => Some(Unit::Ampere),
            Self::ElectricCurrentDensity => Some(Unit::AmperePerSquareMeter),
            Self::ElectricDipoleMoment => Some(Unit::CoulombMeter),
            Self::ElectricFieldStrength => Some(Unit::VoltPerMeter),
            Self::ElectromagneticMoment | Self::MagneticMoment => Some(Unit::AmpereSquareMeter),
            Self::Energy | Self::HeatQuantity | Self::Work => Some(Unit::Joule),
            Self::Force | Self::Weight => Some(Unit::Newton),
            Self::Frequency => Some(Unit::Hertz),
            Self::HeatCapacity | Self::ThermalCapacity => Some(Unit::JoulePerKelvin),
            Self::HeatFlowRate | Self::Power | Self::ApparentPower => Some(Unit::Watt),
            Self::Impedance | Self::Reactance | Self::Resistance => Some(Unit::Ohm),
            Self::Length => Some(Unit::Meter),
            Self::LightQuantity => Some(Unit::LumenSecond),
            Self::Luminance => Some(Unit::CandelaPerSquareMeter),
            Self::LuminousFlux => Some(Unit::Lumen),
            Self::LuminousIntensity => Some(Unit::Candela),
            Self::MagneticFieldStrength | Self::Magnetization => Some(Unit::AmperePerMeter),
            Self::MagneticFlux => Some(Unit::Weber),
            Self::MagneticFluxDensity | Self::MagneticPolarization => Some(Unit::Tesla),
            Self::Mass => Some(Unit::Kilogram),
            Self::MassFlux => Some(Unit::KilogramPerSecond),
            Self::Momentum => Some(Unit::NewtonPerSecond),
            Self::PowerFactor => None,
            Self::Resistivity => Some(Unit::OhmMeter),
            Self::SelfInductance => Some(Unit::Henry),
            Self::SolidAngle => Some(Unit::Steradian),
            Self::SurfaceTension => Some(Unit::NewtonPerMeter),
            Self::ThermalConductivity => Some(Unit::WattPerMeterKelvin),
            Self::ThermoelectricPower => Some(Unit::VoltPerKelvin),
            Self::Torque => Some(Unit::NewtonMeter),
            Self::VolumeFlux => Some(Unit::CubicMeterPerSecond),
            Self::VolumeFluxLs => Some(Unit::LiterPerSecond),

            // DPT 235
            Self::TariffActiveEnergy => Some(Unit::WattHour),

            _ => None,
        }
    }

    /// Get the byte length for the encoded payload.
    #[must_use]
    pub fn byte_length(&self) -> usize {
        match self.main() {
            1 | 2 | 3 | 5 | 6 | 17 | 18 | 20 => 1,
            7..=9 => 2,
            10 | 11 | 232 => 3,
            12..=14 => 4,
            16 => 14,
            19 | 29 => 8,
            235 | 242 | 251 => 6,
            _ => 0,
        }
    }
}

impl DptType {
    /// Zero-copy decode: validates byte length and wraps as the appropriate view.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `bytes.len()` doesn't match this
    /// DPT's expected byte length.
    pub fn decode_ref<'a>(&self, bytes: &'a [u8]) -> Result<DptView<'a>> {
        let expected_len = self.byte_length();
        if expected_len > 0 && bytes.len() != expected_len {
            log_protocol!(
                LogLevel::Warn,
                "DPT {} decode failed: expected {} bytes, got {}",
                self.number_str(),
                expected_len,
                bytes.len()
            );
            return Err(ProtocolError::DptError {
                dpt_type: self.number_str(),
                details: format!("expected {} bytes, got {}", expected_len, bytes.len()),
            }
            .into());
        }

        match self.main() {
            1 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: bit={} raw=[{:02X}]",
                    self.number_str(),
                    bytes[0] & 0x01,
                    bytes[0]
                );
                Ok(DptView::Bool(BoolView(bytes)))
            }
            2 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: control={} value={} raw=[{:02X}]",
                    self.number_str(),
                    (bytes[0] & 0x02) != 0,
                    bytes[0] & 0x01,
                    bytes[0]
                );
                Ok(DptView::Control2(Control2View(bytes)))
            }
            3 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: step={} step_code={} raw=[{:02X}]",
                    self.number_str(),
                    (bytes[0] & 0x08) != 0,
                    bytes[0] & 0x07,
                    bytes[0]
                );
                Ok(DptView::Control(ControlView(bytes)))
            }
            5 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: value={} raw=[{:02X}]",
                    self.number_str(),
                    bytes[0],
                    bytes[0]
                );
                Ok(DptView::U8(U8View(bytes)))
            }
            6 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: value={} raw=[{:02X}]",
                    self.number_str(),
                    bytes[0] as i8,
                    bytes[0]
                );
                Ok(DptView::I8(I8View(bytes)))
            }
            7 => {
                let val = u16::from_be_bytes([bytes[0], bytes[1]]);
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: value={} raw=[{:02X},{:02X}]",
                    self.number_str(),
                    val,
                    bytes[0],
                    bytes[1]
                );
                Ok(DptView::U16(U16View(bytes)))
            }
            8 => {
                let val = i16::from_be_bytes([bytes[0], bytes[1]]);
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: value={} raw=[{:02X},{:02X}]",
                    self.number_str(),
                    val,
                    bytes[0],
                    bytes[1]
                );
                Ok(DptView::I16(I16View(bytes)))
            }
            9 => {
                let raw = (u16::from(bytes[0]) << 8) | u16::from(bytes[1]);
                let m = raw & 0x07FF;
                let sign = raw & 0x8000 != 0;
                let e = (raw >> 11) & 0x0F;
                let view = Float2ByteView(bytes);
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: mantissa={}, exp={}, sign={}, value={:.2} raw=[{:02X},{:02X}]",
                    self.number_str(),
                    m,
                    e,
                    if sign { "-" } else { "+" },
                    view.as_f64(),
                    bytes[0],
                    bytes[1]
                );
                Ok(DptView::Float2Byte(view))
            }
            10 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: day={} hour={} min={} sec={} raw=[{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    (bytes[0] >> 5) & 0x07,
                    bytes[0] & 0x1F,
                    bytes[1] & 0x3F,
                    bytes[2] & 0x3F,
                    bytes[0],
                    bytes[1],
                    bytes[2]
                );
                Ok(DptView::Time(TimeView(bytes)))
            }
            11 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: day={} month={} year={} raw=[{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    bytes[0] & 0x1F,
                    bytes[1] & 0x0F,
                    bytes[2] & 0x7F,
                    bytes[0],
                    bytes[1],
                    bytes[2]
                );
                Ok(DptView::Date(DateView(bytes)))
            }
            12 => {
                let val = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: value={} raw=[{:02X},{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    val,
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[3]
                );
                Ok(DptView::U32(U32View(bytes)))
            }
            13 => {
                let val = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: value={} raw=[{:02X},{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    val,
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[3]
                );
                Ok(DptView::I32(I32View(bytes)))
            }
            14 => {
                let view = Float4ByteView(bytes);
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: ieee754={:.6} raw=[{:02X},{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    view.as_f64(),
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[3]
                );
                Ok(DptView::Float4Byte(view))
            }
            16 => {
                let view = StrView(bytes);
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: str={:?} len={}",
                    self.number_str(),
                    view.as_str(),
                    bytes.len()
                );
                Ok(DptView::Str(view))
            }
            17 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: scene={} raw=[{:02X}]",
                    self.number_str(),
                    (bytes[0] & 0x3F) + 1,
                    bytes[0]
                );
                Ok(DptView::Scene(SceneView(bytes)))
            }
            18 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: scene={} learn={} raw=[{:02X}]",
                    self.number_str(),
                    (bytes[0] & 0x3F) + 1,
                    bytes[0] & 0x80 != 0,
                    bytes[0]
                );
                Ok(DptView::Scene(SceneView(bytes)))
            }
            19 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: datetime raw=[{:02X},{:02X},{:02X},{:02X},{:02X},{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[3],
                    bytes[4],
                    bytes[5],
                    bytes[6],
                    bytes[7]
                );
                Ok(DptView::DateTime(DateTimeView(bytes)))
            }
            20 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: enum_value={} raw=[{:02X}]",
                    self.number_str(),
                    bytes[0],
                    bytes[0]
                );
                Ok(DptView::Enum(EnumView(bytes)))
            }
            29 => {
                let val = i64::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]);
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: value={} raw=[{:02X},{:02X},{:02X},{:02X},{:02X},{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    val,
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[3],
                    bytes[4],
                    bytes[5],
                    bytes[6],
                    bytes[7]
                );
                Ok(DptView::I64(I64View(bytes)))
            }
            232 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: r={} g={} b={} raw=[{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[0],
                    bytes[1],
                    bytes[2]
                );
                Ok(DptView::Rgb(RgbView(bytes)))
            }
            235 => {
                if bytes.len() >= 8 {
                    let val = i64::from_be_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ]);
                    log_protocol!(
                        LogLevel::Trace,
                        "DPT {} decode: value={} raw_len={}",
                        self.number_str(),
                        val,
                        bytes.len()
                    );
                    Ok(DptView::I64(I64View(&bytes[..8])))
                } else {
                    log_protocol!(
                        LogLevel::Warn,
                        "DPT {} decode failed: insufficient bytes for DPT 235 (got {})",
                        self.number_str(),
                        bytes.len()
                    );
                    Err(ProtocolError::DptError {
                        dpt_type: self.number_str(),
                        details: "insufficient bytes for DPT 235".to_string(),
                    }
                    .into())
                }
            }
            242 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: xyy raw=[{:02X},{:02X},{:02X},{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[3],
                    bytes[4],
                    bytes[5]
                );
                Ok(DptView::Xyy(XyyView(bytes)))
            }
            251 => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} decode: rgbw raw=[{:02X},{:02X},{:02X},{:02X},{:02X},{:02X}]",
                    self.number_str(),
                    bytes[0],
                    bytes[1],
                    bytes[2],
                    bytes[3],
                    bytes[4],
                    bytes[5]
                );
                Ok(DptView::Rgbw(RgbwView(bytes)))
            }
            _ => {
                log_protocol!(
                    LogLevel::Warn,
                    "DPT {} decode failed: unsupported main number {}",
                    self.number_str(),
                    self.main()
                );
                Err(ProtocolError::DptError {
                    dpt_type: self.number_str(),
                    details: "unsupported DPT main number".to_string(),
                }
                .into())
            }
        }
    }
}

impl DptType {
    /// Encode a `DptPayload` into raw bytes for this DPT type.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `payload`'s variant doesn't
    /// match what this DPT number encodes.
    pub fn encode(&self, payload: &DptPayload) -> Result<Vec<u8>> {
        let result: Result<Vec<u8>> = match (self.main(), payload) {
            (1, DptPayload::Bool(v)) => Ok(vec![u8::from(*v)]),
            (2, DptPayload::BinaryControl { control, value }) => {
                Ok(vec![(if *control { 0x02 } else { 0 }) | u8::from(*value)])
            }
            (3, DptPayload::Control { step, step_code }) => {
                Ok(vec![(if *step { 0x08 } else { 0 }) | (*step_code & 0x07)])
            }
            (5, DptPayload::UnsignedInt(v)) => Ok(vec![*v as u8]),
            (6, DptPayload::SignedInt(v)) => Ok(vec![*v as u8]),
            (7, DptPayload::UnsignedInt(v)) => Ok((*v as u16).to_be_bytes().to_vec()),
            (8, DptPayload::SignedInt(v)) => Ok((*v as i16).to_be_bytes().to_vec()),
            (9, DptPayload::Float(v)) => {
                // KNX 2-byte float encoding
                let v = *v;
                let mut e: i32 = 0;
                let mut m = (v * 100.0) as i32;
                while m.abs() > 2047 {
                    m >>= 1;
                    e += 1;
                }
                let sign = if m < 0 { 0x8000u16 } else { 0 };
                let m = if m < 0 {
                    ((m + 2048) & 0x07FF) as u16 | 0x8000
                } else {
                    (m & 0x07FF) as u16
                };
                let raw = sign | ((e as u16 & 0x0F) << 11) | (m & 0x07FF);
                Ok(raw.to_be_bytes().to_vec())
            }
            (
                10,
                DptPayload::Time {
                    day,
                    hour,
                    minute,
                    second,
                },
            ) => Ok(vec![
                (day << 5) | (hour & 0x1F),
                *minute & 0x3F,
                *second & 0x3F,
            ]),
            (11, DptPayload::Date { day, month, year }) => {
                if (1990..=2089).contains(year) {
                    let y = if *year >= 2000 {
                        (*year - 2000) as u8
                    } else {
                        (*year - 1900) as u8
                    };
                    Ok(vec![*day & 0x1F, *month & 0x0F, y & 0x7F])
                } else {
                    Err(ProtocolError::DptError {
                        dpt_type: self.number_str(),
                        details: format!("year {year} out of range 1990-2089 for DPT 11"),
                    }
                    .into())
                }
            }
            (
                19,
                DptPayload::DateTime {
                    year,
                    month,
                    day,
                    day_of_week,
                    hour,
                    minute,
                    second,
                },
            ) => {
                if (1900..=2155).contains(year) {
                    Ok(vec![
                        (*year - 1900) as u8,
                        *month & 0x0F,
                        *day & 0x1F,
                        ((*day_of_week & 0x07) << 5) | (*hour & 0x1F),
                        *minute & 0x3F,
                        *second & 0x3F,
                        0x00,
                        0x00,
                    ])
                } else {
                    Err(ProtocolError::DptError {
                        dpt_type: self.number_str(),
                        details: format!("year {year} out of range 1900-2155 for DPT 19"),
                    }
                    .into())
                }
            }
            (12, DptPayload::UnsignedInt(v)) => Ok((*v as u32).to_be_bytes().to_vec()),
            (13, DptPayload::SignedInt(v)) => Ok((*v as i32).to_be_bytes().to_vec()),
            (14, DptPayload::Float(v)) => Ok((*v as f32).to_be_bytes().to_vec()),
            (16, DptPayload::String(s)) => {
                let mut bytes = vec![0u8; 14];
                for (i, b) in s.bytes().take(14).enumerate() {
                    bytes[i] = b;
                }
                Ok(bytes)
            }
            (17, DptPayload::Scene(n)) => Ok(vec![(*n).saturating_sub(1) & 0x3F]),
            (18, DptPayload::SceneControl { scene, learn }) => Ok(vec![
                (if *learn { 0x80 } else { 0 }) | (scene.saturating_sub(1) & 0x3F),
            ]),
            (20, DptPayload::Enum(v)) => Ok(vec![*v]),
            (29, DptPayload::SignedInt(v)) => Ok(v.to_be_bytes().to_vec()),
            (232, DptPayload::ColorRGB { r, g, b }) => Ok(vec![*r, *g, *b]),
            (242, DptPayload::ColorXYY { x, y, brightness }) => {
                let xr = (*x * 65535.0) as u16;
                let yr = (*y * 65535.0) as u16;
                let mut bytes = Vec::with_capacity(6);
                bytes.extend_from_slice(&xr.to_be_bytes());
                bytes.extend_from_slice(&yr.to_be_bytes());
                bytes.push(*brightness);
                bytes.push(0x03); // validity flags
                Ok(bytes)
            }
            (251, DptPayload::ColorRGBW { r, g, b, w }) => Ok(vec![*r, *g, *b, *w, 0x00, 0x0F]),
            _ => Err(ProtocolError::DptError {
                dpt_type: self.number_str(),
                details: format!(
                    "cannot encode {:?} for DPT {}",
                    std::mem::discriminant(payload),
                    self.number_str()
                ),
            }
            .into()),
        };
        match &result {
            Ok(bytes) => {
                log_protocol!(
                    LogLevel::Trace,
                    "DPT {} encode: {:?} → {:02X?}",
                    self.number_str(),
                    payload,
                    bytes
                );
            }
            Err(e) => {
                log_protocol!(
                    LogLevel::Warn,
                    "DPT {} encode failed: {}",
                    self.number_str(),
                    e
                );
            }
        }
        result
    }
}
