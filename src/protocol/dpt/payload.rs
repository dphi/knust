use super::unit::Unit;
use super::view::DptView;

/// Owned decoded DPT value — for storage, serialization, or sending across threads.
#[derive(Debug, Clone, PartialEq)]
pub enum DptPayload {
    Bool(bool),
    BinaryControl {
        control: bool,
        value: bool,
    },
    Control {
        step: bool,
        step_code: u8,
    },
    UnsignedInt(u64),
    SignedInt(i64),
    Float(f64),
    String(String),
    Version(super::dpt217::KnxVersion),
    DoubleNibble(super::dpt25::DoubleNibble),
    SceneInfo(super::dpt26::SceneInfo),
    CombinedInfoOnOff(super::dpt27::CombinedInfoOnOff),
    ChannelActivation24(super::dpt30::ChannelActivation24),
    MBusAddress(super::dpt230::MBusAddress),
    PrioritisedModeControl(super::dpt236::PrioritisedModeControl),
    SceneConfig(super::dpt238::SceneConfig),
    DALIDiagnostics(super::dpt238::DALIDiagnostics),
    DALIControlGearDiagnostic(super::dpt237::DALIControlGearDiagnostic),
    GeographicalLocation(super::dpt255::GeographicalLocation),
    CombinedPosition(super::dpt240::CombinedPosition),
    StatusSAB(super::dpt241::StatusSAB),
    AccessData(super::dpt15::AccessData),
    ModeNext(super::dpt206::ModeNext),
    ScalingTime(super::dpt225::ScalingTime),
    BurnerControllerStatus(super::dpt207::BurnerControllerStatus),
    LockingSignal(super::dpt207::LockingSignal),
    BoilerControllerDemand(super::dpt207::BoilerControllerDemand),
    ActuatorPositionDemand(super::dpt207::ActuatorPositionDemand),
    ActuatorPositionStatus(super::dpt207::ActuatorPositionStatus),
    LightingActuatorStatus(super::dpt207::LightingActuatorStatus),
    MeteringValue(super::dpt229::MeteringValue),
    AlarmInfo(super::dpt219::AlarmInfo),
    ColorTemperatureTransition(super::dpt249::ColorTemperatureTransition),
    ColorTemperatureControl(super::dpt_relative_color::ColorTemperatureControl),
    RelativeControlRGBW(super::dpt_relative_color::RelativeControlRGBW),
    RelativeControlXYY(super::dpt_relative_color::RelativeControlXYY),
    RelativeControlRGB(super::dpt_relative_color::RelativeControlRGB),
    XYYColorTransition(super::dpt243::XYYColorTransition),
    StatusRTSM(super::dpt_hvac_status::CombinedStatusRTSM),
    StatusHVA(super::dpt_hvac_status::CombinedStatusHVA),
    StatusRTC(super::dpt_hvac_status::CombinedStatusRTC),
    Forecast(super::dpt273::Forecast),
    Setpoints3(super::dpt_setpoints::RoomTemperatureSetpoints3),
    Setpoints4(super::dpt_setpoints::RoomTemperatureSetpoints4),
    Time {
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    },
    Date {
        day: u8,
        month: u8,
        year: u16,
    },
    DateTime {
        year: u16,
        month: u8,
        day: u8,
        day_of_week: u8,
        hour: u8,
        minute: u8,
        second: u8,
    },
    Scene(u8),
    SceneControl {
        scene: u8,
        learn: bool,
    },
    Enum(u8),
    ColorRGB {
        r: u8,
        g: u8,
        b: u8,
    },
    ColorRGBW {
        r: u8,
        g: u8,
        b: u8,
        w: u8,
    },
    ColorXYY {
        x: f32,
        y: f32,
        brightness: u8,
    },
}

