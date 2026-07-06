//! Cryptographic primitives for KNX Secure communication.
//!
//! This module implements the core cryptographic operations required by
//! KNX Data Security and KNX IP Secure protocols, including:
//! - AES-CBC for MAC calculation
//! - AES-CTR for encryption/decryption
//! - PBKDF2 for key derivation
//! - X25519 for ECDH key exchange

use aes::Aes128;
use aes::cipher::{BlockCipherEncrypt, KeyInit};
use ctr::cipher::{KeyIvInit, StreamCipher};
use pbkdf2::pbkdf2_hmac;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

use crate::error::{Result, SecurityError};

type Aes128Ctr = ctr::Ctr128BE<Aes128>;

/// Salt for device authentication password derivation
const DEVICE_AUTH_SALT: &[u8] = b"device-authentication-code.1.secure.ip.knx.org";

/// Salt for user password derivation  
const USER_PASSWORD_SALT: &[u8] = b"user-password.1.secure.ip.knx.org";

/// PBKDF2 iteration count for KNX Secure
const PBKDF2_ITERATIONS: u32 = 65536;

/// Key length for AES-128
const KEY_LENGTH: usize = 16;

/// Pad data with zeros to a multiple of `block_size`.
#[must_use]
pub fn byte_pad(data: &[u8], block_size: usize) -> Vec<u8> {
    let remainder = data.len() % block_size;
    if remainder == 0 {
        data.to_vec()
    } else {
        let mut padded = data.to_vec();
        padded.resize(data.len() + block_size - remainder, 0);
        padded
    }
}

/// XOR two byte slices of equal length.
///
/// # Errors
///
/// Returns [`SecurityError::CryptographicError`] if `a` and `b` differ in length.
pub fn bytes_xor(a: &[u8], b: &[u8]) -> Result<Vec<u8>> {
    if a.len() != b.len() {
        return Err(SecurityError::CryptographicError {
            operation: "XOR".to_string(),
            reason: format!("Length mismatch: {} vs {}", a.len(), b.len()),
        }
        .into());
    }
    Ok(a.iter().zip(b.iter()).map(|(x, y)| x ^ y).collect())
}

