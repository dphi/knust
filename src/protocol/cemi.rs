//! CEMI (Common External Message Interface) frame handling.
//!
//! This module provides zero-copy parsing and serialization of CEMI frames
//! according to the KNX specification. CEMI frames are the standard format
//! for KNX telegrams transmitted over KNX/IP networks.

use crate::error::{ProtocolError, Result};
use crate::log_protocol;
use crate::logging::{Component, LogLevel, Timer};
use crate::protocol::address::{Address, GroupAddress, IndividualAddress};

/// CEMI frame structure with all required fields
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemiFrame {
    /// Message code indicating frame type
    pub message_code: MessageCode,

    /// Additional information fields
    pub additional_info: Vec<AdditionalInfo>,

    /// Control field with frame properties
    pub control_field: ControlField,

    /// Extended control field (for extended frames)
    pub extended_control_field: Option<ExtendedControlField>,

    /// Source individual address
    pub source_addr: IndividualAddress,

    /// Destination address (group or individual)
    pub dest_addr: Address,

    /// Data length (NPDU length)
    pub data_length: u8,

    /// Transport Protocol Control Information
    pub tpci: u8,

    /// Application Protocol Control Information / Data
    pub apci_data: Vec<u8>,
}

impl CemiFrame {
    /// Parse a CEMI frame from raw bytes using zero-copy approach
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is truncated, has an
    /// unsupported message code, or has an inconsistent additional-info or
    /// APCI/data length; returns [`ProtocolError::InvalidAddress`] if a
    /// source or destination address field is malformed.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let timer = Timer::start(Component::Protocol, "cemi_parse");
        log_protocol!(LogLevel::Debug, "Parsing CEMI frame ({} bytes)", data.len());

        // Log complete raw data at trace level
        if !data.is_empty() {
            let hex_data = data
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            log_protocol!(LogLevel::Trace, "Raw CEMI data: {}", hex_data);
        }

