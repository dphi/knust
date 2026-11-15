//! Zero-copy view types for DPT-encoded bytes.

use super::dpt_type::DptType;
use super::payload::DptPayload;
use crate::error::{ProtocolError, Result};

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
pub struct StrView<'a>(pub(crate) &'a [u8], pub(crate) bool);
impl<'a> StrView<'a> {
    #[must_use]
    pub fn as_string(&self) -> String {
        if self.1 {
            super::dpt16::decode_latin1_value(self.0)
        } else {
            super::dpt16::decode_ascii_value(self.0)
        }
    }

    /// View the bytes as UTF-8 up to the first NUL, if valid.
    #[must_use]
    pub fn as_str(&self) -> &'a str {
        let end = self.0.iter().position(|&b| b == 0).unwrap_or(self.0.len());
        std::str::from_utf8(&self.0[..end]).unwrap_or("")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

/// Zero-copy view of a DPT 4 ASCII or Latin-1 character.
#[derive(Debug, Clone, Copy)]
pub struct CharacterView<'a>(pub(crate) &'a [u8], pub(crate) bool);

impl<'a> CharacterView<'a> {
    /// Decode the character; NUL yields an empty string.
    #[must_use]
    pub fn as_string(&self) -> String {
        match self.0[0] {
            0 => String::new(),
            byte if self.1 || byte.is_ascii() => char::from(byte).to_string(),
            _ => "�".to_string(),
        }
    }

    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

/// Borrowed DPT 217.001 version payload.
#[derive(Debug, Clone, Copy)]
pub struct VersionView<'a>(pub(crate) &'a [u8]);

impl<'a> VersionView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt217::KnxVersion {
        super::dpt217::KnxVersion::from_raw([self.0[0], self.0[1]])
    }

    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CodeView<'a>(pub(crate) &'a [u8], pub(crate) bool);
impl<'a> CodeView<'a> {
    #[must_use]
    pub fn value(&self) -> String {
        super::dpt234::decode_code(self.0, self.1).expect("validated code")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SceneConfigView<'a>(pub(crate) &'a [u8]);
impl<'a> SceneConfigView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt238::SceneConfig {
        super::dpt238::SceneConfig::decode(self.0[0])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
#[derive(Debug, Clone, Copy)]
pub struct DALIDiagnosticsView<'a>(pub(crate) &'a [u8]);
impl<'a> DALIDiagnosticsView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt238::DALIDiagnostics {
        super::dpt238::DALIDiagnostics::decode(self.0[0])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Setpoints3View<'a>(pub(crate) &'a [u8], pub(crate) bool);
impl<'a> Setpoints3View<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt_setpoints::RoomTemperatureSetpoints3 {
        super::dpt_setpoints::RoomTemperatureSetpoints3::decode(self.0, self.1)
            .expect("validated fields")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Setpoints4View<'a>(pub(crate) &'a [u8], pub(crate) bool);
impl<'a> Setpoints4View<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt_setpoints::RoomTemperatureSetpoints4 {
        super::dpt_setpoints::RoomTemperatureSetpoints4::decode(self.0, self.1)
            .expect("validated fields")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ForecastView<'a>(pub(crate) &'a [u8], pub(crate) u16);
impl<'a> ForecastView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt273::Forecast {
        super::dpt273::Forecast::decode(self.0, self.1).expect("validated forecast")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

