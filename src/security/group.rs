//! Group-level Data Security for encrypting/decrypting secured group telegrams.
//!
//! # ⚠️ Experimental
//!
//! `encrypt_group_payload`/`decrypt_group_payload` have no verified
//! round-trip against a known-good reference implementation or a real
//! Data-Secure device — unlike KNX IP Secure, which was cross-checked
//! against a reference client and confirmed byte-identical against real
//! hardware, this file's PBKDF2/CBC-MAC construction is currently only
//! exercised self-consistently (encrypt and decrypt here agreeing with
//! each other, not with anything external). Do not rely on this for
//! interoperability with real Data-Secure devices until it has been
//! validated against a reference implementation or test vectors.

use crate::error::{Result, SecurityError};
use crate::log_security;
use crate::logging::LogLevel;
use crate::protocol::address::GroupAddress;

use super::keys::KeyRing;
use super::primitives::{calculate_mac_cbc, decrypt_ctr, encrypt_ctr};

/// MAC length in the secured payload (truncated).
const MAC_LENGTH: usize = 4;
/// Sequence number length in the secured payload.
const SEQ_LENGTH: usize = 6;

/// Build the 16-byte CTR `counter_0` block from a 6-byte sequence number.
/// Format: flags(1) | seq(6) | zeros(6) | counter(1=0x00) | padding to 16.
fn build_counter_0(seq_bytes: [u8; 6]) -> [u8; 16] {
    let mut ctr = [0u8; 16];
    ctr[0] = 0x01; // flags: L=2 => flags = (L-1) = 1
    ctr[1..7].copy_from_slice(&seq_bytes);
    ctr
}

/// Build the 16-byte Block 0 for MAC calculation.
/// Format: flags(1) | seq(6) | zeros(6) | length(2) padding to 16.
fn build_block_0(seq_bytes: [u8; 6], payload_len: usize) -> [u8; 16] {
    let mut block = [0u8; 16];
    block[0] = 0x49; // flags for CCM: Adata=1, M=4(mac bytes=4=>t=(M-2)/2=1... actually flags = 0x49 for KNX)
    block[1..7].copy_from_slice(&seq_bytes);
    let len_bytes = (payload_len as u16).to_be_bytes();
    block[14] = len_bytes[0];
    block[15] = len_bytes[1];
    block
}

/// Convert a u64 sequence number to 6 big-endian bytes.
fn seq_to_bytes(sequence_number: u64) -> [u8; 6] {
    let full = sequence_number.to_be_bytes(); // 8 bytes
    let mut out = [0u8; 6];
    out.copy_from_slice(&full[2..8]);
    out
}

/// Encrypt a group payload for Data Secure communication.
///
/// Returns: `seq(6 bytes) | encrypted_payload | mac(4 bytes truncated)`
///
/// # Errors
///
/// Returns [`SecurityError::InvalidCredentials`] if `keyring` has no key for
/// `destination`.
pub fn encrypt_group_payload(
    keyring: &KeyRing,
    destination: &GroupAddress,
    payload: &[u8],
    sequence_number: u64,
) -> Result<Vec<u8>> {
    let key =
        keyring
            .get_group_key(destination)
            .ok_or_else(|| SecurityError::InvalidCredentials {
                details: format!("No group key for {destination}"),
            })?;

    let seq_bytes = seq_to_bytes(sequence_number);
    let block_0 = build_block_0(seq_bytes, payload.len());
    let counter_0 = build_counter_0(seq_bytes);

    // Calculate MAC over plaintext
    let mac_full = calculate_mac_cbc(key.as_bytes(), &seq_bytes, payload, &block_0)?;

    // Encrypt payload and MAC with CTR
    let (encrypted_payload, encrypted_mac) =
        encrypt_ctr(key.as_bytes(), &counter_0, &mac_full, payload)?;

    // Assemble: seq | encrypted_payload | mac(truncated to 4 bytes)
    let mut result = Vec::with_capacity(SEQ_LENGTH + encrypted_payload.len() + MAC_LENGTH);
    result.extend_from_slice(&seq_bytes);
    result.extend_from_slice(&encrypted_payload);
    result.extend_from_slice(&encrypted_mac[..MAC_LENGTH]);

    log_security!(
        LogLevel::Trace,
        "Encrypted group payload for {} (seq={})",
        destination,
        sequence_number
    );

    Ok(result)
}

/// Decrypt a secured group payload from Data Secure communication.
///
/// Expects format: `seq(6 bytes) | encrypted_payload | mac(4 bytes truncated)`
///
/// # Errors
///
/// Returns [`SecurityError::CryptographicError`] if `secured_payload` is
/// shorter than the sequence+MAC overhead or its MAC fails verification, or
/// [`SecurityError::InvalidCredentials`] if `keyring` has no key for
/// `destination`.
///
/// # Panics
///
/// Never panics in practice: the sequence-number slice is only converted to
/// a fixed-size array after the length check above.
pub fn decrypt_group_payload(
    keyring: &KeyRing,
    destination: &GroupAddress,
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

    let key =
        keyring
            .get_group_key(destination)
            .ok_or_else(|| SecurityError::InvalidCredentials {
                details: format!("No group key for {destination}"),
            })?;

    // Parse fields
    let seq_bytes: [u8; 6] = secured_payload[..SEQ_LENGTH]
        .try_into()
        .expect("length checked above");
    let encrypted_data = &secured_payload[SEQ_LENGTH..secured_payload.len() - MAC_LENGTH];
    let encrypted_mac = &secured_payload[secured_payload.len() - MAC_LENGTH..];

    let counter_0 = build_counter_0(seq_bytes);

    // Pad encrypted MAC to 16 bytes for CTR decryption
    let mut mac_padded = [0u8; 16];
    mac_padded[..MAC_LENGTH].copy_from_slice(encrypted_mac);

    // Decrypt MAC and payload
    let (decrypted_payload, decrypted_mac_full) =
        decrypt_ctr(key.as_bytes(), &counter_0, &mac_padded, encrypted_data)?;

    // Verify MAC
    let block_0 = build_block_0(seq_bytes, decrypted_payload.len());
    let expected_mac = calculate_mac_cbc(key.as_bytes(), &seq_bytes, &decrypted_payload, &block_0)?;

    if expected_mac[..MAC_LENGTH] != decrypted_mac_full[..MAC_LENGTH] {
        return Err(SecurityError::CryptographicError {
            operation: "group decrypt".to_string(),
            reason: "MAC verification failed".to_string(),
        }
        .into());
    }

    let seq_number = u64::from_be_bytes([
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
        seq_number
    );

    Ok(decrypted_payload)
}
