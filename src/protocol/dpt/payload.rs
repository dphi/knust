use super::unit::Unit;
use super::view::DptView;

/// Owned decoded DPT value — for storage, serialization, or sending across threads.
#[derive(Debug, Clone, PartialEq)]
pub enum DptPayload {
    Bool(bool),
    Control {
        step: bool,
        step_code: u8,
    },
    UnsignedInt(u64),
    SignedInt(i64),
    Float(f64),
    String(String),
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
            Self::Scene(n) => format!("Scene {n}"),
            Self::SceneControl { scene, learn } => format!(
                "Scene {} ({})",
                scene,
                if *learn { "learn" } else { "activate" }
            ),
            Self::Enum(v) => format!("Enum({v})"),
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
            DptView::Str(v) => Self::String(v.as_str().to_string()),
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