macro_rules! status_view {
    ($view:ident,$value:ident) => {
        #[derive(Debug, Clone, Copy)]
        pub struct $view<'a>(pub(crate) &'a [u8]);
        impl<'a> $view<'a> {
            #[must_use]
            pub fn value(&self) -> super::dpt_hvac_status::$value {
                super::dpt_hvac_status::$value::decode(self.0)
            }
            #[must_use]
            pub fn raw(&self) -> &'a [u8] {
                self.0
            }
        }
    };
}
status_view!(LightActuatorErrorInfoView, LightActuatorErrorInfo);
status_view!(RFFilterInfoView, RFFilterInfo);
status_view!(SecurityReportView, SecurityReport);
status_view!(ChannelActivation8View, ChannelActivation8);
status_view!(VirtualDryContactView, VirtualDryContact);
status_view!(PhasesStatusView, PhasesStatus);
status_view!(RFModeInfoView, RFModeInfo);
status_view!(StatusRTSMView, CombinedStatusRTSM);
status_view!(ForcingSignalView, ForcingSignal);
status_view!(ForcingSignalCoolView, ForcingSignalCool);
status_view!(RoomHeatingControllerStatusView, RoomHeatingControllerStatus);
status_view!(SolarDHWControllerStatusView, SolarDHWControllerStatus);
status_view!(FuelTypeSetView, FuelTypeSet);
status_view!(RoomCoolingControllerStatusView, RoomCoolingControllerStatus);
status_view!(VentilationControllerStatusView, VentilationControllerStatus);
status_view!(GeneralStatusView, GeneralStatus);
status_view!(DeviceControlView, DeviceControl);
status_view!(DHWControllerStatusView, DHWControllerStatus);
status_view!(SupportedMediaView, SupportedMedia);
status_view!(ChannelActivation16View, ChannelActivation16);
status_view!(StatusHVAView, CombinedStatusHVA);
status_view!(StatusRTCView, CombinedStatusRTC);

#[derive(Debug, Clone, Copy)]
pub struct XYYColorTransitionView<'a>(pub(crate) &'a [u8]);
impl<'a> XYYColorTransitionView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt243::XYYColorTransition {
        super::dpt243::XYYColorTransition::decode(self.0.try_into().expect("validated length"))
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RelativeColorView<'a>(pub(crate) &'a [u8], pub(crate) u16);
impl<'a> RelativeColorView<'a> {
    #[must_use]
    pub fn value(&self) -> super::DptPayload {
        super::dpt_relative_color::decode_payload(self.0, self.1).expect("validated length")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ColorTemperatureTransitionView<'a>(pub(crate) &'a [u8]);
impl<'a> ColorTemperatureTransitionView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt249::ColorTemperatureTransition {
        super::dpt249::ColorTemperatureTransition::decode(
            self.0.try_into().expect("validated length"),
        )
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConverterTestInfoView<'a>(pub(crate) &'a [u8]);
impl<'a> ConverterTestInfoView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt247::ConverterTestInfo {
        super::dpt247::ConverterTestInfo::decode(self.0).expect("validated date/time")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConverterTestResultView<'a>(pub(crate) &'a [u8]);
impl<'a> ConverterTestResultView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt245::ConverterTestResult {
        super::dpt245::ConverterTestResult::decode(self.0.try_into().expect("validated length"))
            .expect("validated result")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConverterInfoView<'a>(pub(crate) &'a [u8]);
impl<'a> ConverterInfoView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt272::ConverterInfo {
        super::dpt272::ConverterInfo::decode(self.0.try_into().expect("validated length"))
            .expect("validated information")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConverterInfoFixView<'a>(pub(crate) &'a [u8]);
impl<'a> ConverterInfoFixView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt248::ConverterInfoFix {
        super::dpt248::ConverterInfoFix::decode(self.0.try_into().expect("validated length"))
            .expect("validated information")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConverterStatusView<'a>(pub(crate) &'a [u8]);
impl<'a> ConverterStatusView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt244::ConverterStatus {
        super::dpt244::ConverterStatus::decode([self.0[0], self.0[1]]).expect("validated status")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BatteryInfoView<'a>(pub(crate) &'a [u8]);
impl<'a> BatteryInfoView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt246::BatteryInfo {
        super::dpt246::BatteryInfo::decode([self.0[0], self.0[1]])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AlarmInfoView<'a>(pub(crate) &'a [u8]);
impl<'a> AlarmInfoView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt219::AlarmInfo {
        super::dpt219::AlarmInfo::decode(self.0.try_into().expect("validated length"))
            .expect("validated alarm")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MeteringValueView<'a>(pub(crate) &'a [u8]);
impl<'a> MeteringValueView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt229::MeteringValue {
        super::dpt229::MeteringValue::decode(self.0.try_into().expect("validated length"))
            .expect("validated unit")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ActuatorView<'a>(pub(crate) &'a [u8], pub(crate) u16);