/// Calculate SHA-256 hash of data.
#[must_use]
pub fn sha256_hash(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// Calculate Message Authentication Code using AES-CBC.
///
/// This implements the MAC calculation as specified in KNX Secure.
/// The MAC is the last 16 bytes of the CBC encryption result.
///
/// # Errors
///
/// Returns [`SecurityError::InvalidCredentials`] if `key` is not 16 bytes, or
/// [`SecurityError::CryptographicError`] if `block_0` is not 16 bytes.
///
/// # Panics
///
/// Never panics in practice: the internal AES cipher construction is
/// infallible once `key`'s length has been validated above.
pub fn calculate_mac_cbc(
    key: &[u8],
    additional_data: &[u8],
    payload: &[u8],
    block_0: &[u8],
) -> Result<Vec<u8>> {
    if key.len() != KEY_LENGTH {
        return Err(SecurityError::InvalidCredentials {
            details: format!("Key must be {} bytes, got {}", KEY_LENGTH, key.len()),
        }
        .into());
    }
    if block_0.len() != 16 {
        return Err(SecurityError::CryptographicError {
            operation: "MAC calculation".to_string(),
            reason: format!("Block 0 must be 16 bytes, got {}", block_0.len()),
        }
        .into());
    }

    // Build the input blocks: block_0 + length(additional_data) + additional_data + payload
    let ad_len = (additional_data.len() as u16).to_be_bytes();
    let mut blocks = Vec::with_capacity(block_0.len() + 2 + additional_data.len() + payload.len());
    blocks.extend_from_slice(block_0);
    blocks.extend_from_slice(&ad_len);
    blocks.extend_from_slice(additional_data);
    blocks.extend_from_slice(payload);

    // Pad to block size
    let padded = byte_pad(&blocks, 16);

    // Create AES cipher
    let key_array: [u8; 16] = key
        .try_into()
        .map_err(|_| SecurityError::InvalidCredentials {
            details: "Invalid key length".to_string(),
        })?;
    let cipher = Aes128::new_from_slice(&key_array).expect("validated 16-byte key");

    // Perform CBC-MAC: encrypt each block XORed with previous ciphertext
    let mut y = [0u8; 16]; // Start with zero IV

    for chunk in padded.chunks(16) {
        // XOR with previous ciphertext
        for (i, &byte) in chunk.iter().enumerate() {
            y[i] ^= byte;
        }
        // Encrypt
        let mut block: aes::cipher::Block<Aes128> = y.into();
        cipher.encrypt_block(&mut block);
        y.copy_from_slice(&block);
    }

    // Return the final block as MAC
    Ok(y.to_vec())
}

/// Decrypt data using AES-CTR mode.
///
/// Returns a tuple of (`decrypted_data`, `decrypted_mac`).
/// The MAC is decrypted first with counter 0.
///
/// # Errors
///
/// Returns [`SecurityError::InvalidCredentials`] if `key` is not 16 bytes, or
/// [`SecurityError::CryptographicError`] if `counter_0` is not 16 bytes.
pub fn decrypt_ctr(
    key: &[u8],
    counter_0: &[u8],
    mac: &[u8],
    payload: &[u8],
) -> Result<(Vec<u8>, Vec<u8>)> {
    if key.len() != KEY_LENGTH {
        return Err(SecurityError::InvalidCredentials {
            details: format!("Key must be {} bytes, got {}", KEY_LENGTH, key.len()),
        }
        .into());
    }
    if counter_0.len() != 16 {
        return Err(SecurityError::CryptographicError {
            operation: "CTR decryption".to_string(),
            reason: format!("Counter must be 16 bytes, got {}", counter_0.len()),
        }
        .into());
    }

    let key_array: [u8; 16] = key
        .try_into()
        .map_err(|_| SecurityError::InvalidCredentials {
            details: "Invalid key length".to_string(),
        })?;
    let counter_array: [u8; 16] =
        counter_0
            .try_into()
            .map_err(|_| SecurityError::CryptographicError {
                operation: "CTR decryption".to_string(),
                reason: "Invalid counter length".to_string(),
            })?;

    let mut cipher = Aes128Ctr::new(&key_array.into(), &counter_array.into());

    // Decrypt MAC first (with counter 0)
    let mut mac_decrypted = mac.to_vec();
    cipher.apply_keystream(&mut mac_decrypted);

    // Decrypt payload (with incremented counters)
    let mut decrypted_data = payload.to_vec();
    cipher.apply_keystream(&mut decrypted_data);

    Ok((decrypted_data, mac_decrypted))
}

/// Encrypt data using AES-CTR mode.
///
/// Returns a tuple of (`encrypted_data`, `encrypted_mac`).
/// The MAC is encrypted first with counter 0.
///
/// # Errors
///
/// Returns [`SecurityError::InvalidCredentials`] if `key` is not 16 bytes, or
/// [`SecurityError::CryptographicError`] if `counter_0` is not 16 bytes.
pub fn encrypt_ctr(
    key: &[u8],
    counter_0: &[u8],
    mac_cbc: &[u8],
    payload: &[u8],
) -> Result<(Vec<u8>, Vec<u8>)> {
    if key.len() != KEY_LENGTH {
        return Err(SecurityError::InvalidCredentials {
            details: format!("Key must be {} bytes, got {}", KEY_LENGTH, key.len()),
        }
        .into());
    }
    if counter_0.len() != 16 {
        return Err(SecurityError::CryptographicError {
            operation: "CTR encryption".to_string(),
            reason: format!("Counter must be 16 bytes, got {}", counter_0.len()),
        }
        .into());
    }

    let key_array: [u8; 16] = key
        .try_into()
        .map_err(|_| SecurityError::InvalidCredentials {
            details: "Invalid key length".to_string(),
        })?;
    let counter_array: [u8; 16] =
        counter_0
            .try_into()
            .map_err(|_| SecurityError::CryptographicError {
                operation: "CTR encryption".to_string(),
                reason: "Invalid counter length".to_string(),
            })?;

    let mut cipher = Aes128Ctr::new(&key_array.into(), &counter_array.into());

    // Encrypt MAC first (with counter 0)
    let mut mac_encrypted = mac_cbc.to_vec();
    cipher.apply_keystream(&mut mac_encrypted);

    // Encrypt payload (with incremented counters)
    let mut encrypted_data = payload.to_vec();
    cipher.apply_keystream(&mut encrypted_data);

    Ok((encrypted_data, mac_encrypted))
}

/// Derive device authentication password using PBKDF2-SHA256.
#[must_use]
pub fn derive_device_authentication_password(password: &str) -> Vec<u8> {
    let mut key = vec![0u8; KEY_LENGTH];
    pbkdf2_hmac::<Sha256>(
        password.as_bytes(),
        DEVICE_AUTH_SALT,
        PBKDF2_ITERATIONS,
        &mut key,
    );
    key
}

/// Derive user password using PBKDF2-SHA256.
#[must_use]
pub fn derive_user_password(password: &str) -> Vec<u8> {
    let mut key = vec![0u8; KEY_LENGTH];
    pbkdf2_hmac::<Sha256>(
        password.as_bytes(),
        USER_PASSWORD_SALT,
        PBKDF2_ITERATIONS,
        &mut key,
    );
    key
}

/// ECDH key pair for secure session establishment.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct EcdhKeyPair {
    /// Private key (zeroized on drop)
    #[zeroize(skip)]
    private_key: StaticSecret,
    /// Public key bytes
    pub public_key: [u8; 32],
}

impl EcdhKeyPair {
    /// Generate a new random ECDH key pair.
    #[must_use]
    pub fn generate() -> Self {
        let private_key = StaticSecret::from(rand::random::<[u8; 32]>());
        let public_key = PublicKey::from(&private_key);
        Self {
            private_key,
            public_key: public_key.to_bytes(),
        }
    }

    /// Perform ECDH key exchange with peer's public key.
    #[must_use]
    pub fn exchange(&self, peer_public_key: &[u8; 32]) -> Vec<u8> {
        let peer_pk = PublicKey::from(*peer_public_key);
        self.private_key
            .diffie_hellman(&peer_pk)
            .to_bytes()
            .to_vec()
    }
}

/// Generate an ECDH key pair and return (`private_key_handle`, `public_key_bytes`).
#[must_use]
pub fn generate_ecdh_key_pair() -> EcdhKeyPair {
    EcdhKeyPair::generate()
}

/// Generate a random password suitable for KNX IP Secure device
/// authentication or per-user credentials: a hex-encoded random 16-byte
/// value (128 bits of entropy, 32 hex characters).
#[must_use]
pub fn generate_password() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}
