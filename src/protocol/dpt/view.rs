//! Zero-copy view types for DPT-encoded bytes.

use super::dpt_hvac_status::{
    ChannelActivation8View, ChannelActivation16View, DHWControllerStatusView, DeviceControlView,
    ForcingSignalCoolView, ForcingSignalView, FuelTypeSetView, GeneralStatusView,
    LightActuatorErrorInfoView, PhasesStatusView, RFFilterInfoView, RFModeInfoView,
    RoomCoolingControllerStatusView, RoomHeatingControllerStatusView,
    RoomHeatingCoolingControllerStatusView, SecurityReportView, SolarDHWControllerStatusView,
    StatusHVAView, StatusRTCView, StatusRTSMView, SupportedMediaView,
    VentilationControllerStatusView, VirtualDryContactView,
};
use super::dpt_relative_color::RelativeColorView;
use super::dpt_setpoints::{Setpoints3View, Setpoints4View};
use super::dpt_type::DptType;
use super::dpt4::CharacterView;
use super::dpt15::AccessDataView;
use super::dpt16::StrView;
use super::dpt25::DoubleNibbleView;
use super::dpt26::SceneInfoView;
use super::dpt27::CombinedInfoOnOffView;
use super::dpt30::ChannelActivation24View;
use super::dpt206::ModeNextView;
use super::dpt207::ActuatorView;
use super::dpt217::VersionView;
use super::dpt219::AlarmInfoView;
use super::dpt225::ScalingTimeView;
use super::dpt229::MeteringValueView;
use super::dpt230::MBusAddressView;
use super::dpt234::CodeView;
use super::dpt236::PrioritisedModeControlView;
use super::dpt237::DALIControlGearDiagnosticView;
use super::dpt238::{DALIDiagnosticsView, SceneConfigView};
use super::dpt240::CombinedPositionView;
use super::dpt241::StatusSABView;
use super::dpt243::XYYColorTransitionView;
use super::dpt244::ConverterStatusView;
use super::dpt245::ConverterTestResultView;
use super::dpt246::BatteryInfoView;
use super::dpt247::ConverterTestInfoView;
use super::dpt248::ConverterInfoFixView;
use super::dpt249::ColorTemperatureTransitionView;
use super::dpt255::GeographicalLocationView;
use super::dpt272::ConverterInfoView;
use super::dpt273::ForecastView;

