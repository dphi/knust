//! KNX standardized interface-object metadata.

/// Property Data Type values from KNX Standardized Identifier Tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PropertyDataType {
    Control = 0x00,
    Char = 0x01,
    UnsignedChar = 0x02,
    Int = 0x03,
    UnsignedInt = 0x04,
    KnxFloat = 0x05,
    Date = 0x06,
    Time = 0x07,
    Long = 0x08,
    UnsignedLong = 0x09,
    Float = 0x0a,
    Double = 0x0b,
    CharBlock = 0x0c,
    PollGroupSettings = 0x0d,
    ShortCharBlock = 0x0e,
    DateTime = 0x0f,
    VariableLength = 0x10,
    Generic01 = 0x11,
    Generic02 = 0x12,
    Generic03 = 0x13,
    Generic04 = 0x14,
    Generic05 = 0x15,
    Generic06 = 0x16,
    Generic07 = 0x17,
    Generic08 = 0x18,
    Generic09 = 0x19,
    Generic10 = 0x1a,
    Generic11 = 0x1b,
    Generic12 = 0x1c,
    Generic13 = 0x1d,
    Generic14 = 0x1e,
    Generic15 = 0x1f,
    Generic16 = 0x20,
    Generic17 = 0x21,
    Generic18 = 0x22,
    Generic19 = 0x23,
    Generic20 = 0x24,
    Reference = 0x25,
    Utf8 = 0x2f,
    Version = 0x30,
    AlarmInfo = 0x31,
    BinaryInformation = 0x32,
    Bitset8 = 0x33,
    Bitset16 = 0x34,
    Enum8 = 0x35,
    Scaling = 0x36,
    NeVariableLength = 0x3c,
    NeFixedLength = 0x3d,
    Function = 0x3e,
    Escape = 0x3f,
}

impl PropertyDataType {
    /// Fixed octets per property element, or `None` when the PDT does not fix it.
    #[must_use]
    pub const fn size(self) -> Option<usize> {
        match self {
            Self::Char
            | Self::UnsignedChar
            | Self::BinaryInformation
            | Self::Bitset8
            | Self::Enum8
            | Self::Scaling => Some(1),
            Self::Int | Self::UnsignedInt | Self::KnxFloat | Self::Version | Self::Bitset16 => {
                Some(2)
            }
            Self::Date | Self::Time | Self::PollGroupSettings => Some(3),
            Self::Long | Self::UnsignedLong | Self::Float => Some(4),
            Self::Double => Some(8),
            Self::AlarmInfo => Some(6),
            Self::CharBlock => Some(10),
            Self::ShortCharBlock => Some(5),
            Self::DateTime => Some(8),
            Self::Generic01 => Some(1),
            Self::Generic02 => Some(2),
            Self::Generic03 => Some(3),
            Self::Generic04 => Some(4),
            Self::Generic05 => Some(5),
            Self::Generic06 => Some(6),
            Self::Generic07 => Some(7),
            Self::Generic08 => Some(8),
            Self::Generic09 => Some(9),
            Self::Generic10 => Some(10),
            Self::Generic11 => Some(11),
            Self::Generic12 => Some(12),
            Self::Generic13 => Some(13),
            Self::Generic14 => Some(14),
            Self::Generic15 => Some(15),
            Self::Generic16 => Some(16),
            Self::Generic17 => Some(17),
            Self::Generic18 => Some(18),
            Self::Generic19 => Some(19),
            Self::Generic20 => Some(20),
            Self::Control
            | Self::VariableLength
            | Self::Reference
            | Self::Utf8
            | Self::NeVariableLength
            | Self::NeFixedLength
            | Self::Function
            | Self::Escape => None,
        }
    }
}

impl TryFrom<u8> for PropertyDataType {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0x00 => Self::Control,
            0x01 => Self::Char,
            0x02 => Self::UnsignedChar,
            0x03 => Self::Int,
            0x04 => Self::UnsignedInt,
            0x05 => Self::KnxFloat,
            0x06 => Self::Date,
            0x07 => Self::Time,
            0x08 => Self::Long,
            0x09 => Self::UnsignedLong,
            0x0a => Self::Float,
            0x0b => Self::Double,
            0x0c => Self::CharBlock,
            0x0d => Self::PollGroupSettings,
            0x0e => Self::ShortCharBlock,
            0x0f => Self::DateTime,
            0x10 => Self::VariableLength,
            0x11 => Self::Generic01,
            0x12 => Self::Generic02,
            0x13 => Self::Generic03,
            0x14 => Self::Generic04,
            0x15 => Self::Generic05,
            0x16 => Self::Generic06,
            0x17 => Self::Generic07,
            0x18 => Self::Generic08,
            0x19 => Self::Generic09,
            0x1a => Self::Generic10,
            0x1b => Self::Generic11,
            0x1c => Self::Generic12,
            0x1d => Self::Generic13,
            0x1e => Self::Generic14,
            0x1f => Self::Generic15,
            0x20 => Self::Generic16,
            0x21 => Self::Generic17,
            0x22 => Self::Generic18,
            0x23 => Self::Generic19,
            0x24 => Self::Generic20,
            0x25 => Self::Reference,
            0x2f => Self::Utf8,
            0x30 => Self::Version,
            0x31 => Self::AlarmInfo,
            0x32 => Self::BinaryInformation,
            0x33 => Self::Bitset8,
            0x34 => Self::Bitset16,
            0x35 => Self::Enum8,
            0x36 => Self::Scaling,
            0x3c => Self::NeVariableLength,
            0x3d => Self::NeFixedLength,
            0x3e => Self::Function,
            0x3f => Self::Escape,
            _ => return Err(value),
        })
    }
}