impl<'a> ActuatorView<'a> {
    #[must_use]
    pub fn value(&self) -> super::DptPayload {
        super::dpt207::decode_payload([self.0[0], self.0[1]], self.1).expect("validated actuator")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ScalingTimeView<'a>(pub(crate) &'a [u8], pub(crate) u16);
impl<'a> ScalingTimeView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt225::ScalingTime {
        super::dpt225::ScalingTime::decode([self.0[0], self.0[1], self.0[2]], self.1)
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModeNextView<'a>(pub(crate) &'a [u8], pub(crate) u16);
impl<'a> ModeNextView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt206::ModeNext {
        super::dpt206::ModeNext::decode([self.0[0], self.0[1], self.0[2]], self.1)
            .expect("validated mode")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Clone, Copy)]
pub struct AccessDataView<'a>(pub(crate) &'a [u8]);
impl std::fmt::Debug for AccessDataView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value().fmt(f)
    }
}
impl<'a> AccessDataView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt15::AccessData {
        super::dpt15::AccessData::decode(self.0.try_into().expect("validated length"))
            .expect("validated access data")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StatusSABView<'a>(pub(crate) &'a [u8]);
impl<'a> StatusSABView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt241::StatusSAB {
        super::dpt241::StatusSAB::decode([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CombinedPositionView<'a>(pub(crate) &'a [u8]);
impl<'a> CombinedPositionView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt240::CombinedPosition {
        super::dpt240::CombinedPosition::decode([self.0[0], self.0[1], self.0[2]])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GeographicalLocationView<'a>(pub(crate) &'a [u8]);
impl<'a> GeographicalLocationView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt255::GeographicalLocation {
        super::dpt255::GeographicalLocation::decode(self.0.try_into().expect("validated length"))
            .expect("validated coordinates")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DALIControlGearDiagnosticView<'a>(pub(crate) &'a [u8]);
impl<'a> DALIControlGearDiagnosticView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt237::DALIControlGearDiagnostic {
        super::dpt237::DALIControlGearDiagnostic::decode([self.0[0], self.0[1]])
            .expect("validated address")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PrioritisedModeControlView<'a>(pub(crate) &'a [u8]);
impl<'a> PrioritisedModeControlView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt236::PrioritisedModeControl {
        super::dpt236::PrioritisedModeControl::decode(self.0[0])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MBusAddressView<'a>(pub(crate) &'a [u8]);
impl<'a> MBusAddressView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt230::MBusAddress {
        super::dpt230::MBusAddress::decode(self.0.try_into().expect("validated length"))
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChannelActivation24View<'a>(pub(crate) &'a [u8]);
impl<'a> ChannelActivation24View<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt30::ChannelActivation24 {
        super::dpt30::ChannelActivation24::decode([self.0[0], self.0[1], self.0[2]])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CombinedInfoOnOffView<'a>(pub(crate) &'a [u8]);
impl<'a> CombinedInfoOnOffView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt27::CombinedInfoOnOff {
        super::dpt27::CombinedInfoOnOff::decode([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SceneInfoView<'a>(pub(crate) &'a [u8]);
impl<'a> SceneInfoView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt26::SceneInfo {
        super::dpt26::SceneInfo::decode(self.0[0])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DoubleNibbleView<'a>(pub(crate) &'a [u8]);