        if data.is_empty() {
            log_protocol!(LogLevel::Error, "Empty CEMI frame received");
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: "Empty CEMI frame".to_string(),
            }
            .into());
        }

        if data.len() < 2 {
            log_protocol!(
                LogLevel::Error,
                "CEMI frame too short: {} bytes (minimum 2)",
                data.len()
            );
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: "CEMI frame too short (minimum 2 bytes)".to_string(),
            }
            .into());
        }

        let message_code = MessageCode::from_u8(data[0])?;
        let additional_info_len = data[1] as usize;

        log_protocol!(
            LogLevel::Trace,
            "CEMI message code: {:?}, additional info length: {}",
            message_code,
            additional_info_len
        );

        // Check if we have enough data for additional info
        if data.len() < 2 + additional_info_len {
            let error_msg = format!(
                "Not enough data for additional info (need {}, have {})",
                additional_info_len,
                data.len() - 2
            );
            log_protocol!(LogLevel::Error, "{}", error_msg);
            return Err(ProtocolError::ParseError {
                offset: 1,
                reason: error_msg,
            }
            .into());
        }

        // Parse additional info fields
        let mut additional_info = Vec::new();
        let mut offset = 2;
        let additional_info_end = offset + additional_info_len;

        if additional_info_len > 0 {
            log_protocol!(
                LogLevel::Trace,
                "Parsing {} bytes of additional info",
                additional_info_len
            );
        }

        while offset < additional_info_end {
            if offset + 1 >= data.len() {
                log_protocol!(
                    LogLevel::Error,
                    "Incomplete additional info field at offset {}",
                    offset
                );
                return Err(ProtocolError::ParseError {
                    offset,
                    reason: "Incomplete additional info field".to_string(),
                }
                .into());
            }

            let info_type = data[offset];
            let info_len = data[offset + 1] as usize;

            if offset + 2 + info_len > additional_info_end {
                log_protocol!(
                    LogLevel::Error,
                    "Additional info field extends beyond declared length"
                );
                return Err(ProtocolError::ParseError {
                    offset: offset + 1,
                    reason: "Additional info field extends beyond declared length".to_string(),
                }
                .into());
            }

            let info_data = data[offset + 2..offset + 2 + info_len].to_vec();
            additional_info.push(AdditionalInfo {
                info_type,
                data: info_data,
            });

            log_protocol!(
                LogLevel::Trace,
                "Parsed additional info: type=0x{:02x}, length={}",
                info_type,
                info_len
            );

            offset += 2 + info_len;
        }

        // Now parse the main CEMI frame
        // We need at least: control field (2) + source (2) + dest (2) + data_length (1) = 7 bytes minimum
        if data.len() < offset + 7 {
            return Err(ProtocolError::ParseError {
                offset,
                reason: "Not enough data for CEMI frame header".to_string(),
            }
            .into());
        }

        // Parse 2-byte control field (flags) as per KNX specification
        let flags = u16::from_be_bytes([data[offset], data[offset + 1]]);
        log_protocol!(
            LogLevel::Trace,
            "CEMI control flags: 0x{:04X} (bytes: {:02X} {:02X})",
            flags,
            data[offset],
            data[offset + 1]
        );
        let mut control_field = ControlField::from_flags(flags);
        offset += 2;

        // Parse extended control field if this is an extended frame
        let extended_control_field = if control_field.frame_type == FrameType::Extended {
            if offset >= data.len() {
                return Err(ProtocolError::ParseError {
                    offset,
                    reason: "Missing extended control field".to_string(),
                }
                .into());
            }
            let ext_ctrl = ExtendedControlField::from_u8(data[offset]);
            offset += 1;

            // Update the control field with information from extended control field
            control_field.destination_address_type = ext_ctrl.destination_address_type;
            control_field.hop_count = ext_ctrl.hop_count;

            Some(ext_ctrl)
        } else {
            None
        };

        // Parse source address
        if offset + 1 >= data.len() {
            return Err(ProtocolError::ParseError {
                offset,
                reason: "Missing source address".to_string(),
            }
            .into());
        }
        let source_raw = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let source_addr = IndividualAddress::from_raw(source_raw);
        log_protocol!(
            LogLevel::Trace,
            "CEMI source address: {} (raw: 0x{:04X})",
            source_addr,
            source_raw
        );
        offset += 2;

        // Parse destination address
        if offset + 1 >= data.len() {
            return Err(ProtocolError::ParseError {
                offset,
                reason: "Missing destination address".to_string(),
            }
            .into());
        }
        let dest_raw = u16::from_be_bytes([data[offset], data[offset + 1]]);

        // Use the address type from the control field flags
        let dest_addr = if control_field.destination_address_type == AddressType::Group {
            let group_addr = GroupAddress::try_from_raw(dest_raw).map_err(|e| {
                ProtocolError::InvalidAddress {
                    address: format!("0x{dest_raw:04X}"),
                    reason: e.to_string(),
                }
            })?;
            log_protocol!(
                LogLevel::Trace,
                "CEMI destination address: {} (raw: 0x{:04X}, type: Group)",
                group_addr,
                dest_raw
            );
            Address::Group(group_addr)
        } else {
            let individual_addr = IndividualAddress::from_raw(dest_raw);
            log_protocol!(
                LogLevel::Trace,
                "CEMI destination address: {} (raw: 0x{:04X}, type: Individual)",
                individual_addr,
                dest_raw
            );
            Address::Individual(individual_addr)
        };
        offset += 2;

        // Parse data length
        if offset >= data.len() {
            log_protocol!(
                LogLevel::Error,
                "Missing data length field at offset {}",
                offset
            );
            return Err(ProtocolError::ParseError {
                offset,
                reason: "Missing data length field".to_string(),
            }
            .into());
        }
        let data_length = data[offset];
        offset += 1;

        log_protocol!(LogLevel::Trace, "CEMI data length: {}", data_length);

        // Validate data length. The CEMI NPDU length counts APCI/data bytes;
        // the leading TPCI octet is present when data_length is non-zero.
        let tpdu_length = if data_length == 0 {
            0
        } else {
            data_length as usize + 1
        };
        if data.len() < offset + tpdu_length {
            let error_msg = format!(
                "Not enough data for payload (need {}, have {})",
                tpdu_length,
                data.len() - offset
            );
            log_protocol!(LogLevel::Error, "{}", error_msg);
            return Err(ProtocolError::ParseError {
                offset,
                reason: error_msg,
            }
            .into());
        }

        // Parse TPCI and APCI/Data
        let mut tpci = 0u8;
        let mut apci_data = Vec::new();

        if data_length > 0 {
            tpci = data[offset];
            offset += 1;
            apci_data = data[offset..offset + data_length as usize].to_vec();
        }

        let frame = CemiFrame {
            message_code,
            additional_info,
            control_field,
            extended_control_field,
            source_addr,
            dest_addr,
            data_length,
            tpci,
            apci_data,
        };

        log::debug!(target: "protocol", "CEMI parsed: code=0x{:02x}, src={}, dst={}, payload={} bytes",
            message_code.to_u8(), source_addr,
            match dest_addr { Address::Group(a) => a.to_string(), Address::Individual(a) => a.to_string() },
            data_length as usize);

        timer.finish_with_message(&format!(
            "CEMI frame parsed: {} -> {}",
            source_addr,
            match dest_addr {
                Address::Group(addr) => addr.to_string(),
                Address::Individual(addr) => addr.to_string(),
            }
        ));

        Ok(frame)
    }

    /// Serialize the CEMI frame to bytes
    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let timer = Timer::start(Component::Protocol, "cemi_serialize");
        log_protocol!(
            LogLevel::Debug,
            "Serializing CEMI frame: {} -> {}",
            self.source_addr,
            match self.dest_addr {
                Address::Group(addr) => addr.to_string(),
                Address::Individual(addr) => addr.to_string(),
            }
        );

        let mut result = Vec::new();

        // Message code
        result.push(self.message_code.to_u8());

        // Additional info length and data
        let additional_info_data: Vec<u8> = self
            .additional_info
            .iter()
            .flat_map(|info| {
                let mut info_bytes = vec![info.info_type, info.data.len() as u8];
                info_bytes.extend_from_slice(&info.data);
                info_bytes
            })
            .collect();

        result.push(additional_info_data.len() as u8);
        result.extend_from_slice(&additional_info_data);

        log_protocol!(
            LogLevel::Trace,
            "CEMI additional info: {} bytes",
            additional_info_data.len()
        );

        // Control field (2 bytes)
        let flags = self.control_field.to_flags();
        result.extend_from_slice(&flags.to_be_bytes());

        // Extended control field (if present)
        if let Some(ext_ctrl) = &self.extended_control_field {
            result.push(ext_ctrl.to_u8());
            log_protocol!(LogLevel::Trace, "CEMI extended control field included");
        }

        // Source address
        result.extend_from_slice(&self.source_addr.raw().to_be_bytes());

        // Destination address
        match self.dest_addr {
            Address::Group(addr) => result.extend_from_slice(&addr.raw().to_be_bytes()),
            Address::Individual(addr) => result.extend_from_slice(&addr.raw().to_be_bytes()),
        }

        // Data length
        result.push(self.data_length);

        // TPCI and APCI/Data
        if self.data_length > 0 {
            result.push(self.tpci);
            result.extend_from_slice(&self.apci_data);
        }

        log_protocol!(
            LogLevel::Debug,
            "CEMI frame serialized: {} bytes",
            result.len()
        );
        timer.finish_with_message(&format!("CEMI frame serialized ({} bytes)", result.len()));

        result
    }

    /// Create a new CEMI frame with minimal required fields
    #[must_use]
    pub fn new(
        message_code: MessageCode,
        source_addr: IndividualAddress,
        dest_addr: Address,
        data: Vec<u8>,
    ) -> Self {
        let destination_address_type = match dest_addr {
            Address::Group(_) => AddressType::Group,
            Address::Individual(_) => AddressType::Individual,
        };

        // Use standard frame format with proper flags
        let control_field = ControlField {
            frame_type: FrameType::Standard,
            repeat: false,
            system_broadcast: false,
            priority: Priority::Normal,
            ack_request: false,
            confirm: false,
            destination_address_type,
            hop_count: 6,
        };

        let extended_control_field = None;

        let data_length = data.len() as u8;
        let (tpci, apci_data) = if data.is_empty() {
            (0, Vec::new())
        } else {
            (0x00, data) // Default TPCI for data frames
        };

        CemiFrame {
            message_code,
            additional_info: Vec::new(),
            control_field,
            extended_control_field,
            source_addr,
            dest_addr,
            data_length,
            tpci,
            apci_data,
        }
    }
}

