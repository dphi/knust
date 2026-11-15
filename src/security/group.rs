//! Group-level KNX Data Secure APDU encryption and authentication.
//!
//! Frame addresses and protected control fields are inputs to the wire format.

use crate::error::{Result, SecurityError};
use crate::log_security;
use crate::logging::LogLevel;
use crate::protocol::address::Address;
use crate::protocol::cemi::{AddressType, CemiFrame};

use super::keys::KeyRing;
use super::primitives::{calculate_mac_cbc, decrypt_ctr, encrypt_ctr};

/// MAC length in the secured payload (truncated).
const MAC_LENGTH: usize = 4;
/// Sequence number length in the secured payload.
const SEQ_LENGTH: usize = 6;

fn build_counter_0(seq_bytes: [u8; 6], address_fields: [u8; 4]) -> [u8; 16] {
    let mut ctr = [0u8; 16];
    ctr[..6].copy_from_slice(&seq_bytes);
    ctr[6..10].copy_from_slice(&address_fields);
    ctr[14] = 1;
    ctr
}

fn build_block_0(
    seq_bytes: [u8; 6],
    frame: &CemiFrame,
    address_fields: [u8; 4],
    payload_len: usize,
) -> Result<[u8; 16]> {
    let frame_format = frame
        .extended_control_field
        .map_or(0, |field| field.extended_frame_format);
    if frame_format != 0 {
        return Err(SecurityError::CryptographicError {
            operation: "group encrypt".to_string(),
            reason: format!("Unsupported CEMI frame format: {frame_format}"),
        }
        .into());
    }

    let address_type = match frame.dest_addr {
        Address::Group(_) => AddressType::Group,
        Address::Individual(_) => AddressType::Individual,
    };
    let mut block = [0u8; 16];
    block[..6].copy_from_slice(&seq_bytes);
    block[6..10].copy_from_slice(&address_fields);
    block[11] = match address_type {
        AddressType::Individual => 0,
        AddressType::Group => 0x80,
    };
    block[12] = (frame.tpci & 0xFC) | 0x03;
    block[13] = 0xF1;
    block[15] = payload_len as u8;
    Ok(block)
}

/// Convert a u64 sequence number to 6 big-endian bytes.
fn seq_to_bytes(sequence_number: u64) -> [u8; 6] {
    let full = sequence_number.to_be_bytes(); // 8 bytes
    let mut out = [0u8; 6];
    out.copy_from_slice(&full[2..8]);
    out
}

/// Encrypt a plain group APDU using KNX Data Secure.
///
/// The frame supplies the source, group destination, TPCI, address type, and
/// frame format that are authenticated by the KNX wire format.
/// The secured output is sequence number, encrypted APDU, and four-byte MAC.
pub fn encrypt_group_payload(
    keyring: &KeyRing,
    frame: &CemiFrame,
    payload: &[u8],
    sequence_number: u64,
) -> Result<Vec<u8>> {
    let Address::Group(destination) = frame.dest_addr else {
        return Err(SecurityError::CryptographicError {
            operation: "group encrypt".to_string(),
            reason: "Data Secure group frames require a group destination".to_string(),
        }
        .into());
    };
    if sequence_number > 0xFFFF_FFFF_FFFF {
        return Err(SecurityError::CryptographicError {
            operation: "group encrypt".to_string(),
            reason: "Sequence number exceeds the 48-bit KNX range".to_string(),
        }
        .into());
    }
    if payload.len() > u8::MAX as usize {
        return Err(SecurityError::CryptographicError {
            operation: "group encrypt".to_string(),
            reason: "Payload exceeds the one-byte KNX Data Secure length".to_string(),
        }
        .into());
    }

    let key =
        keyring
            .get_group_key(&destination)
            .ok_or_else(|| SecurityError::InvalidCredentials {
                details: format!("No group key for {destination}"),
            })?;

    let address_fields = [
        (frame.source_addr.raw() >> 8) as u8,
        frame.source_addr.raw() as u8,
        (destination.raw() >> 8) as u8,
        destination.raw() as u8,
    ];
    let seq_bytes = seq_to_bytes(sequence_number);
    let block_0 = build_block_0(seq_bytes, frame, address_fields, payload.len())?;
    let counter_0 = build_counter_0(seq_bytes, address_fields);

    let mac = calculate_mac_cbc(key.as_bytes(), &[0x10], payload, &block_0)?;
    let (encrypted_payload, encrypted_mac) =
        encrypt_ctr(key.as_bytes(), &counter_0, &mac[..MAC_LENGTH], payload)?;

    let mut result = Vec::with_capacity(SEQ_LENGTH + encrypted_payload.len() + MAC_LENGTH);
    result.extend_from_slice(&seq_bytes);
    result.extend_from_slice(&encrypted_payload);
    result.extend_from_slice(&encrypted_mac);

    log_security!(
        LogLevel::Trace,
        "Encrypted group payload for {} (seq={})",
        destination,
        sequence_number
    );

    Ok(result)
}

