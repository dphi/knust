//! DPT 20.xxx - HVAC values, including the legacy Eberle status octet.

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// KNX system error classes used by DPT 20.011.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ErrorClassSystem {
    NoFault = 0,
    GeneralDeviceFault = 1,
    CommunicationFault = 2,
    ConfigurationFault = 3,
    HardwareFault = 4,
    SoftwareFault = 5,
    InsufficientNonVolatileMemory = 6,
    InsufficientVolatileMemory = 7,
    MemoryAllocationSizeZero = 8,
    CrcError = 9,
    WatchdogReset = 10,
    InvalidOpcode = 11,
    GeneralProtectionFault = 12,
    MaximalTableLengthExceeded = 13,
    UndefinedLoadCommand = 14,
    GroupAddressTableNotSorted = 15,
    InvalidConnectionNumber = 16,
    InvalidGroupObjectNumber = 17,
    GroupObjectTypeTooLong = 18,
}

impl ErrorClassSystem {
    pub const ALL: [Self; 19] = [
        Self::NoFault,
        Self::GeneralDeviceFault,
        Self::CommunicationFault,
        Self::ConfigurationFault,
        Self::HardwareFault,
        Self::SoftwareFault,
        Self::InsufficientNonVolatileMemory,
        Self::InsufficientVolatileMemory,
        Self::MemoryAllocationSizeZero,
        Self::CrcError,
        Self::WatchdogReset,
        Self::InvalidOpcode,
        Self::GeneralProtectionFault,
        Self::MaximalTableLengthExceeded,
        Self::UndefinedLoadCommand,
        Self::GroupAddressTableNotSorted,
        Self::InvalidConnectionNumber,
        Self::InvalidGroupObjectNumber,
        Self::GroupObjectTypeTooLong,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoFault => "No Fault",
            Self::GeneralDeviceFault => "General Device Fault",
            Self::CommunicationFault => "Communication Fault",
            Self::ConfigurationFault => "Configuration Fault",
            Self::HardwareFault => "Hardware Fault",
            Self::SoftwareFault => "Software Fault",
            Self::InsufficientNonVolatileMemory => "Insufficient Non-Volatile Memory",
            Self::InsufficientVolatileMemory => "Insufficient Volatile Memory",
            Self::MemoryAllocationSizeZero => "Memory Allocation Size Zero",
            Self::CrcError => "CRC Error",
            Self::WatchdogReset => "Watchdog Reset",
            Self::InvalidOpcode => "Invalid Opcode",
            Self::GeneralProtectionFault => "General Protection Fault",
            Self::MaximalTableLengthExceeded => "Maximal Table Length Exceeded",
            Self::UndefinedLoadCommand => "Undefined Load Command",
            Self::GroupAddressTableNotSorted => "Group Address Table Not Sorted",
            Self::InvalidConnectionNumber => "Invalid Connection Number",
            Self::InvalidGroupObjectNumber => "Invalid Group Object Number",
            Self::GroupObjectTypeTooLong => "Group Object Type Too Long",
        }
    }
}

impl TryFrom<u8> for ErrorClassSystem {
    type Error = crate::error::KnxError;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        Self::ALL.get(usize::from(value)).copied().ok_or_else(|| {
            crate::error::ProtocolError::DptError {
                dpt_type: "20.011".into(),
                details: format!("Reserved system error class {value}"),
            }
            .into()
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DPTErrorClassSystem([u8; 1]);

impl DPTErrorClassSystem {
    #[must_use]
    pub const fn new(value: ErrorClassSystem) -> Self {
        Self([value as u8])
    }

    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(self) -> ErrorClassSystem {
        ErrorClassSystem::try_from(self.0[0]).expect("validated DPT 20.011 value")
    }
}

impl DptValue for DPTErrorClassSystem {
    const DPT_NUMBER: &'static str = "20.011";
    const VALUE_TYPE: &'static str = "error_class_system";
    const BYTE_LENGTH: usize = 1;

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let [value] = bytes else {
            return Err(crate::error::ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.into(),
                details: format!("Expected one byte, got {}", bytes.len()),
            }
            .into());
        };
        ErrorClassSystem::try_from(*value)?;
        Ok(Self([*value]))
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn validate(&self) -> Result<()> {
        ErrorClassSystem::try_from(self.0[0]).map(|_| ())
    }

    fn value_range() -> (f64, f64) {
        (0.0, 18.0)
    }
}

impl super::WriteValue<DPTErrorClassSystem> for ErrorClassSystem {
    fn resolve(self, _: super::DptType) -> Result<DPTErrorClassSystem> {
        Ok(DPTErrorClassSystem::new(self))
    }
}

/// DPT 20.102 - HVAC Mode
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HVACMode {
    value: u8,
    data: [u8; 1],
}

impl HVACMode {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `value > 4`.
    pub fn new(value: u8) -> Result<Self> {
        if value > 4 {
            return Err(ProtocolError::DptError {
                dpt_type: "20.102".to_string(),
                details: format!("HVAC mode {value} out of range [0-4]"),
            }
            .into());
        }

        Ok(Self {
            value,
            data: [value],
        })
    }

    #[must_use]
    pub fn value(&self) -> u8 {
        self.value
    }
}

impl DptValue for HVACMode {
    const DPT_NUMBER: &'static str = "20.102";
    const VALUE_TYPE: &'static str = "hvac_mode";
    const BYTE_LENGTH: usize = 1;

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::BYTE_LENGTH {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!(
                    "Invalid length: expected {}, got {}",
                    Self::BYTE_LENGTH,
                    bytes.len()
                ),
            }
            .into());
        }
        Self::new(bytes[0])
    }

    fn validate(&self) -> Result<()> {
        // HVAC modes: 0=Auto, 1=Comfort, 2=Standby, 3=Economy, 4=Building Protection
        if self.value > 4 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("HVAC mode {} out of range [0-4]", self.value),
            }
            .into());
        }
        Ok(())
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::decode(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn value_range() -> (f64, f64) {
        (0.0, 4.0)
    }
}