impl DptPayload {
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
            Self::UnsignedInt(v) => Some(*v as f64),
            Self::SignedInt(v) => Some(*v as f64),
            Self::Float(v) => Some(*v),
            Self::Scene(v) | Self::Enum(v) => Some(f64::from(*v)),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    #[must_use]
    pub fn formatted(&self, unit: Option<Unit>) -> String {
        let unit_str = unit.map_or("", |u| u.symbol());
        match self {
            Self::Bool(true) => "On".to_string(),
            Self::Bool(false) => "Off".to_string(),
            Self::Float(v) => {
                if unit_str.is_empty() {
                    format!("{v:.1}")
                } else {
                    format!("{v:.1} {unit_str}")
                }
            }
            Self::UnsignedInt(v) => {
                if unit_str.is_empty() {
                    format!("{v}")
                } else {
                    format!("{v} {unit_str}")
                }
            }
            Self::SignedInt(v) => {
                if unit_str.is_empty() {
                    format!("{v}")
                } else {
                    format!("{v} {unit_str}")
                }
            }
            Self::String(s) => s.clone(),
            Self::Version(v) => v.to_string(),
            Self::DoubleNibble(v) => v.to_string(),
            Self::SceneInfo(v) => v.to_string(),
            Self::CombinedInfoOnOff(v) => v.to_string(),
            Self::ChannelActivation24(v) => v.to_string(),
            Self::MBusAddress(v) => v.to_string(),
            Self::PrioritisedModeControl(v) => v.to_string(),
            Self::SceneConfig(v) => v.to_string(),
            Self::DALIDiagnostics(v) => v.to_string(),
            Self::DALIControlGearDiagnostic(v) => v.to_string(),
            Self::GeographicalLocation(v) => v.to_string(),
            Self::CombinedPosition(v) => v.to_string(),
            Self::StatusSAB(v) => v.to_string(),
            Self::AccessData(v) => v.to_string(),
            Self::ModeNext(v) => v.to_string(),
            Self::ScalingTime(v) => v.to_string(),
            Self::BurnerControllerStatus(v) => v.to_string(),
            Self::LockingSignal(v) => v.to_string(),
            Self::BoilerControllerDemand(v) => v.to_string(),
            Self::ActuatorPositionDemand(v) => v.to_string(),
            Self::ActuatorPositionStatus(v) => v.to_string(),
            Self::LightingActuatorStatus(v) => v.to_string(),
            Self::MeteringValue(v) => v.to_string(),
            Self::AlarmInfo(v) => v.to_string(),
            Self::ColorTemperatureTransition(v) => v.to_string(),
            Self::ColorTemperatureControl(v) => v.to_string(),
            Self::RelativeControlRGBW(v) => v.to_string(),
            Self::RelativeControlXYY(v) => v.to_string(),
            Self::RelativeControlRGB(v) => v.to_string(),
            Self::XYYColorTransition(v) => v.to_string(),
            Self::StatusRTSM(v) => v.to_string(),
            Self::StatusHVA(v) => v.to_string(),
            Self::StatusRTC(v) => v.to_string(),
            Self::Forecast(v) => v.to_string(),
            Self::Setpoints3(v) => v.to_string(),
            Self::Setpoints4(v) => v.to_string(),
            Self::Scene(n) => format!("Scene {n}"),
            Self::SceneControl { scene, learn } => format!(
                "Scene {} ({})",
                scene,
                if *learn { "learn" } else { "activate" }
            ),
            Self::Enum(v) => format!("Enum({v})"),
            Self::BinaryControl { control, value } => format!(
                "Control({}, {})",
                if *control { "active" } else { "inactive" },
                u8::from(*value)
            ),
            Self::Control { step, step_code } => format!(
                "Control({}, {})",
                if *step { "increase" } else { "decrease" },
                step_code
            ),
            Self::Time {
                day,
                hour,
                minute,
                second,
            } => format!("{hour:02}:{minute:02}:{second:02} (day {day})"),
            Self::Date { day, month, year } => format!("{year:04}-{month:02}-{day:02}"),
            Self::DateTime {
                year,
                month,
                day,
                day_of_week: _,
                hour,
                minute,
                second,
            } => format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"),
            Self::ColorRGB { r, g, b } => format!("RGB({r}, {g}, {b})"),
            Self::ColorRGBW { r, g, b, w } => format!("RGBW({r}, {g}, {b}, {w})"),
            Self::ColorXYY { x, y, brightness } => format!("XYY({x:.3}, {y:.3}, {brightness})"),
        }
    }
}