/// Decrypt and authenticate a KNX Data Secure group payload.
///
/// `frame` must carry the original CEMI source/destination and control metadata;
/// `secured_payload` is sequence number, encrypted APDU, and four-byte MAC.
pub fn decrypt_group_payload(
    keyring: &KeyRing,
    frame: &CemiFrame,
    secured_payload: &[u8],
) -> Result<Vec<u8>> {
    let min_len = SEQ_LENGTH + MAC_LENGTH;
    if secured_payload.len() < min_len {
        return Err(SecurityError::CryptographicError {
            operation: "group decrypt".to_string(),
            reason: format!(
                "Secured payload too short: {} bytes (minimum {})",
                secured_payload.len(),
                min_len
            ),
        }
        .into());
    }
    let Address::Group(destination) = frame.dest_addr else {
        return Err(SecurityError::CryptographicError {
            operation: "group decrypt".to_string(),
            reason: "Data Secure group frames require a group destination".to_string(),
        }
        .into());
    };

    let key =
        keyring
            .get_group_key(&destination)
            .ok_or_else(|| SecurityError::InvalidCredentials {
                details: format!("No group key for {destination}"),
            })?;

    let seq_bytes: [u8; 6] = secured_payload[..SEQ_LENGTH]
        .try_into()
        .expect("length checked above");
    let encrypted_data = &secured_payload[SEQ_LENGTH..secured_payload.len() - MAC_LENGTH];
    let encrypted_mac = &secured_payload[secured_payload.len() - MAC_LENGTH..];

    let address_fields = [
        (frame.source_addr.raw() >> 8) as u8,
        frame.source_addr.raw() as u8,
        (destination.raw() >> 8) as u8,
        destination.raw() as u8,
    ];
    let counter_0 = build_counter_0(seq_bytes, address_fields);
    let (decrypted_payload, decrypted_mac) =
        decrypt_ctr(key.as_bytes(), &counter_0, encrypted_mac, encrypted_data)?;

    let block_0 = build_block_0(seq_bytes, frame, address_fields, decrypted_payload.len())?;
    let expected_mac = calculate_mac_cbc(key.as_bytes(), &[0x10], &decrypted_payload, &block_0)?;

    if expected_mac[..MAC_LENGTH] != decrypted_mac {
        return Err(SecurityError::CryptographicError {
            operation: "group decrypt".to_string(),
            reason: "MAC verification failed".to_string(),
        }
        .into());
    }

    let sequence_number = u64::from_be_bytes([
        0,
        0,
        seq_bytes[0],
        seq_bytes[1],
        seq_bytes[2],
        seq_bytes[3],
        seq_bytes[4],
        seq_bytes[5],
    ]);
    log_security!(
        LogLevel::Trace,
        "Decrypted group payload for {} (seq={})",
        destination,
        sequence_number
    );

    Ok(decrypted_payload)
}

/// Build the secure APDU fields for a group CEMI frame.
pub fn encrypt_group_frame(
    keyring: &KeyRing,
    frame: &CemiFrame,
    sequence_number: u64,
) -> Result<CemiFrame> {
    if frame.apci_data.is_empty() || frame.apci_data.len() != usize::from(frame.data_length) {
        return Err(SecurityError::CryptographicError {
            operation: "group encrypt".to_string(),
            reason: "CEMI frame has an invalid APDU length".to_string(),
        }
        .into());
    }

    let mut apdu = Vec::with_capacity(frame.apci_data.len() + 1);
    apdu.push((frame.tpci & 0x03) << 6);
    apdu.extend_from_slice(&frame.apci_data);
    let secured = encrypt_group_payload(keyring, frame, &apdu, sequence_number)?;

    let mut secure_frame = frame.clone();
    secure_frame.tpci = (frame.tpci & 0xFC) | 0x03;
    secure_frame.apci_data.clear();
    secure_frame.apci_data.extend_from_slice(&[0xF1, 0x10]);
    secure_frame.apci_data.extend_from_slice(&secured);
    if secure_frame.apci_data.len() > u8::MAX as usize {
        return Err(SecurityError::CryptographicError {
            operation: "group encrypt".to_string(),
            reason: "Secured APDU exceeds the CEMI length field".to_string(),
        }
        .into());
    }
    secure_frame.data_length = secure_frame.apci_data.len() as u8;
    Ok(secure_frame)
}