/// CEMI message codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageCode {
    /// `L_Data.req` - Data request
    LDataReq,

    /// `L_Data.con` - Data confirmation
    LDataCon,

    /// `L_Data.ind` - Data indication
    LDataInd,

    /// `L_Busmon.ind` - Bus monitor indication
    LBusmonInd,

    /// `L_Raw.req` - Raw request
    LRawReq,

    /// `L_Raw.ind` - Raw indication
    LRawInd,

    /// `L_Raw.con` - Raw confirmation
    LRawCon,

    /// `M_PropRead.req` - Property read request
    MPropReadReq,

    /// `M_PropRead.con` - Property read confirmation
    MPropReadCon,

    /// `M_PropWrite.req` - Property write request
    MPropWriteReq,

    /// `M_PropWrite.con` - Property write confirmation
    MPropWriteCon,
}

impl MessageCode {
    fn from_u8(value: u8) -> Result<Self> {
        match value {
            0x11 => Ok(MessageCode::LDataReq),
            0x2E => Ok(MessageCode::LDataCon),
            0x29 => Ok(MessageCode::LDataInd),
            0x2B => Ok(MessageCode::LBusmonInd),
            0x10 => Ok(MessageCode::LRawReq),
            0x2D => Ok(MessageCode::LRawInd),
            0x2F => Ok(MessageCode::LRawCon),
            0xFC => Ok(MessageCode::MPropReadReq),
            0xFB => Ok(MessageCode::MPropReadCon),
            0xF6 => Ok(MessageCode::MPropWriteReq),
            0xF5 => Ok(MessageCode::MPropWriteCon),
            _ => Err(ProtocolError::CemiError {
                message: format!("Unknown message code: 0x{value:02X}"),
            }
            .into()),
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            MessageCode::LDataReq => 0x11,
            MessageCode::LDataCon => 0x2E,
            MessageCode::LDataInd => 0x29,
            MessageCode::LBusmonInd => 0x2B,
            MessageCode::LRawReq => 0x10,
            MessageCode::LRawInd => 0x2D,
            MessageCode::LRawCon => 0x2F,
            MessageCode::MPropReadReq => 0xFC,
            MessageCode::MPropReadCon => 0xFB,
            MessageCode::MPropWriteReq => 0xF6,
            MessageCode::MPropWriteCon => 0xF5,
        }
    }
}