impl<'a> From<DptView<'a>> for DptPayload {
    fn from(view: DptView<'a>) -> Self {
        match view {
            DptView::Bool(v) => Self::Bool(v.value()),
            DptView::Control2(v) => Self::BinaryControl {
                control: v.control(),
                value: v.value(),
            },
            DptView::Control(v) => Self::Control {
                step: v.step(),
                step_code: v.step_code(),
            },
            DptView::U8(v) => Self::UnsignedInt(u64::from(v.value())),
            DptView::I8(v) => Self::SignedInt(i64::from(v.value())),
            DptView::U16(v) => Self::UnsignedInt(u64::from(v.value())),
            DptView::I16(v) => Self::SignedInt(i64::from(v.value())),
            DptView::Float2Byte(v) => Self::Float(v.as_f64()),
            DptView::U32(v) => Self::UnsignedInt(u64::from(v.value())),
            DptView::I32(v) => Self::SignedInt(i64::from(v.value())),
            DptView::Float4Byte(v) => Self::Float(v.as_f64()),
            DptView::I64(v) => Self::SignedInt(v.value()),
            DptView::Str(v) => Self::String(v.as_string()),
            DptView::Character(v) => Self::String(v.as_string()),
            DptView::Version(v) => Self::Version(v.value()),
            DptView::DoubleNibble(v) => Self::DoubleNibble(v.value()),
            DptView::SceneInfo(v) => Self::SceneInfo(v.value()),
            DptView::CombinedInfoOnOff(v) => Self::CombinedInfoOnOff(v.value()),
            DptView::ChannelActivation24(v) => Self::ChannelActivation24(v.value()),
            DptView::MBusAddress(v) => Self::MBusAddress(v.value()),
            DptView::PrioritisedModeControl(v) => Self::PrioritisedModeControl(v.value()),
            DptView::SceneConfig(v) => Self::SceneConfig(v.value()),
            DptView::DALIDiagnostics(v) => Self::DALIDiagnostics(v.value()),
            DptView::DALIControlGearDiagnostic(v) => Self::DALIControlGearDiagnostic(v.value()),
            DptView::GeographicalLocation(v) => Self::GeographicalLocation(v.value()),
            DptView::CombinedPosition(v) => Self::CombinedPosition(v.value()),
            DptView::StatusSAB(v) => Self::StatusSAB(v.value()),
            DptView::AccessData(v) => Self::AccessData(v.value()),
            DptView::ModeNext(v) => Self::ModeNext(v.value()),
            DptView::ScalingTime(v) => Self::ScalingTime(v.value()),
            DptView::Actuator(v) => v.value(),
            DptView::MeteringValue(v) => Self::MeteringValue(v.value()),
            DptView::AlarmInfo(v) => Self::AlarmInfo(v.value()),
            DptView::ColorTemperatureTransition(v) => Self::ColorTemperatureTransition(v.value()),
            DptView::RelativeColor(v) => v.value(),
            DptView::XYYColorTransition(v) => Self::XYYColorTransition(v.value()),
            DptView::StatusRTSM(v) => Self::StatusRTSM(v.value()),
            DptView::StatusHVA(v) => Self::StatusHVA(v.value()),
            DptView::StatusRTC(v) => Self::StatusRTC(v.value()),
            DptView::Forecast(v) => Self::Forecast(v.value()),
            DptView::Setpoints3(v) => Self::Setpoints3(v.value()),
            DptView::Setpoints4(v) => Self::Setpoints4(v.value()),
            DptView::Code(v) => Self::String(v.value()),
            DptView::Time(v) => Self::Time {
                day: v.day(),
                hour: v.hour(),
                minute: v.minute(),
                second: v.second(),
            },
            DptView::Date(v) => Self::Date {
                day: v.day(),
                month: v.month(),
                year: v.year(),
            },
            DptView::DateTime(v) => Self::DateTime {
                year: v.year(),
                month: v.month(),
                day: v.day(),
                day_of_week: v.day_of_week(),
                hour: v.hour(),
                minute: v.minute(),
                second: v.second(),
            },
            DptView::Scene(v) => Self::Scene(v.scene_number()),
            DptView::Enum(v) => Self::Enum(v.value()),
            DptView::Rgb(v) => Self::ColorRGB {
                r: v.r(),
                g: v.g(),
                b: v.b(),
            },
            DptView::Rgbw(v) => Self::ColorRGBW {
                r: v.r(),
                g: v.g(),
                b: v.b(),
                w: v.w(),
            },
            DptView::Xyy(v) => Self::ColorXYY {
                x: v.x(),
                y: v.y(),
                brightness: v.brightness(),
            },
        }
    }
}

impl std::fmt::Display for DptPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.formatted(None))
    }
}