impl<'a> DoubleNibbleView<'a> {
    #[must_use]
    pub fn value(&self) -> super::dpt25::DoubleNibble {
        super::dpt25::DoubleNibble {
            busy: self.0[0] >> 4,
            nak: self.0[0] & 15,
        }
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
fn weekday_name(day: u8) -> Option<&'static str> {
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

impl DptType {
    /// Parse a human-readable value into raw bytes for this DPT type.
    ///
    /// This is the reverse of [`DptView::formatted`]: it accepts the same
    /// formats that method produces (e.g. `"On"`, `"23.5 °C"`,
    /// `"2026-06-11"`), plus common synonyms for booleans (`true`/`false`,
    /// `1`/`0`).
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `input` doesn't parse into a
    /// value this DPT can encode, or if the parsed value is out of range.
    pub fn parse(&self, input: &str) -> Result<Vec<u8>> {
        let input = if matches!(self.main(), 4 | 16 | 234) {
            input
        } else {
            input.trim()
        };
        let payload = (*self).parse_payload(input)?;
        self.encode(&payload)
    }

    fn parse_payload(self, s: &str) -> Result<DptPayload> {
        let bad = |details: String| -> crate::error::KnxError {
            ProtocolError::DptError {
                dpt_type: self.number_str(),
                details,
            }
            .into()
        };
        match self.main() {
            1 => parse_bool(s, self)
                .map(DptPayload::Bool)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a boolean"))),
            2 => {
                let value = parse_bool(s, control2_underlying(self))
                    .ok_or_else(|| bad(format!("cannot parse {s:?} as a boolean")))?;
                Ok(DptPayload::BinaryControl {
                    control: true,
                    value,
                })
            }
            3 => parse_control3(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as a control step (try \"increase 3\", \"decrease 3\", or \"stop\")"
                ))
            }),
            5 | 6 | 7 | 8 | 12 | 13 => {
                let value = parse_number(s, self)
                    .ok_or_else(|| bad(format!("cannot parse {s:?} as a number")))?;
                let (min, max) = int_range(self.main());
                if value < min || value > max {
                    return Err(bad(format!("{value} out of range {min}..={max}")));
                }
                if matches!(self.main(), 6 | 8 | 13) {
                    Ok(DptPayload::SignedInt(value as i64))
                } else {
                    Ok(DptPayload::UnsignedInt(value as u64))
                }
            }
            9 => {
                const MIN: f64 = -671_088.64;
                const MAX: f64 = 670_433.28;
                let value = parse_number(s, self)
                    .ok_or_else(|| bad(format!("cannot parse {s:?} as a number")))?;
                if !(MIN..=MAX).contains(&value) {
                    return Err(bad(format!("{value} out of range {MIN}..={MAX}")));
                }
                Ok(DptPayload::Float(value))
            }
            14 => {
                let value = parse_number(s, self)
                    .ok_or_else(|| bad(format!("cannot parse {s:?} as a number")))?;
                if value.is_finite() && !(value as f32).is_finite() {
                    return Err(bad(format!("{value} out of range for a 4-byte float")));
                }
                Ok(DptPayload::Float(value))
            }
            29 => s
                .parse::<i64>()
                .map(DptPayload::SignedInt)
                .map_err(|_| bad(format!("cannot parse {s:?} as an integer"))),
            10 => parse_time(s)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a time (expected \"HH:MM:SS\")"))),
            11 => parse_date(
                s,
            )
            .ok_or_else(|| bad(format!("cannot parse {s:?} as a date (expected \"YYYY-MM-DD\")"))),
            19 => parse_datetime(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as a date/time (expected \"YYYY-MM-DD HH:MM:SS\")"
                ))
            }),
            4 => Ok(DptPayload::String(s.to_string())),
            16 => Ok(DptPayload::String(s.to_string())),
            17 => parse_scene_number(s)
                .map(DptPayload::Scene)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a scene number (1-64)"))),
            18 => parse_scene_control(s)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a scene control value"))),
            20 => parse_enum(s, self)
                .map(DptPayload::Enum)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as an enum value"))),
            217 => s.parse().map(DptPayload::Version),
            25 => s.parse().map(DptPayload::DoubleNibble),
            26 => s.parse().map(DptPayload::SceneInfo),
            27 => s.parse().map(DptPayload::CombinedInfoOnOff),
            30 => s.parse().map(DptPayload::ChannelActivation24),
            230 => s.parse().map(DptPayload::MBusAddress),
            236 => s.parse().map(DptPayload::PrioritisedModeControl),
            237 => s.parse().map(DptPayload::DALIControlGearDiagnostic),
            255 => s.parse().map(DptPayload::GeographicalLocation),
            240 => s.parse().map(DptPayload::CombinedPosition),
            241 => s.parse().map(DptPayload::StatusSAB),
            15 => s.parse().map(DptPayload::AccessData),
            206 => s.parse().map(DptPayload::ModeNext),
            225 => s.parse().map(DptPayload::ScalingTime),
            207 => super::dpt207::parse_payload(s,self.sub()),
            229 => s.parse().map(DptPayload::MeteringValue),
            219 => s.parse().map(DptPayload::AlarmInfo),
            246 => s.parse().map(DptPayload::BatteryInfo),
            244 => s.parse().map(DptPayload::ConverterStatus),
            248 => s.parse().map(DptPayload::ConverterInfoFix),
            272 => s.parse().map(DptPayload::ConverterInfo),
            245 => s.parse().map(DptPayload::ConverterTestResult),
            247 => s.parse().map(DptPayload::ConverterTestInfo),
            249 => s.parse().map(DptPayload::ColorTemperatureTransition),
            250 | 252..=254 => super::dpt_relative_color::parse_payload(s,self.main()),
            243 => s.parse().map(DptPayload::XYYColorTransition),
            21 if self.sub() == 1000 => s.parse().map(DptPayload::RFModeInfo),
            21 if self.sub() == 1 => s.parse().map(DptPayload::GeneralStatus),
            21 if self.sub() == 1200 => s.parse().map(DptPayload::VirtualDryContact),
            21 if self.sub() == 1201 => s.parse().map(DptPayload::PhasesStatus),
            21 if self.sub() == 1001 => s.parse().map(DptPayload::RFFilterInfo),
            21 if self.sub() == 1002 => s.parse().map(DptPayload::SecurityReport),
            21 if self.sub() == 1010 => s.parse().map(DptPayload::ChannelActivation8),
            21 if self.sub() == 601 => s.parse().map(DptPayload::LightActuatorErrorInfo),
            21 if self.sub() == 100 => s.parse().map(DptPayload::ForcingSignal),
            21 if self.sub() == 101 => s.parse().map(DptPayload::ForcingSignalCool),
            21 if self.sub() == 102 => s.parse().map(DptPayload::RoomHeatingControllerStatus),
            21 if self.sub() == 103 => s.parse().map(DptPayload::SolarDHWControllerStatus),
            21 if self.sub() == 104 => s.parse().map(DptPayload::FuelTypeSet),
            21 if self.sub() == 105 => s.parse().map(DptPayload::RoomCoolingControllerStatus),
            21 if self.sub() == 106 => s.parse().map(DptPayload::VentilationControllerStatus),
            21 if self.sub() == 2 => s.parse().map(DptPayload::DeviceControl),
            21 => s.parse().map(DptPayload::StatusRTSM),
            22 if self.sub() == 100 => s.parse().map(DptPayload::DHWControllerStatus),
            22 if self.sub() == 1000 => s.parse().map(DptPayload::SupportedMedia),
            22 if self.sub() == 1010 => s.parse().map(DptPayload::ChannelActivation16),
            22 if self.sub() == 102 => s.parse().map(DptPayload::StatusHVA),
            22 => s.parse().map(DptPayload::StatusRTC),
            273 => s.parse().map(DptPayload::Forecast),
            222 => s.parse().map(DptPayload::Setpoints3),
            275 => s.parse().map(DptPayload::Setpoints4),
            238 if self.sub() == 1 => s.parse().map(DptPayload::SceneConfig),
            238 => s.parse().map(DptPayload::DALIDiagnostics),
            234 => Ok(DptPayload::String(s.to_string())),
            232 => parse_rgb(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as RGB (try \"RGB(r, g, b)\" or \"#RRGGBB\")"
                ))
            }),
            242 => parse_xyy(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as XYY (try \"XYY(x, y, brightness)\")"
                ))
            }),
            251 => parse_rgbw(s).ok_or_else(|| {
                bad(format!(
                    "cannot parse {s:?} as RGBW (try \"RGBW(r, g, b, w)\" or \"#RRGGBBWW\")"
                ))
            }),
            _ => Err(bad("unsupported DPT main number".to_string())),
        }
    }
}