/// Frame type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Extended frame (0)
    Extended,
    /// Standard frame (1)
    Standard,
}

/// Address type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressType {
    /// Individual address (0)
    Individual,
    /// Group address (1)
    Group,
}

/// Priority enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// System priority (0)
    System,
    /// Normal priority (1)
    Normal,
    /// Urgent priority (2)
    Urgent,
    /// Low priority (3)
    Low,
}

/// CEMI control field
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlField {
    /// Frame type (0 = extended, 1 = standard)
    pub frame_type: FrameType,

    /// Repeat flag
    pub repeat: bool,

    /// System broadcast
    pub system_broadcast: bool,

    /// Priority (0 = system, 1 = normal, 2 = urgent, 3 = low)
    pub priority: Priority,

    /// Acknowledge request
    pub ack_request: bool,

    /// Confirm flag
    pub confirm: bool,

    /// Destination address type (0 = individual, 1 = group)
    pub destination_address_type: AddressType,

    /// Hop count (0-7)
    pub hop_count: u8,
}

impl ControlField {
    #[cfg(test)]
    fn from_u8(value: u8) -> Self {
        let frame_type = if (value & 0x80) != 0 {
            FrameType::Standard
        } else {
            FrameType::Extended
        };

        let repeat = (value & 0x20) != 0;
        let system_broadcast = (value & 0x10) != 0;

        let priority = match (value >> 2) & 0x03 {
            0 => Priority::System,
            1 => Priority::Normal,
            2 => Priority::Urgent,
            3 => Priority::Low,
            _ => unreachable!(),
        };

        let ack_request = (value & 0x02) != 0;
        let confirm = (value & 0x01) != 0;

        // For standard frames, we need to determine destination address type from context
        // For extended frames, it will be overridden from the extended control field
        ControlField {
            frame_type,
            repeat,
            system_broadcast,
            priority,
            ack_request,
            confirm,
            destination_address_type: AddressType::Group, // Default, will be updated
            hop_count: 6,                                 // Default hop count
        }
    }

