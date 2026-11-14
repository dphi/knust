//! DPT 230.1000 M-Bus address.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MBusAddress {
    pub manufacturer_id: u16,
    pub identification_number: u32,
    pub version: u8,
    pub medium: u8,
}
impl MBusAddress {
    pub(super) fn encode(self) -> [u8; 8] {
        let m = self.manufacturer_id.to_be_bytes();
        let i = self.identification_number.to_be_bytes();
        [
            m[0],
            m[1],
            i[0],
            i[1],
            i[2],
            i[3],
            self.version,
            self.medium,
        ]
    }
    pub(super) fn decode(b: [u8; 8]) -> Self {
        Self {
            manufacturer_id: u16::from_be_bytes([b[0], b[1]]),
            identification_number: u32::from_be_bytes([b[2], b[3], b[4], b[5]]),
            version: b[6],
            medium: b[7],
        }
    }
}
impl std::fmt::Display for MBusAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{},{}",
            self.manufacturer_id, self.identification_number, self.version, self.medium
        )
    }
}
impl std::str::FromStr for MBusAddress {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || ProtocolError::DptError {
            dpt_type: "230.1000".into(),
            details: "Expected manufacturer,identification,version,medium".into(),
        };
        let mut p = s.split(',').map(str::trim);
        let value = Self {
            manufacturer_id: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            identification_number: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            version: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            medium: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
        };
        if p.next().is_some() {
            return Err(bad().into());
        }
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTMBusAddress([u8; 8]);
impl DPTMBusAddress {
    #[must_use]
    pub fn new(value: MBusAddress) -> Self {
        Self(value.encode())
    }
    #[must_use]
    pub fn value(&self) -> MBusAddress {
        MBusAddress::decode(self.0)
    }
}
impl DptValue for DPTMBusAddress {
    const DPT_NUMBER: &'static str = "230.1000";
    const VALUE_TYPE: &'static str = "mbus_address";
    const BYTE_LENGTH: usize = 8;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(bytes.try_into().map_err(|_| {
            ProtocolError::DptError {
                dpt_type: "230.1000".into(),
                details: "Expected eight bytes".into(),
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
        (0.0, 0.0)
    }
}
impl WriteValue<DPTMBusAddress> for MBusAddress {
    fn resolve(self, _: DptType) -> Result<DPTMBusAddress> {
        Ok(DPTMBusAddress::new(self))
    }
}