/// Parse a boolean, accepting the subtype's own semantic label pair (e.g.
/// "Open"/"Closed" for `OpenClose`) case-insensitively, plus generic
/// true/false, 1/0, on/off, yes/no synonyms.
fn parse_bool(s: &str, dpt: DptType) -> Option<bool> {
    let lower = s.to_ascii_lowercase();
    if lower == bool_label(true, dpt).to_ascii_lowercase() {
        return Some(true);
    }
    if lower == bool_label(false, dpt).to_ascii_lowercase() {
        return Some(false);
    }
    match lower.as_str() {
        "true" | "1" | "on" | "yes" => Some(true),
        "false" | "0" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// Parse a DPT 3.xxx control value: "stop"/"break", "increase N"/"decrease
/// N" (also "up"/"down"), or signed shorthand like "+3"/"-3".
fn parse_control3(s: &str) -> Option<DptPayload> {
    let lower = s.trim().to_ascii_lowercase();
    if lower == "stop" || lower == "break" {
        return Some(DptPayload::Control {
            step: false,
            step_code: 0,
        });
    }
    if let Some((word, rest)) = lower.split_once(char::is_whitespace) {
        let step = match word {
            "increase" | "up" => true,
            "decrease" | "down" => false,
            _ => return None,
        };
        let step_code: u8 = rest.trim().parse().ok()?;
        return (step_code <= 7).then_some(DptPayload::Control { step, step_code });
    }
    let n: i8 = lower.parse().ok()?;
    let step_code = n.unsigned_abs();
    (step_code <= 7).then_some(DptPayload::Control {
        step: n >= 0,
        step_code,
    })
}

/// Valid (min, max) range for the raw integer encoding of a numeric DPT
/// main number. Unbounded mains (e.g. DPT 29's i64) return the full f64
/// range.
fn int_range(main: u16) -> (f64, f64) {
    match main {
        5 => (0.0, 255.0),
        6 => (-128.0, 127.0),
        7 => (0.0, 65535.0),
        8 => (-32768.0, 32767.0),
        12 => (0.0, 4_294_967_295.0),
        13 => (-2_147_483_648.0, 2_147_483_647.0),
        _ => (f64::MIN, f64::MAX),
    }
}

/// Parse a plain or unit-suffixed number, stripping the DPT's own unit
/// symbol if present (e.g. `"23.5 °C"` for a `Temperature` DPT).
fn parse_number(s: &str, dpt: DptType) -> Option<f64> {
    let s = match dpt.unit() {
        Some(unit) => s.strip_suffix(unit.symbol()).map_or(s, str::trim),
        None => s,
    };
    s.trim().parse::<f64>().ok()
}

/// Split a trailing `" (Weekday)"` suffix off, returning the day-of-week
/// number (0 if absent or unrecognized).
fn split_weekday_suffix(s: &str) -> (&str, u8) {
    if let Some(open) = s.rfind('(') {
        if let Some(close_offset) = s[open..].find(')') {
            let name = s[open + 1..open + close_offset].trim();
            if let Some(day) = weekday_number(name) {
                return (s[..open].trim(), day);
            }
        }
    }
    (s.trim(), 0)
}

fn weekday_number(name: &str) -> Option<u8> {
    (1..=7).find(|&d| weekday_name(d) == Some(name))
}

fn parse_time(s: &str) -> Option<DptPayload> {
    let (time_part, day) = split_weekday_suffix(s);
    let mut it = time_part.splitn(3, ':');
    let hour: u8 = it.next()?.trim().parse().ok()?;
    let minute: u8 = it.next()?.trim().parse().ok()?;
    let second: u8 = it.next()?.trim().parse().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(DptPayload::Time {
        day,
        hour,
        minute,
        second,
    })
}

fn parse_date(s: &str) -> Option<DptPayload> {
    let mut it = s.trim().splitn(3, '-');
    let year: u16 = it.next()?.parse().ok()?;
    let month: u8 = it.next()?.parse().ok()?;
    let day: u8 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(DptPayload::Date { day, month, year })
}

fn parse_datetime(s: &str) -> Option<DptPayload> {
    let (main_part, day_of_week) = split_weekday_suffix(s);
    let (date_part, time_part) = main_part.split_once(' ')?;
    let Some(DptPayload::Date { day, month, year }) = parse_date(date_part) else {
        return None;
    };
    let mut it = time_part.trim().splitn(3, ':');
    let hour: u8 = it.next()?.trim().parse().ok()?;
    let minute: u8 = it.next()?.trim().parse().ok()?;
    let second: u8 = it.next()?.trim().parse().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(DptPayload::DateTime {
        year,
        month,
        day,
        day_of_week,
        hour,
        minute,
        second,
    })
}

fn parse_scene_number(s: &str) -> Option<u8> {
    let s = s.trim();
    let digits = s
        .strip_prefix("Scene ")
        .or_else(|| s.strip_prefix("scene "))
        .unwrap_or(s);
    let n: u8 = digits.trim().parse().ok()?;
    (1..=64).contains(&n).then_some(n)
}

/// Parse a DPT 18 scene control value: `"Scene N (learn)"`,
/// `"Scene N (activate)"`, or a bare scene number (defaults to activate).
fn parse_scene_control(s: &str) -> Option<DptPayload> {
    let s = s.trim();
    let (num_part, learn) = match s.strip_suffix(')').and_then(|s| {
        let idx = s.rfind('(')?;
        Some((&s[..idx], s[idx + 1..].trim().to_ascii_lowercase()))
    }) {
        Some((num_part, mode)) => (num_part.trim(), mode == "learn"),
        None => (s, false),
    };
    let scene = parse_scene_number(num_part)?;
    Some(DptPayload::SceneControl { scene, learn })
}

fn parse_enum(s: &str, dpt: DptType) -> Option<u8> {
    if let Ok(n) = s.trim().parse::<u8>() {
        return Some(n);
    }
    let lower = s.trim().to_ascii_lowercase();
    match dpt {
        DptType::ChangeoverMode => match lower.as_str() {
            "auto" => Some(0),
            "cooling only" | "cooling_only" => Some(1),
            "heating only" | "heating_only" => Some(2),
            _ => None,
        },
        DptType::EnableHCStage => match lower.as_str() {
            "disabled" => Some(0),
            "stage a" | "stage_a" => Some(1),
            "stage b" | "stage_b" => Some(2),
            "both stages" | "both_stages" => Some(3),
            _ => None,
        },
        DptType::HVACMode => match lower.as_str() {
            "auto" => Some(0),
            "comfort" => Some(1),
            "standby" => Some(2),
            "economy" => Some(3),
            "building protection" => Some(4),
            _ => None,
        },
        DptType::HVACContrMode => match lower.as_str() {
            "auto" => Some(0),
            "heat" => Some(1),
            "morning warmup" => Some(2),
            "cool" => Some(3),
            "night purge" => Some(4),
            "precool" => Some(5),
            "off" => Some(6),
            "test" => Some(7),
            "emergency heat" => Some(8),
            "fan only" => Some(9),
            "free cool" => Some(10),
            "ice" => Some(11),
            "maximum heating mode" => Some(12),
            "economic heat/cool mode" => Some(13),
            "dehumidification" => Some(14),
            "calibration mode" => Some(15),
            "emergency cool mode" => Some(16),
            "emergency steam mode" => Some(17),
            "no demand" => Some(20),
            _ => None,
        },
        _ => None,
    }
}

/// Strip a `"Name("`/`")"` wrapper if present, otherwise return the input
/// trimmed and unwrapped (so bare `"r, g, b"` also works).
fn strip_wrapper<'a>(s: &'a str, prefix: &str) -> &'a str {
    let s = s.trim();
    s.strip_prefix(prefix)
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(s)
}