    fn from_flags(flags: u16) -> Self {
        // Parse the 2-byte CEMI control field according to KNX specification
        // Byte 1 (high byte): Frame type, repeat, broadcast, priority, ack, confirm
        // Byte 0 (low byte): destination address type, hop count, extended frame format

        let high_byte = (flags >> 8) as u8;
        let low_byte = flags as u8;

        let frame_type = if (high_byte & 0x80) != 0 {
            FrameType::Standard
        } else {
            FrameType::Extended
        };

        let repeat = (high_byte & 0x20) == 0; // Note: 0 = repeat, 1 = do not repeat
        let system_broadcast = (high_byte & 0x10) == 0; // Note: 0 = system broadcast, 1 = broadcast

        let priority = match (high_byte >> 2) & 0x03 {
            0 => Priority::System,
            1 => Priority::Normal,
            2 => Priority::Urgent,
            3 => Priority::Low,
            _ => unreachable!(),
        };

        let ack_request = (high_byte & 0x02) != 0;
        let confirm = (high_byte & 0x01) != 0;

        // Parse destination address type from low byte
        let destination_address_type = if (low_byte & 0x80) != 0 {
            AddressType::Group
        } else {
            AddressType::Individual
        };

        // Parse hop count from low byte (bits 6-4)
        let hop_count = (low_byte >> 4) & 0x07;

        ControlField {
            frame_type,
            repeat,
            system_broadcast,
            priority,
            ack_request,
            confirm,
            destination_address_type,
            hop_count,
        }
    }

    #[cfg(test)]
    fn to_u8(self) -> u8 {
        let mut result = 0u8;

        if self.frame_type == FrameType::Standard {
            result |= 0x80;
        }
        if self.repeat {
            result |= 0x20;
        }
        if self.system_broadcast {
            result |= 0x10;
        }

        result |= match self.priority {
            Priority::System => 0,
            Priority::Normal => 1,
            Priority::Urgent => 2,
            Priority::Low => 3,
        } << 2;

        if self.ack_request {
            result |= 0x02;
        }
        if self.confirm {
            result |= 0x01;
        }

        result
    }

    fn to_flags(self) -> u16 {
        // Convert to 2-byte CEMI control field format
        let mut high_byte = 0u8;
        let mut low_byte = 0u8;

        // High byte
        if self.frame_type == FrameType::Standard {
            high_byte |= 0x80;
        }
        if !self.repeat {
            // Note: inverted logic
            high_byte |= 0x20;
        }
        if !self.system_broadcast {
            // Note: inverted logic
            high_byte |= 0x10;
        }

        high_byte |= match self.priority {
            Priority::System => 0,
            Priority::Normal => 1,
            Priority::Urgent => 2,
            Priority::Low => 3,
        } << 2;

        if self.ack_request {
            high_byte |= 0x02;
        }
        if self.confirm {
            high_byte |= 0x01;
        }

        // Low byte
        if self.destination_address_type == AddressType::Group {
            low_byte |= 0x80;
        }

        low_byte |= (self.hop_count & 0x07) << 4;

        (u16::from(high_byte) << 8) | u16::from(low_byte)
    }
}

/// Extended control field (for extended frames)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtendedControlField {
    /// Destination address type (0 = individual, 1 = group)
    pub destination_address_type: AddressType,

    /// Hop count (0-7)
    pub hop_count: u8,

    /// Extended frame format (0-15)
    pub extended_frame_format: u8,
}

