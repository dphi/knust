//! Security layer for KNX/IP Secure communication.
//!
//! This module provides encryption, decryption, authentication, and key management
//! for KNX Data Security and KNX IP Secure protocols.

pub mod group;
pub mod keys;
pub mod primitives;
pub mod session;

#[cfg(test)]
mod tests;

pub use group::{decrypt_group_payload, encrypt_group_payload};
pub use keys::{KeyRing, SecurityCredentials, SecurityKey};
pub use primitives::{
    byte_pad, bytes_xor, calculate_mac_cbc, decrypt_ctr, derive_device_authentication_password,
    derive_user_password, encrypt_ctr, generate_ecdh_key_pair, generate_password, sha256_hash,
};
pub use session::{SecureSession, SecureSessionState, SessionConfig};