fn parse_rgb(s: &str) -> Option<DptPayload> {
    let inner = strip_wrapper(s, "RGB(");
    if let Some(hex) = inner.strip_prefix('#') {
        let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
        let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
        let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
        return Some(DptPayload::ColorRGB { r, g, b });
    }
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let [r, g, b] = parts[..] else { return None };
    Some(DptPayload::ColorRGB {
        r: r.parse().ok()?,
        g: g.parse().ok()?,
        b: b.parse().ok()?,
    })
}

fn parse_rgbw(input: &str) -> Option<DptPayload> {
    let inner = strip_wrapper(input, "RGBW(");
    if let Some(hex) = inner.strip_prefix('#') {
        let red = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
        let green = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
        let blue = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
        let white = u8::from_str_radix(hex.get(6..8)?, 16).ok()?;
        return Some(DptPayload::ColorRGBW {
            r: red,
            g: green,
            b: blue,
            w: white,
        });
    }
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let [red, green, blue, white] = parts[..] else {
        return None;
    };
    Some(DptPayload::ColorRGBW {
        r: red.parse().ok()?,
        g: green.parse().ok()?,
        b: blue.parse().ok()?,
        w: white.parse().ok()?,
    })
}

fn parse_xyy(s: &str) -> Option<DptPayload> {
    let inner = strip_wrapper(s, "XYY(");
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let [x, y, brightness] = parts[..] else {
        return None;
    };
    Some(DptPayload::ColorXYY {
        x: x.parse().ok()?,
        y: y.parse().ok()?,
        brightness: brightness.parse().ok()?,
    })
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
