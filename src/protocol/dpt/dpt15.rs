//! DPT 15.000 access data.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccessData {
    pub access_code: u32,
    pub detection_error: bool,
    pub permission: bool,
    pub read_right_to_left: bool,
    pub encrypted: bool,
    pub index: u8,
}
impl std::fmt::Debug for AccessData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessData")
            .field("access_code", &"[redacted]")
            .field("detection_error", &self.detection_error)
            .field("permission", &self.permission)
            .field("read_right_to_left", &self.read_right_to_left)
            .field("encrypted", &self.encrypted)
            .field("index", &self.index)
            .finish()
    }
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "15.000".into(),
        details: "Invalid access data".into(),
    }
    .into()
}
impl AccessData {
    pub(super) fn encode(self) -> Result<[u8; 4]> {
        if self.access_code > 999_999 || self.index > 15 {
            return Err(bad());
        }
        let mut code = self.access_code;
        let mut bytes = [0; 4];
        for byte in bytes[..3].iter_mut().rev() {
            let low = (code % 10) as u8;
            code /= 10;
            let high = (code % 10) as u8;
            code /= 10;
            *byte = high << 4 | low;
        }
        bytes[3] = u8::from(self.detection_error) << 7
            | u8::from(self.permission) << 6
            | u8::from(self.read_right_to_left) << 5
            | u8::from(self.encrypted) << 4
            | self.index;
        Ok(bytes)
    }
    pub(super) fn decode(bytes: [u8; 4]) -> Result<Self> {
        let mut code = 0u32;
        for byte in &bytes[..3] {
            let high = byte >> 4;
            let low = byte & 15;
            if high > 9 || low > 9 {
                return Err(bad());
            }
            code = code * 100 + u32::from(high) * 10 + u32::from(low);
        }
        Ok(Self {
            access_code: code,
            detection_error: bytes[3] & 128 != 0,
            permission: bytes[3] & 64 != 0,
            read_right_to_left: bytes[3] & 32 != 0,
            encrypted: bytes[3] & 16 != 0,
            index: bytes[3] & 15,
        })
    }
}
impl std::fmt::Display for AccessData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{},{},{},{}",
            self.access_code,
            self.detection_error,
            self.permission,
            self.read_right_to_left,
            self.encrypted,
            self.index
        )
    }
}
impl std::str::FromStr for AccessData {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let mut p = s.split(',').map(str::trim);
        let v = Self {
            access_code: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            detection_error: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            permission: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            read_right_to_left: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            encrypted: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            index: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
        };
        if p.next().is_some() {
            return Err(bad());
        }
        v.encode()?;
        Ok(v)
    }
}
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DPTAccessData([u8; 4]);
impl std::fmt::Debug for DPTAccessData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value().fmt(f)
    }
}
impl DPTAccessData {
    pub fn new(value: AccessData) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> AccessData {
        AccessData::decode(self.0).expect("validated access data")
    }
}
impl DptValue for DPTAccessData {
    const DPT_NUMBER: &'static str = "15.000";
    const VALUE_TYPE: &'static str = "access_data";
    const BYTE_LENGTH: usize = 4;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| bad())?;
        AccessData::decode(raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        AccessData::decode(self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTAccessData> for AccessData {
    fn resolve(self, _: DptType) -> Result<DPTAccessData> {
        DPTAccessData::new(self)
    }
}

#[derive(Clone, Copy)]
pub struct AccessDataView<'a>(pub(super) &'a [u8]);
impl std::fmt::Debug for AccessDataView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value().fmt(f)
    }
}
impl<'a> AccessDataView<'a> {
    #[must_use]
    pub fn value(&self) -> AccessData {
        AccessData::decode(self.0.try_into().expect("validated length"))
            .expect("validated access data")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