/// DPT 20.105 - HVAC Controller Mode
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HVACControllerMode {
    value: u8,
    data: [u8; 1],
}

impl HVACControllerMode {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `value > 17` and `value != 20`.
    pub fn new(value: u8) -> Result<Self> {
        if value > 17 && value != 20 {
            return Err(ProtocolError::DptError {
                dpt_type: "20.105".to_string(),
                details: format!(
                    "HVAC controller mode {value} out of Python-compatible range [0-17, 20]"
                ),
            }
            .into());
        }

        Ok(Self {
            value,
            data: [value],
        })
    }

    #[must_use]
    pub fn value(&self) -> u8 {
        self.value
    }
}

impl DptValue for HVACControllerMode {
    const DPT_NUMBER: &'static str = "20.105";
    const VALUE_TYPE: &'static str = "hvac_controller_mode";
    const BYTE_LENGTH: usize = 1;

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::BYTE_LENGTH {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!(
                    "Invalid length: expected {}, got {}",
                    Self::BYTE_LENGTH,
                    bytes.len()
                ),
            }
            .into());
        }
        Self::new(bytes[0])
    }

    fn validate(&self) -> Result<()> {
        if self.value > 17 && self.value != 20 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!(
                    "HVAC controller mode {} out of Python-compatible range [0-17, 20]",
                    self.value
                ),
            }
            .into());
        }
        Ok(())
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::decode(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn value_range() -> (f64, f64) {
        (0.0, 20.0)
    }
}

/// Legacy, non-standard Eberle HVAC status octet (DPT 20.60102).
///
/// KNX DPT 21.107 and 22.103 replace this format. It remains supported for
/// decoding installations that still use the Eberle status octet.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HVACStatus {
    pub mode: HVACMode,
    pub dew_point: bool,
    /// The wire bit is set for heating and clear for cooling.
    pub heat_cool: bool,
    pub inactive: bool,
    pub frost_alarm: bool,
}

/// DPT 20.60102 - Legacy HVAC status byte.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTHVACStatus([u8; 1]);

impl DPTHVACStatus {
    #[must_use]
    pub fn new(value: HVACStatus) -> Self {
        let mode_bit = match value.mode.value() {
            1 => 0x01,
            2 => 0x02,
            3 => 0x04,
            4 => 0x08,
            _ => 0,
        };
        Self([mode_bit
            | (u8::from(value.dew_point) << 4)
            | (u8::from(value.heat_cool) << 5)
            | (u8::from(value.inactive) << 6)
            | (u8::from(value.frost_alarm) << 7)])
    }

    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> HVACStatus {
        let raw = self.0[0];
        let mode = if raw & 0x01 != 0 {
            1
        } else if raw & 0x02 != 0 {
            2
        } else if raw & 0x04 != 0 {
            3
        } else if raw & 0x08 != 0 {
            4
        } else {
            0
        };
        HVACStatus {
            mode: HVACMode::new(mode).expect("decoded HVAC mode is in range"),
            dew_point: raw & 0x10 != 0,
            heat_cool: raw & 0x20 != 0,
            inactive: raw & 0x40 != 0,
            frost_alarm: raw & 0x80 != 0,
        }
    }
}