/// Restore a group CEMI frame from a secured APDU.
pub fn decrypt_group_frame(keyring: &KeyRing, frame: &CemiFrame) -> Result<CemiFrame> {
    if frame.apci_data.len() < 2 || frame.apci_data[0] != 0xF1 || frame.apci_data[1] != 0x10 {
        return Err(SecurityError::CryptographicError {
            operation: "group decrypt".to_string(),
            reason: "Unsupported Data Secure service or algorithm".to_string(),
        }
        .into());
    }

    let apdu = decrypt_group_payload(keyring, frame, &frame.apci_data[2..])?;
    if apdu.is_empty() {
        return Err(SecurityError::CryptographicError {
            operation: "group decrypt".to_string(),
            reason: "Decrypted APDU is empty".to_string(),
        }
        .into());
    }

    let mut plain_frame = frame.clone();
    plain_frame.tpci = (frame.tpci & 0xFC) | (apdu[0] >> 6);
    plain_frame.apci_data = apdu[1..].to_vec();
    plain_frame.data_length = u8::try_from(plain_frame.apci_data.len()).map_err(|_| {
        SecurityError::CryptographicError {
            operation: "group decrypt".to_string(),
            reason: "Decrypted APDU exceeds the CEMI length field".to_string(),
        }
    })?;
    Ok(plain_frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::address::{Address, GroupAddress, IndividualAddress};
    use crate::protocol::cemi::{CemiFrame, MessageCode};

    fn reference_frame() -> CemiFrame {
        CemiFrame::new(
            MessageCode::LDataReq,
            "5.0.1".parse::<IndividualAddress>().unwrap(),
            Address::Group("0/4/0".parse::<GroupAddress>().unwrap()),
            vec![0x00, 0x00],
        )
    }

    fn reference_keyring() -> KeyRing {
        let destination = "0/4/0".parse::<GroupAddress>().unwrap();
        let mut keyring = KeyRing::default();
        keyring.add_group_key(
            destination,
            super::super::keys::SecurityKey::from_hex("dfdf23a59fbb40404091d1c162087e8b").unwrap(),
        );
        keyring
    }

    #[test]
    fn matches_reference_group_read_vector() {
        let secured = encrypt_group_payload(
            &reference_keyring(),
            &reference_frame(),
            &[0x00, 0x00],
            160170101607,
        )
        .unwrap();

        assert_eq!(&secured[..6], &[0x00, 0x25, 0x4a, 0xe1, 0xcb, 0x67]);
        assert_eq!(&secured[6..8], &[0xcd, 0x18]);
        assert_eq!(&secured[8..], &[0x4a, 0xfe, 0x57, 0x44]);
    }

    #[test]
    fn wraps_and_restores_secure_cemi_apdu() {
        let plain = CemiFrame::new(
            MessageCode::LDataReq,
            "5.0.1".parse::<IndividualAddress>().unwrap(),
            Address::Group("0/4/0".parse::<GroupAddress>().unwrap()),
            vec![0x00],
        );
        let keyring = reference_keyring();
        let secure = encrypt_group_frame(&keyring, &plain, 160170101607).unwrap();

        assert_eq!(secure.tpci, 0x03);
        assert_eq!(secure.data_length, 14);
        assert_eq!(&secure.apci_data[..2], &[0xF1, 0x10]);
        assert_eq!(&secure.apci_data[8..10], &[0xCD, 0x18]);
        assert_eq!(&secure.apci_data[10..], &[0x4A, 0xFE, 0x57, 0x44]);

        let restored = decrypt_group_frame(&keyring, &secure).unwrap();
        assert_eq!(restored.tpci, plain.tpci);
        assert_eq!(restored.apci_data, plain.apci_data);
        assert_eq!(restored.data_length, plain.data_length);
    }

    #[test]
    fn decrypts_reference_vector_and_rejects_modified_payload() {
        let secured = [
            0x00, 0x25, 0x4a, 0xe1, 0xcb, 0x67, 0xcd, 0x18, 0x4a, 0xfe, 0x57, 0x44,
        ];
        let keyring = reference_keyring();
        let frame = reference_frame();

        assert_eq!(
            decrypt_group_payload(&keyring, &frame, &secured).unwrap(),
            [0x00, 0x00]
        );

        let mut tampered = secured;
        tampered[6] ^= 0x01;
        assert!(decrypt_group_payload(&keyring, &frame, &tampered).is_err());
    }
}
