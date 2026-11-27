use super::primitives::{calculate_mac_cbc, decrypt_ctr, encrypt_ctr};
use crate::error::{Result, SecurityError};
use subtle::ConstantTimeEq;

const WRAPPER_PREFIX: [u8; 4] = [0x06, 0x10, 0x09, 0x50];
const HEADER_LENGTH: usize = 8; // KNX/IP header plus the session ID.
const PAYLOAD_OFFSET: usize = 22;
const MAC_LENGTH: usize = 16;
const OVERHEAD: usize = PAYLOAD_OFFSET + MAC_LENGTH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SecureWrapper {
    pub session_id: u16,
    pub sequence: [u8; 6],
    pub serial_number: [u8; 6],
    pub message_tag: [u8; 2],
}

impl SecureWrapper {
    pub(super) fn sequence_value(self) -> u64 {
        let mut bytes = [0; 8];
        bytes[2..].copy_from_slice(&self.sequence);
        u64::from_be_bytes(bytes)
    }

    pub(super) fn seal(self, key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        let length = plaintext
            .len()
            .checked_add(OVERHEAD)
            .and_then(|length| u16::try_from(length).ok())
            .ok_or_else(|| {
                wrapper_error("encrypt", "SecureWrapper exceeds the KNX/IP frame length")
            })?;
        let mut header = [0; HEADER_LENGTH];
        header[..4].copy_from_slice(&WRAPPER_PREFIX);
        header[4..6].copy_from_slice(&length.to_be_bytes());
        header[6..].copy_from_slice(&self.session_id.to_be_bytes());
        let payload_length = length - OVERHEAD as u16;
        let mac = calculate_mac_cbc(
            key,
            &header,
            plaintext,
            &self.block(payload_length.to_be_bytes()),
        )?;
        let (ciphertext, mac) = encrypt_ctr(key, &self.block([0xff, 0x00]), &mac, plaintext)?;
        let mut packet = Vec::with_capacity(length.into());
        packet.extend_from_slice(&header);
        packet.extend_from_slice(&self.sequence);
        packet.extend_from_slice(&self.serial_number);
        packet.extend_from_slice(&self.message_tag);
        packet.extend_from_slice(&ciphertext);
        packet.extend_from_slice(&mac);
        Ok(packet)
    }

    pub(super) fn open(key: &[u8], session_id: u16, packet: &[u8]) -> Result<(Self, Vec<u8>)> {
        if packet.len() < OVERHEAD
            || !packet.starts_with(&WRAPPER_PREFIX)
            || usize::from(u16::from_be_bytes([packet[4], packet[5]])) != packet.len()
        {
            return Err(wrapper_error("decrypt", "Invalid SecureWrapper header or length").into());
        }
        let header = &packet[..HEADER_LENGTH];
        if header[6..8] != session_id.to_be_bytes() {
            return Err(wrapper_error("decrypt", "SecureWrapper session ID mismatch").into());
        }
        let mut wrapper = Self {
            session_id,
            sequence: [0; 6],
            serial_number: [0; 6],
            message_tag: [0; 2],
        };
        wrapper.sequence.copy_from_slice(&packet[8..14]);
        wrapper.serial_number.copy_from_slice(&packet[14..20]);
        wrapper.message_tag.copy_from_slice(&packet[20..22]);
        let (ciphertext, mac) = packet[PAYLOAD_OFFSET..].split_at(packet.len() - OVERHEAD);
        let (plaintext, received_mac) =
            decrypt_ctr(key, &wrapper.block([0xff, 0x00]), mac, ciphertext)?;
        let payload_length = u16::try_from(plaintext.len())
            .map_err(|_| wrapper_error("decrypt", "SecureWrapper payload too large"))?;
        let expected_mac = calculate_mac_cbc(
            key,
            header,
            &plaintext,
            &wrapper.block(payload_length.to_be_bytes()),
        )?;
        if !bool::from(expected_mac.ct_eq(&received_mac)) {
            return Err(wrapper_error("decrypt", "SecureWrapper MAC verification failed").into());
        }
        Ok((wrapper, plaintext))
    }

    fn block(self, suffix: [u8; 2]) -> [u8; 16] {
        let mut block = [0; 16];
        block[..6].copy_from_slice(&self.sequence);
        block[6..12].copy_from_slice(&self.serial_number);
        block[12..14].copy_from_slice(&self.message_tag);
        block[14..].copy_from_slice(&suffix);
        block
    }
}

fn wrapper_error(operation: &str, reason: &str) -> SecurityError {
    SecurityError::CryptographicError {
        operation: operation.to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multicast_wrapper_matches_wire_vector_and_checks_authenticated_fields() {
        let key: [u8; 16] = std::array::from_fn(|index| u8::try_from(index).unwrap());
        let plaintext = hex::decode("0610053000112900bce011010001010081").unwrap();
        let expected = hex::decode(concat!(
            "06100950003700000102030405060000786b6e78aabb",
            "a909672c044170eb451a3ed22a9feb5c4f35c80c21fc4cc45de23badaf16130a2b"
        ))
        .unwrap();
        let wrapper = SecureWrapper {
            session_id: 0,
            sequence: [1, 2, 3, 4, 5, 6],
            serial_number: [0, 0, b'x', b'k', b'n', b'x'],
            message_tag: [0xaa, 0xbb],
        };
        assert_eq!(wrapper.seal(&key, &plaintext).unwrap(), expected);
        assert_eq!(
            SecureWrapper::open(&key, 0, &expected).unwrap(),
            (wrapper, plaintext)
        );
        assert!(SecureWrapper::open(&key, 1, &expected).is_err());
        for field_offset in [3, 5, 8, 14, 20, PAYLOAD_OFFSET, expected.len() - 1] {
            let mut changed = expected.clone();
            changed[field_offset] ^= 1;
            assert!(SecureWrapper::open(&key, 0, &changed).is_err());
        }
        assert!(wrapper.seal(&key, &vec![0; usize::from(u16::MAX)]).is_err());
    }
}
