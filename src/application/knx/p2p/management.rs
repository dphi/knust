//! Management operations over a connected point-to-point link.

use super::P2pConnection;
use crate::error::Result;

/// One interface object and its discovered property descriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedInterfaceObject {
    pub object_index: u8,
    pub object_type: u16,
    pub properties: Vec<crate::protocol::management::PropertyDescriptionResponse>,
}

impl P2pConnection {
    /// Read a single property-value range over this P2P session.
    pub async fn read_property_value(
        &mut self,
        request: crate::protocol::management::PropertyValueRead,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::PropertyValueResponse> {
        let response = self
            .request(request.encode()?, [0x03, 0xD6], timeout)
            .await?;
        let response = crate::protocol::management::PropertyValueResponse::decode(&response)?;
        if response.object_index != request.object_index
            || response.property_id != request.property_id
            || response.start_index != request.start_index
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "PropertyValueResponse does not match the requested object, property, and start index".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Read a property value range, splitting it into APDUs that fit the negotiated limit.
    pub async fn read_property_values(
        &mut self,
        request: crate::protocol::management::PropertyValueRead,
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        if max_apdu_length == 0 || request.count == 0 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property read requires a positive APDU length and element count"
                    .to_owned(),
            }
            .into());
        }
        if request.start_index == 0 && request.count != 1 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property element-count query at index 0 requires count 1".to_owned(),
            }
            .into());
        }
        if u32::from(request.start_index) + u32::from(request.count) - 1 > 0x0FFF {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property read range exceeds the 12-bit start index".to_owned(),
            }
            .into());
        }

        let mut data = Vec::new();
        let mut current_index = request.start_index;
        let mut remaining = request.count;
        let mut max_elements_per_chunk = 1;
        let mut element_size = None;

        while remaining > 0 {
            let chunk_count = remaining.min(max_elements_per_chunk);
            let response = self
                .read_property_value(
                    crate::protocol::management::PropertyValueRead {
                        object_index: request.object_index,
                        property_id: request.property_id,
                        count: chunk_count,
                        start_index: current_index,
                    },
                    timeout,
                )
                .await?;
            if response.count != chunk_count {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Property read requested {chunk_count} elements at index {current_index}, got {}",
                        response.count
                    ),
                }
                .into());
            }
            let response_size = response.data.len();
            if let Some(element_size) = element_size {
                if response_size != usize::from(chunk_count) * element_size {
                    return Err(crate::error::ProtocolError::InvalidFrame {
                        details:
                            "Property read response data length does not match its element count"
                                .to_owned(),
                    }
                    .into());
                }
            } else {
                if response_size == 0 {
                    return Err(crate::error::ProtocolError::InvalidFrame {
                        details: "Property read returned empty data for one element".to_owned(),
                    }
                    .into());
                }
                element_size = Some(response_size);
                max_elements_per_chunk =
                    (max_apdu_length.saturating_sub(6) / response_size).clamp(1, 15) as u8;
            }
            data.extend(response.data);
            remaining -= chunk_count;
            current_index += u16::from(chunk_count);
        }

        Ok(data)
    }

    /// Write a single property-value range over this P2P session.
    pub async fn write_property_value(
        &mut self,
        request: crate::protocol::management::PropertyValueWrite,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::PropertyValueResponse> {
        let response = self
            .request(request.encode()?, [0x03, 0xD6], timeout)
            .await?;
        let response = crate::protocol::management::PropertyValueResponse::decode(&response)?;
        if response.object_index != request.object_index
            || response.property_id != request.property_id
            || response.start_index != request.start_index
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "PropertyValueResponse does not match the requested object, property, and start index".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Switch programming mode through Device Object property PID_PROGMODE (54).
    ///
    /// Use this property-based procedure for Realisation Type 1 devices.
    /// With `verify`, the write response must echo the requested value.
    pub async fn switch_programming_mode_property(
        &mut self,
        enabled: bool,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let value = u8::from(enabled);
        let response = self
            .write_property_value(
                crate::protocol::management::PropertyValueWrite {
                    object_index: 0,
                    property_id: 54,
                    count: 1,
                    start_index: 1,
                    data: vec![value],
                },
                timeout,
            )
            .await?;
        if response.count != 1 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Programming-mode property write returned count {}",
                    response.count
                ),
            }
            .into());
        }
        if verify && response.data != [value] {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Programming-mode write response did not echo {value}: {:?}",
                    response.data
                ),
            }
            .into());
        }
        Ok(())
    }

    /// Verify a property value range by reading it back and comparing its bytes.
    pub async fn verify_property_values(
        &mut self,
        request: crate::protocol::management::PropertyValueRead,
        expected: &[u8],
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<()> {
        if self
            .read_property_values(request, max_apdu_length, timeout)
            .await?
            != expected
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Property verification failed for object {} property {} at index {}",
                    request.object_index, request.property_id, request.start_index
                ),
            }
            .into());
        }
        Ok(())
    }

    /// Write a property value range, splitting it into APDUs that fit the negotiated limit.
    pub async fn write_property_values(
        &mut self,
        request: crate::protocol::management::PropertyValueWrite,
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<()> {
        if max_apdu_length == 0 || request.count == 0 || request.start_index == 0 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property write requires a positive APDU length, count, and start index"
                    .to_owned(),
            }
            .into());
        }
        if u32::from(request.start_index) + u32::from(request.count) - 1 > 0x0FFF {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property write range exceeds the 12-bit start index".to_owned(),
            }
            .into());
        }
        let element_count = usize::from(request.count);
        if request.data.is_empty() || !request.data.len().is_multiple_of(element_count) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details:
                    "Property write data must contain an equal, nonzero byte count per element"
                        .to_owned(),
            }
            .into());
        }
        let element_size = request.data.len() / element_count;
        let max_elements_per_chunk = (max_apdu_length.saturating_sub(6) / element_size).min(15);
        if max_elements_per_chunk == 0 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "One property element does not fit within the APDU length limit"
                    .to_owned(),
            }
            .into());
        }

        let mut current_index = request.start_index;
        let mut remaining = request.count;
        let mut data_offset = 0;
        while remaining > 0 {
            let chunk_count = remaining.min(max_elements_per_chunk as u8);
            let chunk_size = usize::from(chunk_count) * element_size;
            let response = self
                .write_property_value(
                    crate::protocol::management::PropertyValueWrite {
                        object_index: request.object_index,
                        property_id: request.property_id,
                        count: chunk_count,
                        start_index: current_index,
                        data: request.data[data_offset..data_offset + chunk_size].to_vec(),
                    },
                    timeout,
                )
                .await?;
            if response.count != chunk_count {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Property write requested {chunk_count} elements at index {current_index}, got {}",
                        response.count
                    ),
                }
                .into());
            }
            current_index += u16::from(chunk_count);
            remaining -= chunk_count;
            data_offset += chunk_size;
        }

        Ok(())
    }

    /// Read one range from the standard 64 KiB memory address space.
    pub async fn read_memory(
        &mut self,
        request: crate::protocol::management::MemoryRead,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::MemoryResponse> {
        let response = self
            .request(request.encode()?, [0x02, 0x40], timeout)
            .await?;
        let response = crate::protocol::management::MemoryResponse::decode(&response)?;
        if response.address != request.address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "MemoryResponse address does not match the request".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Write one range to the standard 64 KiB memory address space.
    ///
    /// Memory writes have a transport acknowledgement but no application response.
    pub async fn write_memory(
        &mut self,
        request: crate::protocol::management::MemoryWrite,
    ) -> Result<()> {
        if request.count == 0
            || request.data.len() != usize::from(request.count)
            || request.data.len() > 0x1_0000 - usize::from(request.address)
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Memory write data, count, or address range is invalid".to_owned(),
            }
            .into());
        }
        self.send_data(request.encode()?, true).await
    }

    /// Switch programming mode through memory address 0x60 (Realisation Type 2).
    ///
    /// Preserves the other bits and toggles the parity bit when the mode changes.
    /// `verify` reads the byte back after the transport-acknowledged write.
    pub async fn switch_programming_mode_memory(
        &mut self,
        enabled: bool,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let response = self
            .read_memory(
                crate::protocol::management::MemoryRead {
                    address: 0x0060,
                    count: 1,
                },
                timeout,
            )
            .await?;
        if response.count != 1 || response.data.len() != 1 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Programming-mode memory read must return one byte".to_owned(),
            }
            .into());
        }
        let current = response.data[0];
        if (current & 1 != 0) == enabled {
            return Ok(());
        }
        let value = current ^ 0x81;
        self.write_memory(crate::protocol::management::MemoryWrite {
            address: 0x0060,
            count: 1,
            data: vec![value],
        })
        .await?;
        if verify {
            let response = self
                .read_memory(
                    crate::protocol::management::MemoryRead {
                        address: 0x0060,
                        count: 1,
                    },
                    timeout,
                )
                .await?;
            if response.count != 1 || response.data != [value] {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!("Programming-mode read-back returned {:?}", response.data),
                }
                .into());
            }
        }
        Ok(())
    }

    /// Read one range from the 1 MiB user-memory address space.
    pub async fn read_user_memory(
        &mut self,
        request: crate::protocol::management::UserMemoryRead,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::UserMemoryResponse> {
        let response = self
            .request(request.encode()?, [0x02, 0xC1], timeout)
            .await?;
        let response = crate::protocol::management::UserMemoryResponse::decode(&response)?;
        if response.address != request.address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "UserMemoryResponse address does not match the request".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Write one range to the 1 MiB user-memory address space.
    ///
    /// Memory writes have a transport acknowledgement but no application response.
    pub async fn write_user_memory(
        &mut self,
        request: crate::protocol::management::UserMemoryWrite,
    ) -> Result<()> {
        if request.count == 0
            || request.data.len() != usize::from(request.count)
            || request.address > 0x0F_FFFF
            || request.data.len() > 0x10_0000 - request.address as usize
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "User memory write data, count, or address range is invalid".to_owned(),
            }
            .into());
        }
        self.send_data(request.encode()?, true).await
    }

    /// Read a standard-memory range in response-sized chunks.
    pub async fn read_memory_range(
        &mut self,
        address: u16,
        count: usize,
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        if count == 0 || max_apdu_length <= 4 || count > 0x1_0000 - usize::from(address) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Memory read count, APDU length, or address range is invalid".to_owned(),
            }
            .into());
        }
        let chunk_limit = (max_apdu_length - 4).min(63);
        let mut data = Vec::with_capacity(count);
        let mut current_address = usize::from(address);
        while data.len() < count {
            let chunk_count = (count - data.len()).min(chunk_limit);
            let response = self
                .read_memory(
                    crate::protocol::management::MemoryRead {
                        address: current_address as u16,
                        count: chunk_count as u8,
                    },
                    timeout,
                )
                .await?;
            if usize::from(response.count) != chunk_count || response.data.len() != chunk_count {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Memory read at 0x{current_address:04X} requested {chunk_count} bytes, got {}",
                        response.count
                    ),
                }
                .into());
            }
            data.extend(response.data);
            current_address += chunk_count;
        }
        Ok(data)
    }

    /// Read a user-memory range in response-sized chunks.
    pub async fn read_user_memory_range(
        &mut self,
        address: u32,
        count: usize,
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        if count == 0
            || max_apdu_length <= 5
            || address > 0x10_0000
            || count > 0x10_0000 - address as usize
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "User memory read count, APDU length, or address range is invalid"
                    .to_owned(),
            }
            .into());
        }
        let chunk_limit = (max_apdu_length - 5).min(15);
        let mut data = Vec::with_capacity(count);
        let mut current_address = address;
        while data.len() < count {
            let chunk_count = (count - data.len()).min(chunk_limit);
            let response = self
                .read_user_memory(
                    crate::protocol::management::UserMemoryRead {
                        address: current_address,
                        count: chunk_count as u8,
                    },
                    timeout,
                )
                .await?;
            if usize::from(response.count) != chunk_count || response.data.len() != chunk_count {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "User memory read at 0x{current_address:05X} requested {chunk_count} bytes, got {}",
                        response.count
                    ),
                }
                .into());
            }
            data.extend(response.data);
            current_address += chunk_count as u32;
        }
        Ok(data)
    }

    /// Write a standard-memory range in bounded chunks, optionally reading each back.
    pub async fn write_memory_range(
        &mut self,
        address: u16,
        data: &[u8],
        max_apdu_length: usize,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        if data.is_empty() || max_apdu_length <= 4 || data.len() > 0x1_0000 - usize::from(address) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Memory write data, APDU length, or address range is invalid".to_owned(),
            }
            .into());
        }
        let chunk_limit = (max_apdu_length - 4).min(63);
        let mut offset = 0;
        while offset < data.len() {
            let chunk = &data[offset..(offset + chunk_limit).min(data.len())];
            let current_address = usize::from(address) + offset;
            self.write_memory(crate::protocol::management::MemoryWrite {
                address: current_address as u16,
                count: chunk.len() as u8,
                data: chunk.to_vec(),
            })
            .await?;
            if verify
                && self
                    .read_memory_range(
                        current_address as u16,
                        chunk.len(),
                        max_apdu_length,
                        timeout,
                    )
                    .await?
                    != chunk
            {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!("Memory verification failed at 0x{current_address:04X}"),
                }
                .into());
            }
            offset += chunk.len();
        }
        Ok(())
    }

    /// Write a user-memory range in bounded chunks, optionally reading each back.
    pub async fn write_user_memory_range(
        &mut self,
        address: u32,
        data: &[u8],
        max_apdu_length: usize,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        if data.is_empty()
            || max_apdu_length <= 5
            || address > 0x10_0000
            || data.len() > 0x10_0000 - address as usize
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "User memory write data, APDU length, or address range is invalid"
                    .to_owned(),
            }
            .into());
        }
        let chunk_limit = (max_apdu_length - 5).min(15);
        let mut offset = 0;
        while offset < data.len() {
            let chunk = &data[offset..(offset + chunk_limit).min(data.len())];
            let current_address = address + offset as u32;
            self.write_user_memory(crate::protocol::management::UserMemoryWrite {
                address: current_address,
                count: chunk.len() as u8,
                data: chunk.to_vec(),
            })
            .await?;
            if verify
                && self
                    .read_user_memory_range(current_address, chunk.len(), max_apdu_length, timeout)
                    .await?
                    != chunk
            {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!("User memory verification failed at 0x{current_address:05X}"),
                }
                .into());
            }
            offset += chunk.len();
        }
        Ok(())
    }

    /// Read metadata for one interface-object property over this P2P session.
    pub async fn read_property_description(
        &mut self,
        request: crate::protocol::management::PropertyDescriptionRead,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::PropertyDescriptionResponse> {
        let response = self
            .request(request.encode(), [0x03, 0xD9], timeout)
            .await?;
        let response = crate::protocol::management::PropertyDescriptionResponse::decode(&response)?;
        if response.object_index != request.object_index
            || (request.property_id != 0 && response.property_id != request.property_id)
            || response.property_index != request.property_index
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "PropertyDescriptionResponse does not match the requested object, property, and index".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Authorize this P2P session with a device access key.
    pub async fn authorize(
        &mut self,
        key: u32,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::AuthorizeResponse> {
        let response = self
            .request(
                crate::protocol::management::AuthorizeRequest { key }.encode(),
                [0x03, 0xD2],
                timeout,
            )
            .await?;
        crate::protocol::management::AuthorizeResponse::decode(&response)
    }

    /// Try free access and the configured key, returning the better access level.
    pub async fn authorize_best(
        &mut self,
        client_key: u32,
        timeout: std::time::Duration,
    ) -> Result<u8> {
        let free_level = self
            .authorize(crate::protocol::management::FREE_ACCESS_KEY, timeout)
            .await?
            .level;
        let client_level = self.authorize(client_key, timeout).await?.level;
        Ok(free_level.min(client_level))
    }

    /// Read one device descriptor over this P2P session.
    pub async fn read_device_descriptor(
        &mut self,
        descriptor: u8,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::DeviceDescriptorResponse> {
        if descriptor > 0x3F {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("Device descriptor type {descriptor} exceeds six bits"),
            }
            .into());
        }
        let response = self
            .request(
                crate::protocol::management::DeviceDescriptorRead { descriptor }.encode(),
                [0x03, 0x40],
                timeout,
            )
            .await?;
        let response = crate::protocol::management::DeviceDescriptorResponse::decode(&response)?;
        if response.descriptor != descriptor {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "DeviceDescriptorResponse type does not match the request".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Confirm device management access and return the device mask version.
    pub async fn read_mask_version(&mut self, timeout: std::time::Duration) -> Result<u16> {
        let response = self.read_device_descriptor(0, timeout).await?;
        if response.data.len() != 2 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Mask version descriptor must contain two bytes".to_owned(),
            }
            .into());
        }
        Ok(u16::from_be_bytes([response.data[0], response.data[1]]))
    }

    /// Switch programming mode using the mechanism associated with the mask version.
    ///
    /// Unknown mask versions return an error without writing to the device.
    pub async fn switch_programming_mode(
        &mut self,
        enabled: bool,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let mask = self.read_mask_version(timeout).await?;
        match mask {
            0x0010..=0x0013
            | 0x0020
            | 0x0021
            | 0x0025
            | 0x0700
            | 0x0701
            | 0x0705
            | 0x0900
            | 0x0910..=0x0912
            | 0x091a
            | 0x1011..=0x1013
            | 0x1900
            | 0x5705 => {
                self.switch_programming_mode_memory(enabled, verify, timeout)
                    .await
            }
            0x0300 | 0x07b0 | 0x17b0 | 0x2705 | 0x27b0 | 0x2920 | 0x57b0 => {
                self.switch_programming_mode_property(enabled, verify, timeout)
                    .await
            }
            _ => Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Programming-mode location is unknown for mask version {mask:04X}"
                ),
            }
            .into()),
        }
    }

    /// Send an unconfirmed basic restart to the connected device.
    ///
    /// The device can restart before acknowledging the telegram and the P2P
    /// link may stop working. Close the connection after this operation.
    pub async fn restart(&mut self) -> Result<()> {
        self.send_data(vec![0x03, 0x80], false).await
    }

    /// Perform a confirmed Master Reset on the connected device.
    ///
    /// Wait at least the returned process_time before sending another request.
    pub async fn master_reset(
        &mut self,
        erase_code: u8,
        channel_number: u8,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::RestartMasterResetResponse> {
        if !(1..=8).contains(&erase_code) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("Master Reset erase code {erase_code} is not defined"),
            }
            .into());
        }
        let response = self
            .request(
                crate::protocol::management::RestartMasterReset {
                    erase_code,
                    channel_number,
                }
                .encode(),
                [0x03, 0xA1],
                timeout,
            )
            .await?;
        let response = crate::protocol::management::RestartMasterResetResponse::decode(&response)?;
        if response.error_code != 0 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Master Reset failed with error code 0x{:02X}",
                    response.error_code
                ),
            }
            .into());
        }
        Ok(response)
    }

    /// Discover interface objects and their property descriptions on this device.
    pub async fn scan_interface_objects(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Vec<ScannedInterfaceObject>> {
        let mut objects = Vec::new();
        for object_index in 0..=u8::MAX {
            let first_property = self
                .read_property_description(
                    crate::protocol::management::PropertyDescriptionRead {
                        object_index,
                        property_id: 0,
                        property_index: 0,
                    },
                    timeout,
                )
                .await?;
            if first_property.property_id == 0 {
                return Ok(objects);
            }

            let object_type = self
                .read_property_values(
                    crate::protocol::management::PropertyValueRead {
                        object_index,
                        property_id: 1,
                        count: 1,
                        start_index: 1,
                    },
                    15,
                    timeout,
                )
                .await?;
            if object_type.len() != 2 {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Object type for interface object {object_index} must be two bytes"
                    ),
                }
                .into());
            }

            let mut properties = Vec::new();
            let mut found_property_end = false;
            for property_index in 0..=u8::MAX {
                let property = self
                    .read_property_description(
                        crate::protocol::management::PropertyDescriptionRead {
                            object_index,
                            property_id: 0,
                            property_index,
                        },
                        timeout,
                    )
                    .await?;
                if property.property_id == 0 {
                    found_property_end = true;
                    break;
                }
                properties.push(property);
            }
            if !found_property_end {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Property list for interface object {object_index} has no terminator"
                    ),
                }
                .into());
            }
            objects.push(ScannedInterfaceObject {
                object_index,
                object_type: u16::from_be_bytes([object_type[0], object_type[1]]),
                properties,
            });
            if object_index == u8::MAX {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: "Interface object list has no terminator".to_owned(),
                }
                .into());
            }
        }
        Err(crate::error::ProtocolError::InvalidFrame {
            details: "Interface object scan ended without a terminator".to_owned(),
        }
        .into())
    }
}
