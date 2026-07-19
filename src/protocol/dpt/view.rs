//! Zero-copy view types for DPT-encoded bytes.

use super::dpt_type::DptType;

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
            Self::Time(v) => format!("{:02}:{:02}:{:02}", v.hour(), v.minute(), v.second()),
            Self::Date(v) => format!("{:04}-{:02}-{:02}", v.year(), v.month(), v.day()),
            Self::DateTime(v) => format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                v.year(),
                v.month(),
                v.day(),
                v.hour(),
                v.minute(),
                v.second()
            ),
            Self::Scene(v) => format!("Scene {}", v.scene_number()),
            Self::Enum(v) => format_enum(v.value(), dpt),
            Self::Rgb(v) => format!("RGB({}, {}, {})", v.r(), v.g(), v.b()),
            Self::Rgbw(v) => format!("RGBW({}, {}, {}, {})", v.r(), v.g(), v.b(), v.w()),
            Self::Xyy(v) => format!("XYY({:.3}, {:.3}, {})", v.x(), v.y(), v.brightness()),
        }
    }
}

fn format_bool(value: bool, dpt: DptType) -> String {
    match dpt {
        DptType::Switch => if value { "On" } else { "Off" }.to_string(),
        DptType::OpenClose | DptType::WindowDoor => {
            if value { "Open" } else { "Closed" }.to_string()
        }
        DptType::UpDown => if value { "Down" } else { "Up" }.to_string(),
        DptType::Start => if value { "Start" } else { "Stop" }.to_string(),
        DptType::Alarm => if value { "Alarm" } else { "No Alarm" }.to_string(),
        DptType::DayNight => if value { "Night" } else { "Day" }.to_string(),
        DptType::HeatCool => if value { "Heat" } else { "Cool" }.to_string(),
        DptType::Enable => if value { "Enable" } else { "Disable" }.to_string(),
        DptType::Occupancy => if value { "Occupied" } else { "Not Occupied" }.to_string(),
        _ => if value { "True" } else { "False" }.to_string(),
    }
}

/// Semantic label for a DPT 2.xxx control value bit, using the same enum
/// names as the corresponding DPT 1.xxx type.
fn format_control2(value: bool, dpt: DptType) -> &'static str {
    match dpt {
        DptType::SwitchControl => {
            if value {
                "on"
            } else {
                "off"
            }
        }
        DptType::BoolControl => {
            if value {
                "true"
            } else {
                "false"
            }
        }
        DptType::EnableControl => {
            if value {
                "enable"
            } else {
                "disable"
            }
        }
        DptType::RampControl => {
            if value {
                "ramp"
            } else {
                "no_ramp"
            }
        }
        DptType::AlarmControl => {
            if value {
                "alarm"
            } else {
                "no_alarm"
            }
        }
        DptType::BinaryValueControl => {
            if value {
                "high"
            } else {
                "low"
            }
        }
        DptType::StepControl => {
            if value {
                "increase"
            } else {
                "decrease"
            }
        }
        DptType::Direction1Control | DptType::Direction2Control => {
            if value {
                "down"
            } else {
                "up"
            }
        }
        DptType::StartControl => {
            if value {
                "start"
            } else {
                "stop"
            }
        }
        DptType::StateControl => {
            if value {
                "active"
            } else {
                "inactive"
            }
        }
        DptType::InvertControl => {
            if value {
                "inverted"
            } else {
                "not_inverted"
            }
        }
        _ => {
            if value {
                "1"
            } else {
                "0"
            }
        }
    }
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
