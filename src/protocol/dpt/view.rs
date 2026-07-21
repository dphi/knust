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
pub struct StrView<'a>(pub(crate) &'a [u8]);
impl<'a> StrView<'a> {
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
            Self::Str(v) => v.as_str().to_string(),
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
        let payload = (*self).parse_payload(input.trim())?;
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
                const MAX: f64 = 670_760.96;
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
            16 => {
                if s.len() > 14 {
                    return Err(bad(format!(
                        "string too long for DPT 16 (max 14 bytes, got {})",
                        s.len()
                    )));
                }
                Ok(DptPayload::String(s.to_string()))
            }
            17 => parse_scene_number(s)
                .map(DptPayload::Scene)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a scene number (1-64)"))),
            18 => parse_scene_control(s)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as a scene control value"))),
            20 => parse_enum(s, self)
                .map(DptPayload::Enum)
                .ok_or_else(|| bad(format!("cannot parse {s:?} as an enum value"))),
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