impl ExtendedControlField {
    fn from_u8(value: u8) -> Self {
        let destination_address_type = if (value & 0x80) != 0 {
            AddressType::Group
        } else {
            AddressType::Individual
        };

        let hop_count = (value >> 4) & 0x07;
        let extended_frame_format = value & 0x0F;

        ExtendedControlField {
            destination_address_type,
            hop_count,
            extended_frame_format,
        }
    }

    fn to_u8(self) -> u8 {
        let mut result = 0u8;

        if self.destination_address_type == AddressType::Group {
            result |= 0x80;
        }

        result |= (self.hop_count & 0x07) << 4;
        result |= self.extended_frame_format & 0x0F;

        result
    }
}

/// Additional information field
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditionalInfo {
    /// Information type
    pub info_type: u8,

    /// Information data
    pub data: Vec<u8>,
}

impl AdditionalInfo {
    /// Create a new additional info field
    #[must_use]
    pub fn new(info_type: u8, data: Vec<u8>) -> Self {
        Self { info_type, data }
    }

    /// Get the total length of this additional info field (including header)
    #[must_use]
    pub fn total_length(&self) -> usize {
        2 + self.data.len() // type + length + data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Generate a valid `MessageCode` for property testing
    fn arb_message_code() -> impl Strategy<Value = MessageCode> {
        prop_oneof![
            Just(MessageCode::LDataReq),
            Just(MessageCode::LDataCon),
            Just(MessageCode::LDataInd),
            Just(MessageCode::LBusmonInd),
            Just(MessageCode::LRawReq),
            Just(MessageCode::LRawInd),
            Just(MessageCode::LRawCon),
            Just(MessageCode::MPropReadReq),
            Just(MessageCode::MPropReadCon),
            Just(MessageCode::MPropWriteReq),
            Just(MessageCode::MPropWriteCon),
        ]
    }

    /// Generate a valid `AdditionalInfo` for property testing
    fn arb_additional_info() -> impl Strategy<Value = AdditionalInfo> {
        (
            any::<u8>(),
            // Limit additional info data to reasonable sizes (max 50 bytes per field)
            // to ensure total additional info length fits in u8
            prop::collection::vec(any::<u8>(), 0..=50),
        )
            .prop_map(|(info_type, data)| AdditionalInfo { info_type, data })
    }

    /// Generate a valid `IndividualAddress` for property testing
    fn arb_individual_address() -> impl Strategy<Value = IndividualAddress> {
        any::<u16>().prop_map(IndividualAddress::from_raw)
    }

    /// Generate a valid `GroupAddress` for property testing
    fn arb_group_address() -> impl Strategy<Value = GroupAddress> {
        (
            0u8..=GroupAddress::MAX_MAIN,
            0u8..=GroupAddress::MAX_MIDDLE,
            0u8..=GroupAddress::MAX_SUB,
        )
            .prop_map(|(main, middle, sub)| GroupAddress::new(main, middle, sub))
    }

    /// Generate a valid Address for property testing
    fn arb_address() -> impl Strategy<Value = Address> {
        prop_oneof![
            arb_individual_address().prop_map(Address::Individual),
            arb_group_address().prop_map(Address::Group),
        ]
    }

    /// Generate a valid `CemiFrame` for property testing
    fn arb_cemi_frame() -> impl Strategy<Value = CemiFrame> {
        (
            arb_message_code(),
            // Limit to 2 additional info fields to ensure total length fits in u8
            prop::collection::vec(arb_additional_info(), 0..=2),
            arb_individual_address(),
            arb_address(),
            any::<u8>(),
            prop::collection::vec(any::<u8>(), 0..=254),
        )
            .prop_map(
                |(message_code, additional_info, source_addr, dest_addr, tpci, apci_data)| {
                    // Ensure data_length matches actual data
                    let actual_data_length = apci_data.len() as u8;

                    let destination_address_type = match dest_addr {
                        Address::Group(_) => AddressType::Group,
                        Address::Individual(_) => AddressType::Individual,
                    };

                    // For round-trip property testing, we need to use extended frames
                    // because standard frames cannot reliably preserve address type information
                    // for all address values (especially ambiguous ones like 0)
                    let control_field = ControlField {
                        frame_type: FrameType::Extended,
                        repeat: false,
                        system_broadcast: false,
                        priority: Priority::System,
                        ack_request: false,
                        confirm: false,
                        destination_address_type,
                        hop_count: 0,
                    };
                    let extended_control_field = Some(ExtendedControlField {
                        destination_address_type,
                        hop_count: 0,
                        extended_frame_format: 0,
                    });

                    // Ensure TPCI is consistent with data_length
                    let (final_tpci, final_apci_data) = if actual_data_length == 0 {
                        (0, Vec::new())
                    } else {
                        (tpci, apci_data)
                    };

                    CemiFrame {
                        message_code,
                        additional_info,
                        control_field,
                        extended_control_field,
                        source_addr,
                        dest_addr,
                        data_length: actual_data_length,
                        tpci: final_tpci,
                        apci_data: final_apci_data,
                    }
                },
            )
    }

    /// For any valid telegram, serializing then deserializing should produce an equivalent telegram.
    /// This ensures that our CEMI frame parsing and serialization implementations are consistent
    /// and preserve all frame information correctly.
    #[test]
    fn property_cemi_frame_round_trip() {
        proptest!(|(frame in arb_cemi_frame())| {
            let serialized = frame.serialize();
            let parsed = CemiFrame::parse(&serialized)?;
            prop_assert_eq!(frame, parsed);
        });
    }

    /// For any invalid or malformed input data, processing should handle errors gracefully
    /// without panics or memory corruption. This ensures our parsing is robust against
    /// malicious or corrupted network data.
    #[test]
    fn property_memory_safety_under_invalid_input() {
        proptest!(|(data in prop::collection::vec(any::<u8>(), 0..1000))| {
            // This should never panic, only return errors
            let result = std::panic::catch_unwind(|| {
                CemiFrame::parse(&data)
            });

            prop_assert!(result.is_ok(), "CEMI parsing should never panic");

            // If parsing succeeds, the result should be valid
            if let Ok(Ok(frame)) = result {
                // Verify that serializing the parsed frame doesn't panic
                let serialize_result = std::panic::catch_unwind(|| {
                    frame.serialize()
                });
                prop_assert!(serialize_result.is_ok(), "CEMI serialization should never panic");

                // Verify that the serialized data can be parsed again
                if let Ok(serialized) = serialize_result {
                    let reparse_result = std::panic::catch_unwind(|| {
                        CemiFrame::parse(&serialized)
                    });
                    prop_assert!(reparse_result.is_ok(), "Re-parsing serialized CEMI should never panic");
                }
            }
        });
    }

    #[test]
    fn test_message_code_conversion() {
        // Test all message codes round trip correctly
        let codes = [
            MessageCode::LDataReq,
            MessageCode::LDataCon,
            MessageCode::LDataInd,
            MessageCode::LBusmonInd,
            MessageCode::LRawReq,
            MessageCode::LRawInd,
            MessageCode::LRawCon,
            MessageCode::MPropReadReq,
            MessageCode::MPropReadCon,
            MessageCode::MPropWriteReq,
            MessageCode::MPropWriteCon,
        ];

        for code in &codes {
            let byte = code.to_u8();
            let parsed = MessageCode::from_u8(byte).unwrap();
            assert_eq!(*code, parsed);
        }

        // Test invalid message code
        assert!(MessageCode::from_u8(0xFF).is_err());
    }

    #[test]
    fn test_control_field_conversion() {
        let control_field = ControlField {
            frame_type: FrameType::Standard,
            repeat: true,
            system_broadcast: false,
            priority: Priority::Urgent,
            ack_request: true,
            confirm: false,
            destination_address_type: AddressType::Group,
            hop_count: 5,
        };

        let byte = control_field.to_u8();
        let parsed = ControlField::from_u8(byte);

        assert_eq!(control_field.frame_type, parsed.frame_type);
        assert_eq!(control_field.repeat, parsed.repeat);
        assert_eq!(control_field.system_broadcast, parsed.system_broadcast);
        assert_eq!(control_field.priority, parsed.priority);
        assert_eq!(control_field.ack_request, parsed.ack_request);
        assert_eq!(control_field.confirm, parsed.confirm);
    }

    #[test]
    fn test_extended_control_field_conversion() {
        let ext_control_field = ExtendedControlField {
            destination_address_type: AddressType::Group,
            hop_count: 7,
            extended_frame_format: 15,
        };

        let byte = ext_control_field.to_u8();
        let parsed = ExtendedControlField::from_u8(byte);

        assert_eq!(ext_control_field, parsed);
    }

    #[test]
    fn test_cemi_frame_creation() {
        let source = IndividualAddress::new(1, 2, 3);
        let dest = Address::Group(GroupAddress::try_from_raw(0x1234).expect("Valid test address"));
        let data = vec![0x01, 0x02, 0x03];

        let frame = CemiFrame::new(MessageCode::LDataReq, source, dest, data.clone());

        assert_eq!(frame.message_code, MessageCode::LDataReq);
        assert_eq!(frame.source_addr, source);
        assert_eq!(frame.dest_addr, dest);
        assert_eq!(frame.apci_data, data);
        assert_eq!(frame.data_length, 3);
        assert_eq!(
            frame.control_field.destination_address_type,
            AddressType::Group
        );
    }

    #[test]
    fn test_empty_frame_parsing() {
        let result = CemiFrame::parse(&[]);
        assert!(result.is_err());

        let result = CemiFrame::parse(&[0x11]); // Only message code
        assert!(result.is_err());
    }

    #[test]
    fn test_minimal_valid_frame() {
        // Create minimal valid CEMI frame (standard frame)
        // Format: message_code, add_info_len, ctrl1, ctrl2, src_hi, src_lo, dst_hi, dst_lo, data_len, tpci
        let data = vec![
            0x11, // L_Data.req
            0x00, // No additional info
            0xBC, // Control byte 1: Standard frame (0x80), repeat (0x20), system broadcast (0x10), normal priority (0x0C)
            0xE0, // Control byte 2: Group address (0x80), hop count 6 (0x60)
            0x12, 0x34, // Source address
            0x56, 0x78, // Destination address
            0x00, // Data length
        ];

        let frame = CemiFrame::parse(&data).unwrap();
        assert_eq!(frame.message_code, MessageCode::LDataReq);
        assert_eq!(frame.source_addr.raw(), 0x1234);
        assert_eq!(frame.data_length, 0);
        assert_eq!(frame.tpci, 0x00);
        assert!(frame.apci_data.is_empty());
        assert_eq!(frame.control_field.frame_type, FrameType::Standard);
        // Standard frames don't have extended control field in the new parsing
        // The destination address type comes from the 2-byte control field
    }

    #[test]
    fn test_frame_with_additional_info() {
        // Create frame with additional info
        // Format: message_code, add_info_len, [add_info...], ctrl1, ctrl2, src_hi, src_lo, dst_hi, dst_lo, data_len, tpci, [apci_data...]
        let data = vec![
            0x11, // L_Data.req
            0x04, // Additional info length: 4 bytes
            0x01, 0x02, 0xAA, 0xBB, // Additional info: type=1, len=2, data=[0xAA, 0xBB]
            0xBC, // Control byte 1: Standard frame
            0xE0, // Control byte 2: Group address, hop count 6
            0x12, 0x34, // Source address
            0x56, 0x78, // Destination address
            0x01, // Data length
            0x00, 0x81, // TPCI + APCI/data
        ];

        let frame = CemiFrame::parse(&data).unwrap();
        assert_eq!(frame.additional_info.len(), 1);
        assert_eq!(frame.additional_info[0].info_type, 0x01);
        assert_eq!(frame.additional_info[0].data, vec![0xAA, 0xBB]);
        assert_eq!(frame.data_length, 1);
        assert_eq!(frame.apci_data, vec![0x81]);
    }
}