#[derive(Debug, Clone, Copy)]
pub struct BoolView<'a>(pub(crate) &'a [u8]);
impl<'a> BoolView<'a> {
    #[must_use]
    pub fn value(&self) -> bool {
        (self.0[0] & 0x01) != 0
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ControlView<'a>(pub(crate) &'a [u8]);
impl<'a> ControlView<'a> {
    #[must_use]
    pub fn step(&self) -> bool {
        (self.0[0] & 0x08) != 0
    }
    #[must_use]
    pub fn step_code(&self) -> u8 {
        self.0[0] & 0x07
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Control2View<'a>(pub(crate) &'a [u8]);
impl<'a> Control2View<'a> {
    #[must_use]
    pub fn control(&self) -> bool {
        (self.0[0] & 0x02) != 0
    }
    #[must_use]
    pub fn value(&self) -> bool {
        (self.0[0] & 0x01) != 0
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct U8View<'a>(pub(crate) &'a [u8]);
impl<'a> U8View<'a> {
    #[must_use]
    pub fn value(&self) -> u8 {
        self.0[0]
    }
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        f64::from(self.0[0])
    }
    #[must_use]
    pub fn scaled_percent(&self) -> f64 {
        f64::from(self.0[0]) * 100.0 / 255.0
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct I8View<'a>(pub(crate) &'a [u8]);
impl<'a> I8View<'a> {
    #[must_use]
    pub fn value(&self) -> i8 {
        self.0[0] as i8
    }
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        f64::from(self.value())
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct U16View<'a>(pub(crate) &'a [u8]);
impl<'a> U16View<'a> {
    #[must_use]
    pub fn value(&self) -> u16 {
        u16::from_be_bytes([self.0[0], self.0[1]])
    }
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        f64::from(self.value())
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct I16View<'a>(pub(crate) &'a [u8]);
impl<'a> I16View<'a> {
    #[must_use]
    pub fn value(&self) -> i16 {
        i16::from_be_bytes([self.0[0], self.0[1]])
    }
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        f64::from(self.value())
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Float2ByteView<'a>(pub(crate) &'a [u8]);
impl<'a> Float2ByteView<'a> {
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        let b = self.0;
        let raw = (u16::from(b[0]) << 8) | u16::from(b[1]);
        let m = raw & 0x07FF;
        let m = if raw & 0x8000 != 0 {
            (m as i16) - 2048
        } else {
            m as i16
        };
        let e = u32::from((raw >> 11) & 0x0F);
        0.01 * f64::from(m) * f64::from(1u32 << e)
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct U32View<'a>(pub(crate) &'a [u8]);
impl<'a> U32View<'a> {
    #[must_use]
    pub fn value(&self) -> u32 {
        u32::from_be_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        f64::from(self.value())
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct I32View<'a>(pub(crate) &'a [u8]);
impl<'a> I32View<'a> {
    #[must_use]
    pub fn value(&self) -> i32 {
        i32::from_be_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        f64::from(self.value())
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Float4ByteView<'a>(pub(crate) &'a [u8]);
impl<'a> Float4ByteView<'a> {
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        let bytes = [self.0[0], self.0[1], self.0[2], self.0[3]];
        f64::from(f32::from_be_bytes(bytes))
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct I64View<'a>(pub(crate) &'a [u8]);
impl<'a> I64View<'a> {
    #[must_use]
    pub fn value(&self) -> i64 {
        i64::from_be_bytes([
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5], self.0[6], self.0[7],
        ])
    }
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        self.value() as f64
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TimeView<'a>(pub(crate) &'a [u8]);
impl<'a> TimeView<'a> {
    #[must_use]
    pub fn day(&self) -> u8 {
        (self.0[0] >> 5) & 0x07
    }
    #[must_use]
    pub fn hour(&self) -> u8 {
        self.0[0] & 0x1F
    }
    #[must_use]
    pub fn minute(&self) -> u8 {
        self.0[1] & 0x3F
    }
    #[must_use]
    pub fn second(&self) -> u8 {
        self.0[2] & 0x3F
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DateView<'a>(pub(crate) &'a [u8]);
impl<'a> DateView<'a> {
    #[must_use]
    pub fn day(&self) -> u8 {
        self.0[0] & 0x1F
    }
    #[must_use]
    pub fn month(&self) -> u8 {
        self.0[1] & 0x0F
    }
    #[must_use]
    pub fn year(&self) -> u16 {
        let y = u16::from(self.0[2] & 0x7F);
        if y >= 90 { 1900 + y } else { 2000 + y }
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DateTimeView<'a>(pub(crate) &'a [u8]);
impl<'a> DateTimeView<'a> {
    #[must_use]
    pub fn year(&self) -> u16 {
        u16::from(self.0[0]) + 1900
    }
    #[must_use]
    pub fn month(&self) -> u8 {
        self.0[1] & 0x0F
    }
    #[must_use]
    pub fn day(&self) -> u8 {
        self.0[2] & 0x1F
    }
    #[must_use]
    pub fn day_of_week(&self) -> u8 {
        (self.0[3] >> 5) & 0x07
    }
    #[must_use]
    pub fn hour(&self) -> u8 {
        self.0[3] & 0x1F
    }
    #[must_use]
    pub fn minute(&self) -> u8 {
        self.0[4] & 0x3F
    }
    #[must_use]
    pub fn second(&self) -> u8 {
        self.0[5] & 0x3F
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SceneView<'a>(pub(crate) &'a [u8]);
impl<'a> SceneView<'a> {
    #[must_use]
    pub fn scene_number(&self) -> u8 {
        (self.0[0] & 0x3F) + 1
    }
    #[must_use]
    pub fn learn(&self) -> bool {
        (self.0[0] & 0x80) != 0
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EnumView<'a>(pub(crate) &'a [u8]);
impl<'a> EnumView<'a> {
    #[must_use]
    pub fn value(&self) -> u8 {
        self.0[0]
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RgbView<'a>(pub(crate) &'a [u8]);
impl<'a> RgbView<'a> {
    #[must_use]
    pub fn r(&self) -> u8 {
        self.0[0]
    }
    #[must_use]
    pub fn g(&self) -> u8 {
        self.0[1]
    }
    #[must_use]
    pub fn b(&self) -> u8 {
        self.0[2]
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RgbwView<'a>(pub(crate) &'a [u8]);
impl<'a> RgbwView<'a> {
    #[must_use]
    pub fn r(&self) -> u8 {
        self.0[0]
    }
    #[must_use]
    pub fn g(&self) -> u8 {
        self.0[1]
    }
    #[must_use]
    pub fn b(&self) -> u8 {
        self.0[2]
    }
    #[must_use]
    pub fn w(&self) -> u8 {
        self.0[3]
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct XyyView<'a>(pub(crate) &'a [u8]);
impl<'a> XyyView<'a> {
    #[must_use]
    pub fn x(&self) -> f32 {
        f32::from(u16::from_be_bytes([self.0[0], self.0[1]])) / 65535.0
    }
    #[must_use]
    pub fn y(&self) -> f32 {
        f32::from(u16::from_be_bytes([self.0[2], self.0[3]])) / 65535.0
    }
    #[must_use]
    pub fn brightness(&self) -> u8 {
        self.0[4]
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

/// Zero-copy view into DPT-encoded bytes.
#[derive(Debug, Clone, Copy)]
pub enum DptView<'a> {
    Bool(BoolView<'a>),
    Control2(Control2View<'a>),
    Control(ControlView<'a>),
    U8(U8View<'a>),
    I8(I8View<'a>),
    U16(U16View<'a>),
    I16(I16View<'a>),
    Float2Byte(Float2ByteView<'a>),
    U32(U32View<'a>),
    I32(I32View<'a>),
    Float4Byte(Float4ByteView<'a>),
    I64(I64View<'a>),
    Str(StrView<'a>),
    Character(CharacterView<'a>),
    Version(VersionView<'a>),
    DoubleNibble(DoubleNibbleView<'a>),
    SceneInfo(SceneInfoView<'a>),
    CombinedInfoOnOff(CombinedInfoOnOffView<'a>),
    ChannelActivation24(ChannelActivation24View<'a>),
    MBusAddress(MBusAddressView<'a>),
    PrioritisedModeControl(PrioritisedModeControlView<'a>),
    SceneConfig(SceneConfigView<'a>),
    DALIDiagnostics(DALIDiagnosticsView<'a>),
    DALIControlGearDiagnostic(DALIControlGearDiagnosticView<'a>),
    GeographicalLocation(GeographicalLocationView<'a>),
    CombinedPosition(CombinedPositionView<'a>),
    StatusSAB(StatusSABView<'a>),
    AccessData(AccessDataView<'a>),
    ModeNext(ModeNextView<'a>),
    ScalingTime(ScalingTimeView<'a>),
    Actuator(ActuatorView<'a>),
    MeteringValue(MeteringValueView<'a>),
    AlarmInfo(AlarmInfoView<'a>),
    BatteryInfo(BatteryInfoView<'a>),
    ConverterStatus(ConverterStatusView<'a>),
    ConverterInfoFix(ConverterInfoFixView<'a>),
    ConverterInfo(ConverterInfoView<'a>),
    ConverterTestResult(ConverterTestResultView<'a>),
    ConverterTestInfo(ConverterTestInfoView<'a>),
    ColorTemperatureTransition(ColorTemperatureTransitionView<'a>),
    RelativeColor(RelativeColorView<'a>),
    XYYColorTransition(XYYColorTransitionView<'a>),
    ForcingSignal(ForcingSignalView<'a>),
    ForcingSignalCool(ForcingSignalCoolView<'a>),
    RoomHeatingControllerStatus(RoomHeatingControllerStatusView<'a>),
    SolarDHWControllerStatus(SolarDHWControllerStatusView<'a>),
    FuelTypeSet(FuelTypeSetView<'a>),
    RoomCoolingControllerStatus(RoomCoolingControllerStatusView<'a>),
    VentilationControllerStatus(VentilationControllerStatusView<'a>),
    LightActuatorErrorInfo(LightActuatorErrorInfoView<'a>),
    RFFilterInfo(RFFilterInfoView<'a>),
    SecurityReport(SecurityReportView<'a>),
    ChannelActivation8(ChannelActivation8View<'a>),
    VirtualDryContact(VirtualDryContactView<'a>),
    PhasesStatus(PhasesStatusView<'a>),
    RFModeInfo(RFModeInfoView<'a>),
    StatusRTSM(StatusRTSMView<'a>),
    GeneralStatus(GeneralStatusView<'a>),
    DeviceControl(DeviceControlView<'a>),
    DHWControllerStatus(DHWControllerStatusView<'a>),
    SupportedMedia(SupportedMediaView<'a>),
    ChannelActivation16(ChannelActivation16View<'a>),
    RoomHeatingCoolingControllerStatus(RoomHeatingCoolingControllerStatusView<'a>),
    StatusHVA(StatusHVAView<'a>),
    StatusRTC(StatusRTCView<'a>),
    Forecast(ForecastView<'a>),
    Setpoints3(Setpoints3View<'a>),
    Setpoints4(Setpoints4View<'a>),
    Code(CodeView<'a>),
    Time(TimeView<'a>),
    Date(DateView<'a>),
    DateTime(DateTimeView<'a>),
    Scene(SceneView<'a>),
    Enum(EnumView<'a>),
    Rgb(RgbView<'a>),
    Rgbw(RgbwView<'a>),
    Xyy(XyyView<'a>),
}

impl<'a> DptView<'a> {
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Bool(v) => Some(if v.value() { 1.0 } else { 0.0 }),
            Self::U8(v) => Some(v.as_f64()),
            Self::I8(v) => Some(v.as_f64()),
            Self::U16(v) => Some(v.as_f64()),
            Self::I16(v) => Some(v.as_f64()),
            Self::Float2Byte(v) => Some(v.as_f64()),
            Self::U32(v) => Some(v.as_f64()),
            Self::I32(v) => Some(v.as_f64()),
            Self::Float4Byte(v) => Some(v.as_f64()),
            Self::I64(v) => Some(v.as_f64()),
            Self::Scene(v) => Some(f64::from(v.scene_number())),
            Self::Enum(v) => Some(f64::from(v.value())),
            Self::Control(_)
            | Self::Control2(_)
            | Self::Str(_)
            | Self::Character(_)
            | Self::Version(_)
            | Self::DoubleNibble(_)
            | Self::SceneInfo(_)
            | Self::CombinedInfoOnOff(_)
            | Self::ChannelActivation24(_)
            | Self::MBusAddress(_)
            | Self::PrioritisedModeControl(_)
            | Self::SceneConfig(_)
            | Self::DALIDiagnostics(_)
            | Self::DALIControlGearDiagnostic(_)
            | Self::GeographicalLocation(_)
            | Self::CombinedPosition(_)
            | Self::StatusSAB(_)
            | Self::AccessData(_)
            | Self::ModeNext(_)
            | Self::ScalingTime(_)
            | Self::Actuator(_)
            | Self::MeteringValue(_)
            | Self::AlarmInfo(_)
            | Self::BatteryInfo(_)
            | Self::ConverterStatus(_)
            | Self::ConverterInfoFix(_)
            | Self::ConverterInfo(_)
            | Self::ConverterTestResult(_)
            | Self::ConverterTestInfo(_)
            | Self::ColorTemperatureTransition(_)
            | Self::RelativeColor(_)
            | Self::XYYColorTransition(_)
            | Self::ForcingSignal(_)
            | Self::ForcingSignalCool(_)
            | Self::RoomHeatingControllerStatus(_)
            | Self::SolarDHWControllerStatus(_)
            | Self::FuelTypeSet(_)
            | Self::RoomCoolingControllerStatus(_)
            | Self::VentilationControllerStatus(_)
            | Self::LightActuatorErrorInfo(_)
            | Self::RFFilterInfo(_)
            | Self::SecurityReport(_)
            | Self::ChannelActivation8(_)
            | Self::VirtualDryContact(_)
            | Self::PhasesStatus(_)
            | Self::RFModeInfo(_)
            | Self::StatusRTSM(_)
            | Self::GeneralStatus(_)
            | Self::DeviceControl(_)
            | Self::DHWControllerStatus(_)
            | Self::SupportedMedia(_)
            | Self::ChannelActivation16(_)
            | Self::RoomHeatingCoolingControllerStatus(_)
            | Self::StatusHVA(_)
            | Self::StatusRTC(_)
            | Self::Forecast(_)
            | Self::Setpoints3(_)
            | Self::Setpoints4(_)
            | Self::Code(_)
            | Self::Time(_)
            | Self::Date(_)
            | Self::DateTime(_)
            | Self::Rgb(_)
            | Self::Rgbw(_)
            | Self::Xyy(_) => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(v.value()),
            _ => None,
        }
    }

    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        match self {
            Self::Bool(v) => v.raw(),
            Self::Control2(v) => v.raw(),
            Self::Control(v) => v.raw(),
            Self::U8(v) => v.raw(),
            Self::I8(v) => v.raw(),
            Self::U16(v) => v.raw(),
            Self::I16(v) => v.raw(),
            Self::Float2Byte(v) => v.raw(),
            Self::U32(v) => v.raw(),
            Self::I32(v) => v.raw(),
            Self::Float4Byte(v) => v.raw(),
            Self::I64(v) => v.raw(),
            Self::Str(v) => v.raw(),
            Self::Character(v) => v.raw(),
            Self::Version(v) => v.raw(),
            Self::DoubleNibble(v) => v.raw(),
            Self::SceneInfo(v) => v.raw(),
            Self::CombinedInfoOnOff(v) => v.raw(),
            Self::ChannelActivation24(v) => v.raw(),
            Self::MBusAddress(v) => v.raw(),
            Self::PrioritisedModeControl(v) => v.raw(),
            Self::SceneConfig(v) => v.raw(),
            Self::DALIDiagnostics(v) => v.raw(),
            Self::DALIControlGearDiagnostic(v) => v.raw(),
            Self::GeographicalLocation(v) => v.raw(),
            Self::CombinedPosition(v) => v.raw(),
            Self::StatusSAB(v) => v.raw(),
            Self::AccessData(v) => v.raw(),
            Self::ModeNext(v) => v.raw(),
            Self::ScalingTime(v) => v.raw(),
            Self::Actuator(v) => v.raw(),
            Self::MeteringValue(v) => v.raw(),
            Self::AlarmInfo(v) => v.raw(),
            Self::BatteryInfo(v) => v.raw(),
            Self::ConverterStatus(v) => v.raw(),
            Self::ConverterInfoFix(v) => v.raw(),
            Self::ConverterInfo(v) => v.raw(),
            Self::ConverterTestResult(v) => v.raw(),
            Self::ConverterTestInfo(v) => v.raw(),
            Self::ColorTemperatureTransition(v) => v.raw(),
            Self::RelativeColor(v) => v.raw(),
            Self::XYYColorTransition(v) => v.raw(),
            Self::ForcingSignal(v) => v.raw(),
            Self::ForcingSignalCool(v) => v.raw(),
            Self::RoomHeatingControllerStatus(v) => v.raw(),
            Self::SolarDHWControllerStatus(v) => v.raw(),
            Self::FuelTypeSet(v) => v.raw(),
            Self::RoomCoolingControllerStatus(v) => v.raw(),
            Self::VentilationControllerStatus(v) => v.raw(),
            Self::LightActuatorErrorInfo(v) => v.raw(),
            Self::RFFilterInfo(v) => v.raw(),
            Self::SecurityReport(v) => v.raw(),
            Self::ChannelActivation8(v) => v.raw(),
            Self::VirtualDryContact(v) => v.raw(),
            Self::PhasesStatus(v) => v.raw(),
            Self::RFModeInfo(v) => v.raw(),
            Self::StatusRTSM(v) => v.raw(),
            Self::GeneralStatus(v) => v.raw(),
            Self::DeviceControl(v) => v.raw(),
            Self::DHWControllerStatus(v) => v.raw(),
            Self::SupportedMedia(v) => v.raw(),
            Self::ChannelActivation16(v) => v.raw(),
            Self::RoomHeatingCoolingControllerStatus(v) => v.raw(),
            Self::StatusHVA(v) => v.raw(),
            Self::StatusRTC(v) => v.raw(),
            Self::Forecast(v) => v.raw(),
            Self::Setpoints3(v) => v.raw(),
            Self::Setpoints4(v) => v.raw(),
            Self::Code(v) => v.raw(),
            Self::Time(v) => v.raw(),
            Self::Date(v) => v.raw(),
            Self::DateTime(v) => v.raw(),
            Self::Scene(v) => v.raw(),
            Self::Enum(v) => v.raw(),
            Self::Rgb(v) => v.raw(),
            Self::Rgbw(v) => v.raw(),
            Self::Xyy(v) => v.raw(),
        }
    }
}

impl DptView<'_> {
    /// Format as human-readable string with semantic meaning.
    #[must_use]
    pub fn formatted(&self, dpt: DptType) -> String {
        match self {
            Self::Bool(v) => format_bool(v.value(), dpt),
            Self::Control2(v) => {
                format!(
                    "Control({}, {})",
                    v.control(),
                    format_control2(v.value(), dpt)
                )
            }
            Self::Control(v) => format!(
                "Control({}, step={})",
                if v.step() { "increase" } else { "decrease" },
                v.step_code()
            ),
            Self::U8(v) => format_numeric(v.as_f64(), dpt),
            Self::I8(v) => format_numeric(v.as_f64(), dpt),
            Self::U16(v) => format_numeric(v.as_f64(), dpt),
            Self::I16(v) => format_numeric(v.as_f64(), dpt),
            Self::Float2Byte(v) => format_numeric(v.as_f64(), dpt),
            Self::U32(v) => format_numeric(v.as_f64(), dpt),
            Self::I32(v) => format_numeric(v.as_f64(), dpt),
            Self::Float4Byte(v) => format_numeric(v.as_f64(), dpt),
            Self::I64(v) => format_numeric(v.as_f64(), dpt),
            Self::Str(v) => v.as_string(),
            Self::Character(v) => v.as_string(),
            Self::Version(v) => v.value().to_string(),
            Self::DoubleNibble(v) => v.value().to_string(),
            Self::SceneInfo(v) => v.value().to_string(),
            Self::CombinedInfoOnOff(v) => v.value().to_string(),
            Self::ChannelActivation24(v) => v.value().to_string(),
            Self::MBusAddress(v) => v.value().to_string(),
            Self::PrioritisedModeControl(v) => v.value().to_string(),
            Self::SceneConfig(v) => v.value().to_string(),
            Self::DALIDiagnostics(v) => v.value().to_string(),
            Self::DALIControlGearDiagnostic(v) => v.value().to_string(),
            Self::GeographicalLocation(v) => v.value().to_string(),
            Self::CombinedPosition(v) => v.value().to_string(),
            Self::StatusSAB(v) => v.value().to_string(),
            Self::AccessData(v) => v.value().to_string(),
            Self::ModeNext(v) => v.value().to_string(),
            Self::ScalingTime(v) => v.value().to_string(),
            Self::Actuator(v) => v.value().formatted(None),
            Self::MeteringValue(v) => v.value().to_string(),
            Self::AlarmInfo(v) => v.value().to_string(),
            Self::BatteryInfo(v) => v.value().to_string(),
            Self::ConverterStatus(v) => v.value().to_string(),
            Self::ConverterInfoFix(v) => v.value().to_string(),
            Self::ConverterInfo(v) => v.value().to_string(),
            Self::ConverterTestResult(v) => v.value().to_string(),
            Self::ConverterTestInfo(v) => v.value().to_string(),
            Self::ColorTemperatureTransition(v) => v.value().to_string(),
            Self::RelativeColor(v) => v.value().formatted(None),
            Self::XYYColorTransition(v) => v.value().to_string(),
            Self::ForcingSignal(v) => v.value().to_string(),
            Self::ForcingSignalCool(v) => v.value().to_string(),
            Self::RoomHeatingControllerStatus(v) => v.value().to_string(),
            Self::SolarDHWControllerStatus(v) => v.value().to_string(),
            Self::FuelTypeSet(v) => v.value().to_string(),
            Self::RoomCoolingControllerStatus(v) => v.value().to_string(),
            Self::VentilationControllerStatus(v) => v.value().to_string(),
            Self::LightActuatorErrorInfo(v) => v.value().to_string(),
            Self::RFFilterInfo(v) => v.value().to_string(),
            Self::SecurityReport(v) => v.value().to_string(),
            Self::ChannelActivation8(v) => v.value().to_string(),
            Self::VirtualDryContact(v) => v.value().to_string(),
            Self::PhasesStatus(v) => v.value().to_string(),
            Self::RFModeInfo(v) => v.value().to_string(),
            Self::StatusRTSM(v) => v.value().to_string(),
            Self::GeneralStatus(v) => v.value().to_string(),
            Self::DeviceControl(v) => v.value().to_string(),
            Self::DHWControllerStatus(v) => v.value().to_string(),
            Self::SupportedMedia(v) => v.value().to_string(),
            Self::ChannelActivation16(v) => v.value().to_string(),
            Self::RoomHeatingCoolingControllerStatus(v) => v.value().to_string(),
            Self::StatusHVA(v) => v.value().to_string(),
            Self::StatusRTC(v) => v.value().to_string(),
            Self::Forecast(v) => v.value().to_string(),
            Self::Setpoints3(v) => v.value().to_string(),
            Self::Setpoints4(v) => v.value().to_string(),
            Self::Code(v) => v.value(),
            Self::Time(v) => {
                let base = format!("{:02}:{:02}:{:02}", v.hour(), v.minute(), v.second());
                match weekday_name(v.day()) {
                    Some(day) => format!("{base} ({day})"),
                    None => base,
                }
            }
            Self::Date(v) => format!("{:04}-{:02}-{:02}", v.year(), v.month(), v.day()),
            Self::DateTime(v) => {
                let base = format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    v.year(),
                    v.month(),
                    v.day(),
                    v.hour(),
                    v.minute(),
                    v.second()
                );
                match weekday_name(v.day_of_week()) {
                    Some(day) => format!("{base} ({day})"),
                    None => base,
                }
            }
            Self::Scene(v) => match dpt {
                DptType::SceneControl => format!(
                    "Scene {} ({})",
                    v.scene_number(),
                    if v.learn() { "learn" } else { "activate" }
                ),
                _ => format!("Scene {}", v.scene_number()),
            },
            Self::Enum(v) => format_enum(v.value(), dpt),
            Self::Rgb(v) => format!("RGB({}, {}, {})", v.r(), v.g(), v.b()),
            Self::Rgbw(v) => format!("RGBW({}, {}, {}, {})", v.r(), v.g(), v.b(), v.w()),
            Self::Xyy(v) => format!("XYY({:.3}, {:.3}, {})", v.x(), v.y(), v.brightness()),
        }
    }
}

fn format_bool(value: bool, dpt: DptType) -> String {
    bool_label(value, dpt).to_string()
}

/// Semantic label for a DPT 1.xxx boolean value, one pair per subtype.
pub(crate) fn bool_label(value: bool, dpt: DptType) -> &'static str {
    match dpt {
        DptType::Switch => {
            if value {
                "On"
            } else {
                "Off"
            }
        }
        // DptType::Bool falls through to the wildcard below (True/False).
        DptType::Enable => {
            if value {
                "Enable"
            } else {
                "Disable"
            }
        }
        DptType::Ramp => {
            if value {
                "Ramp"
            } else {
                "No Ramp"
            }
        }
        DptType::Alarm => {
            if value {
                "Alarm"
            } else {
                "No Alarm"
            }
        }
        DptType::BinaryValue => {
            if value {
                "High"
            } else {
                "Low"
            }
        }
        DptType::Step => {
            if value {
                "Increase"
            } else {
                "Decrease"
            }
        }
        DptType::UpDown => {
            if value {
                "Down"
            } else {
                "Up"
            }
        }
        DptType::OpenClose | DptType::WindowDoor => {
            if value {
                "Open"
            } else {
                "Closed"
            }
        }
        DptType::Start => {
            if value {
                "Start"
            } else {
                "Stop"
            }
        }
        DptType::State => {
            if value {
                "Active"
            } else {
                "Inactive"
            }
        }
        DptType::Invert => {
            if value {
                "Inverted"
            } else {
                "Not Inverted"
            }
        }
        DptType::DimSendStyle => {
            if value {
                "Cyclically"
            } else {
                "Start/Stop"
            }
        }
        DptType::InputSource => {
            if value {
                "Calculated"
            } else {
                "Fixed"
            }
        }
        DptType::Reset => {
            if value {
                "Reset"
            } else {
                "No Action"
            }
        }
        DptType::Ack => {
            if value {
                "Acknowledge"
            } else {
                "No Action"
            }
        }
        DptType::Trigger => {
            if value {
                "Trigger"
            } else {
                "Trigger 0"
            }
        }
        DptType::Occupancy => {
            if value {
                "Occupied"
            } else {
                "Not Occupied"
            }
        }
        DptType::LogicalFunction => {
            if value {
                "And"
            } else {
                "Or"
            }
        }
        DptType::SceneAB => {
            if value {
                "Scene B"
            } else {
                "Scene A"
            }
        }
        DptType::ShutterBlindsMode => {
            if value {
                "Step/Stop Mode"
            } else {
                "Up/Down Mode"
            }
        }
        DptType::DayNight => {
            if value {
                "Night"
            } else {
                "Day"
            }
        }
        DptType::HeatCool => {
            if value {
                "Heat"
            } else {
                "Cool"
            }
        }
        DptType::ConsumerProducer => {
            if value {
                "Producer"
            } else {
                "Consumer"
            }
        }
        DptType::EnergyDirection => {
            if value {
                "Negative"
            } else {
                "Positive"
            }
        }
        _ => {
            if value {
                "True"
            } else {
                "False"
            }
        }
    }
}

/// The DPT 1.xxx type whose labels a DPT 2.xxx control type reuses for its
/// value bit (e.g. `SwitchControl`'s value bit is labeled like `Switch`).
pub(crate) fn control2_underlying(dpt: DptType) -> DptType {
    match dpt {
        DptType::SwitchControl => DptType::Switch,
        DptType::EnableControl => DptType::Enable,
        DptType::RampControl => DptType::Ramp,
        DptType::AlarmControl => DptType::Alarm,
        DptType::BinaryValueControl => DptType::BinaryValue,
        DptType::StepControl => DptType::Step,
        DptType::Direction1Control | DptType::Direction2Control => DptType::UpDown,
        DptType::StartControl => DptType::Start,
        DptType::StateControl => DptType::State,
        DptType::InvertControl => DptType::Invert,
        _ => DptType::Bool,
    }
}

/// Semantic label for a DPT 2.xxx control value bit, using the same labels
/// as the corresponding DPT 1.xxx type.
fn format_control2(value: bool, dpt: DptType) -> &'static str {
    bool_label(value, control2_underlying(dpt))
}

// `value == value.floor()` is an intentional whole-number test, not a fuzzy compare.
#[allow(clippy::float_cmp)]
fn format_numeric(value: f64, dpt: DptType) -> String {
    match dpt.unit() {
        Some(unit) => {
            if value == value.floor() {
                format!("{} {}", value as i64, unit.symbol())
            } else {
                format!("{:.1} {}", value, unit.symbol())
            }
        }
        None => {
            if value == value.floor() {
                format!("{}", value as i64)
            } else {
                format!("{value:.1}")
            }
        }
    }
}

/// Weekday label for the 3-bit day-of-week field in DPT 10 (Time) and
/// DPT 19 (`DateTime`). `0` means "no day"/"any day" and has no label.
pub(super) fn weekday_name(day: u8) -> Option<&'static str> {
    match day {
        1 => Some("Monday"),
        2 => Some("Tuesday"),
        3 => Some("Wednesday"),
        4 => Some("Thursday"),
        5 => Some("Friday"),
        6 => Some("Saturday"),
        7 => Some("Sunday"),
        _ => None,
    }
}

fn format_enum(value: u8, dpt: DptType) -> String {
    match dpt {
        DptType::ErrorClassSystem => crate::protocol::dpt::ErrorClassSystem::try_from(value)
            .map_or_else(
                |_| format!("System Error Class {value}"),
                |class| class.label().to_owned(),
            ),
        DptType::ChangeoverMode => ["Auto", "Cooling Only", "Heating Only"]
            .get(value as usize)
            .unwrap_or(&"Unknown")
            .to_string(),
        DptType::EnableHCStage => ["Disabled", "Stage A", "Stage B", "Both Stages"]
            .get(value as usize)
            .unwrap_or(&"Unknown")
            .to_string(),
        DptType::HVACMode => match value {
            0 => "Auto".to_string(),
            1 => "Comfort".to_string(),
            2 => "Standby".to_string(),
            3 => "Economy".to_string(),
            4 => "Building Protection".to_string(),
            _ => format!("HVAC Mode {value}"),
        },
        DptType::HVACContrMode => match value {
            0 => "Auto".to_string(),
            1 => "Heat".to_string(),
            2 => "Morning Warmup".to_string(),
            3 => "Cool".to_string(),
            4 => "Night Purge".to_string(),
            5 => "Precool".to_string(),
            6 => "Off".to_string(),
            7 => "Test".to_string(),
            8 => "Emergency Heat".to_string(),
            9 => "Fan Only".to_string(),
            10 => "Free Cool".to_string(),
            11 => "Ice".to_string(),
            12 => "Maximum Heating Mode".to_string(),
            13 => "Economic Heat/Cool Mode".to_string(),
            14 => "Dehumidification".to_string(),
            15 => "Calibration Mode".to_string(),
            16 => "Emergency Cool Mode".to_string(),
            17 => "Emergency Steam Mode".to_string(),
            20 => "No Demand".to_string(),
            _ => format!("HVAC Controller Mode {value}"),
        },
        _ => format!("Enum({value})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_view_byte_layout() {
        // DPT 19.001: 2026-06-11 (Thu) 08:14:22
        // byte0: year offset from 1900 -> 2026-1900 = 126 = 0x7E
        // byte1: month (low nibble) -> 6
        // byte2: day -> 11
        // byte3: day_of_week (bits 5-7) + hour (bits 0-4) -> Thu(4)<<5 | 8 = 0x88
        // byte4: minute -> 14
        // byte5: second -> 22
        // byte6/7: flags (0)
        let bytes = [0x7E, 0x06, 0x0B, (4 << 5) | 8, 14, 22, 0x00, 0x00];
        let v = DateTimeView(&bytes);
        assert_eq!(v.year(), 2026);
        assert_eq!(v.month(), 6);
        assert_eq!(v.day(), 11);
        assert_eq!(v.day_of_week(), 4);
        assert_eq!(v.hour(), 8);
        assert_eq!(v.minute(), 14);
        assert_eq!(v.second(), 22);
        let formatted = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            v.year(),
            v.month(),
            v.day(),
            v.hour(),
            v.minute(),
            v.second()
        );
        assert_eq!(formatted, "2026-06-11 08:14:22");
    }
}