impl DptValue for DPTHVACStatus {
    const DPT_NUMBER: &'static str = "20.60102";
    const VALUE_TYPE: &'static str = "hvac_status";
    const BYTE_LENGTH: usize = 1;

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(bytes.try_into().map_err(|_| {
            ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.into(),
                details: format!("Invalid length: expected 1, got {}", bytes.len()),
            }
        })?))
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn value_range() -> (f64, f64) {
        (0.0, 255.0)
    }
}

impl super::WriteValue<DPTHVACStatus> for HVACStatus {
    fn resolve(self, _: super::DptType) -> Result<DPTHVACStatus> {
        Ok(DPTHVACStatus::new(self))
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for HVACMode {
    type InnerType = u8;
    fn new(value: u8) -> Self {
        HVACMode::new(value).unwrap()
    }
    fn into_inner(self) -> u8 {
        self.value
    }
}

impl DptInnerType for HVACControllerMode {
    type InnerType = u8;
    fn new(value: u8) -> Self {
        HVACControllerMode::new(value).unwrap()
    }
    fn into_inner(self) -> u8 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::dpt::{DptPayload, DptRegistry, DptType};

    #[test]
    fn legacy_hvac_status_decodes_and_encodes_all_octets() {
        let dpt = DptType::from_number(20, 60_102).unwrap();

        for raw in u8::MIN..=u8::MAX {
            let decoded = DPTHVACStatus::from_bytes(&[raw]).unwrap();
            let value = decoded.value();
            let mode_bit = if raw & 0x01 != 0 {
                0x01
            } else if raw & 0x02 != 0 {
                0x02
            } else if raw & 0x04 != 0 {
                0x04
            } else if raw & 0x08 != 0 {
                0x08
            } else {
                0
            };
            assert_eq!(
                DPTHVACStatus::new(value).as_bytes(),
                &[raw & 0xf0 | mode_bit]
            );
            assert_eq!(dpt.parse(&raw.to_string()).unwrap(), [raw]);
            assert_eq!(
                dpt.encode(&DptPayload::UnsignedInt(u64::from(raw)))
                    .unwrap(),
                [raw]
            );
        }
    }

    #[test]
    fn error_class_system_is_typed_registered_and_validated() {
        let dpt = DptType::from_number(20, 11).unwrap();
        assert_eq!(dpt, DptType::ErrorClassSystem);
        let registry = DptRegistry::new();
        assert_eq!(
            registry.metadata("20.011").unwrap().value_type,
            "error_class_system"
        );

        for (raw, expected) in ErrorClassSystem::ALL.into_iter().enumerate() {
            let decoded = DPTErrorClassSystem::from_bytes(&[raw as u8]).unwrap();
            assert_eq!(decoded.value(), expected);
            assert_eq!(decoded.as_bytes(), &[raw as u8]);
            let bytes = [raw as u8];
            let view = dpt.decode_ref(&bytes).unwrap();
            assert_eq!(view.formatted(dpt), expected.label());
        }

        assert_eq!(dpt.parse("General Device Fault").unwrap(), [1]);
        assert_eq!(dpt.parse("group_address_table_not_sorted").unwrap(), [15]);
        assert!(DPTErrorClassSystem::from_bytes(&[]).is_err());
        assert!(DPTErrorClassSystem::from_bytes(&[19]).is_err());
        assert!(dpt.parse("19").is_err());
        assert!(dpt.parse("Not an error class").is_err());

        let decoded = registry.decode("20.011", &[18]).unwrap();
        assert_eq!(
            decoded.downcast::<DPTErrorClassSystem>().unwrap().value(),
            ErrorClassSystem::GroupObjectTypeTooLong
        );
    }

    #[test]
    fn legacy_hvac_status_is_registered_and_rejects_bad_lengths() {
        let registry = DptRegistry::new();
        let metadata = registry.metadata("20.60102").unwrap();
        assert_eq!(metadata.value_type, "hvac_status");

        let decoded = registry.decode("20.60102", &[0xF1]).unwrap();
        assert_eq!(
            decoded
                .downcast::<DPTHVACStatus>()
                .unwrap()
                .value()
                .mode
                .value(),
            1
        );

        assert!(DPTHVACStatus::from_bytes(&[]).is_err());
        assert!(DPTHVACStatus::from_bytes(&[0, 1]).is_err());
        assert!(
            DptType::from_number(20, 60_102)
                .unwrap()
                .parse("256")
                .is_err()
        );
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(DPTHVACMode, 20, 102, HVACMode, "hvac_mode", None, None);
dpt_alias!(
    DPTHVACContrMode,
    20,
    105,
    HVACControllerMode,
    "hvac_controller_mode",
    None,
    None
);
