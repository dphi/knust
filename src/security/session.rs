//! Secure session management for KNX IP Secure.
//!
//! This module provides the `SecureSession` struct for managing encrypted
//! communication sessions with KNX/IP gateways.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use zeroize::Zeroize;

use super::primitives::{
    EcdhKeyPair, bytes_xor, calculate_mac_cbc, decrypt_ctr, derive_device_authentication_password,
    derive_user_password, encrypt_ctr, sha256_hash,
};
use crate::error::{Result, SecurityError};

/// Counter value used in handshake (`SessionResponse` and `SessionAuthenticate`)
const COUNTER_0_HANDSHAKE: [u8; 16] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x00,
];

/// Message tag for tunneling (0x0000)
const MESSAGE_TAG_TUNNELLING: [u8; 2] = [0x00, 0x00];

/// Knx serial number for identification
const DEVICE_SERIAL_NUMBER: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

/// Configuration for secure session establishment.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// User ID for authentication (1-127)
    pub user_id: u8,
    /// User password
    pub user_password: String,
    /// Device authentication password (optional)
    pub device_auth_password: Option<String>,
    /// Session keepalive interval in seconds
    pub keepalive_interval: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            user_id: 1,
            user_password: String::new(),
            device_auth_password: None,
            keepalive_interval: 60,
        }
    }
}

/// State of a secure session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureSessionState {
    /// Session not initialized
    Uninitialized,
    /// Handshake in progress
    Handshaking,
    /// Session authenticated and ready
    Authenticated,
    /// Session expired or closed
    Closed,
    /// Session in error state
    Error,
}

/// Secure session for KNX IP Secure communication.
///
/// Manages encryption/decryption of KNX/IP frames and session state.
pub struct SecureSession {
    /// Session ID assigned by the gateway
    session_id: u16,
    /// Session key derived from ECDH exchange
    session_key: Arc<RwLock<SessionKey>>,
    /// Sequence number for outgoing frames
    sequence_number: AtomicU64,
    /// Last received sequence number
    sequence_number_received: AtomicU64,
    /// Current session state
    state: Arc<RwLock<SecureSessionState>>,
    /// ECDH key pair for this session
    key_pair: Option<EcdhKeyPair>,
    /// Derived device authentication code
    device_auth_code: Option<Vec<u8>>,
    /// Derived user password
    user_password: Vec<u8>,
    /// User ID
    user_id: u8,
}

/// Session key wrapper with zeroization on drop.
#[derive(Zeroize, Default)]
#[zeroize(drop)]
struct SessionKey {
    key: [u8; 16],
    initialized: bool,
}

impl SecureSession {
    /// Create a new secure session with the given configuration.
    #[must_use]
    pub fn new(config: &SessionConfig) -> Self {
        let device_auth_code = config
            .device_auth_password
            .as_ref()
            .map(|p| derive_device_authentication_password(p));
        let user_password = derive_user_password(&config.user_password);

        Self {
            session_id: 0,
            session_key: Arc::new(RwLock::new(SessionKey::default())),
            sequence_number: AtomicU64::new(0),
            sequence_number_received: AtomicU64::new(0),
            state: Arc::new(RwLock::new(SecureSessionState::Uninitialized)),
            key_pair: None,
            device_auth_code,
            user_password,
            user_id: config.user_id,
        }
    }

    /// Initialize the session and generate ECDH key pair.
    pub async fn initialize(&mut self) -> Vec<u8> {
        let key_pair = EcdhKeyPair::generate();
        let public_key = key_pair.public_key.to_vec();
        self.key_pair = Some(key_pair);

        *self.state.write().await = SecureSessionState::Handshaking;
        self.sequence_number.store(0, Ordering::SeqCst);
        self.sequence_number_received.store(0, Ordering::SeqCst);

        public_key
    }

    /// Get the current session state.
    pub async fn state(&self) -> SecureSessionState {
        *self.state.read().await
    }

    /// Get the session ID.
    pub fn session_id(&self) -> u16 {
        self.session_id
    }

    /// Process session response from gateway and complete handshake.
    ///
    /// Returns the MAC for `SessionAuthenticate` message.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::InvalidConfiguration`] if [`Self::initialize`]
    /// hasn't been called yet, or [`SecurityError::AuthenticationFailed`] if
    /// device authentication is configured and the server's MAC doesn't verify.
    pub async fn process_session_response(
        &mut self,
        session_id: u16,
        server_public_key: &[u8; 32],
        server_mac: &[u8],
    ) -> Result<Vec<u8>> {
        let key_pair =
            self.key_pair
                .as_ref()
                .ok_or_else(|| SecurityError::InvalidConfiguration {
                    details: "Session not initialized".to_string(),
                })?;

        // Verify server MAC if device authentication is configured
        if let Some(ref device_auth) = self.device_auth_code {
            let pub_keys_xor = bytes_xor(&key_pair.public_key, server_public_key)?;

            // SessionResponse header: 06 10 09 52 00 38
            let response_header = [0x06, 0x10, 0x09, 0x52, 0x00, 0x38];
            let session_id_bytes = session_id.to_be_bytes();

            let mut additional_data = Vec::new();
            additional_data.extend_from_slice(&response_header);
            additional_data.extend_from_slice(&session_id_bytes);
            additional_data.extend_from_slice(&pub_keys_xor);

            let mac_cbc = calculate_mac_cbc(device_auth, &additional_data, &[], &[0u8; 16])?;

            let (_, mac_tr) = decrypt_ctr(device_auth, &COUNTER_0_HANDSHAKE, server_mac, &[])?;

            if mac_cbc != mac_tr {
                return Err(SecurityError::AuthenticationFailed {
                    reason: "SessionResponse MAC verification failed".to_string(),
                }
                .into());
            }
        }

        self.session_id = session_id;

        // Calculate session key from ECDH shared secret
        let shared_secret = key_pair.exchange(server_public_key);
        let session_key_full = sha256_hash(&shared_secret);
        let mut session_key = SessionKey::default();
        session_key.key.copy_from_slice(&session_key_full[..16]);
        session_key.initialized = true;
        *self.session_key.write().await = session_key;

        // Generate SessionAuthenticate MAC
        self.generate_authenticate_mac(server_public_key)
    }

    /// Generate MAC for `SessionAuthenticate` message.
    fn generate_authenticate_mac(&self, server_public_key: &[u8; 32]) -> Result<Vec<u8>> {
        let key_pair =
            self.key_pair
                .as_ref()
                .ok_or_else(|| SecurityError::InvalidConfiguration {
                    details: "Session not initialized".to_string(),
                })?;

        let pub_keys_xor = bytes_xor(&key_pair.public_key, server_public_key)?;

        // SessionAuthenticate header: 06 10 09 53 00 18
        let auth_header = [0x06, 0x10, 0x09, 0x53, 0x00, 0x18];

        let mut additional_data = Vec::new();
        additional_data.extend_from_slice(&auth_header);
        additional_data.push(0x00); // reserved
        additional_data.push(self.user_id);
        additional_data.extend_from_slice(&pub_keys_xor);

        let mac_cbc = calculate_mac_cbc(&self.user_password, &additional_data, &[], &[0u8; 16])?;

        let (_, mac) = encrypt_ctr(&self.user_password, &COUNTER_0_HANDSHAKE, &mac_cbc, &[])?;

        Ok(mac)
    }

    /// Process a `SessionRequest` from a connecting client and compute the
    /// session key plus the device-authentication MAC for `SessionResponse`.
    ///
    /// This is the server-side mirror of [`process_session_response`]: the
    /// caller (the server's dispatch code) assigns `session_id` (analogous to
    /// how it assigns a tunnel `channel_id`), this computes the same ECDH
    /// shared secret + session key math, and — instead of *verifying* an
    /// incoming device-auth MAC like the client does — *generates* one to
    /// send back, proving to the client that this server knows the device
    /// authentication password. Mirrors `generate_authenticate_mac`'s
    /// compute-then-encrypt pattern, using `device_auth_code` and the
    /// `SessionResponse` header instead of `user_password` and the
    /// `SessionAuthenticate` header.
    ///
    /// If no device authentication password is configured, returns an
    /// all-zero MAC — matches [`process_session_response`]'s existing
    /// leniency of skipping verification entirely when `device_auth_code` is
    /// `None`, so a client without a configured device-auth password still
    /// completes the handshake against a server configured the same way.
    ///
    /// [`process_session_response`]: SecureSession::process_session_response
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::InvalidConfiguration`] if [`Self::initialize`]
    /// hasn't been called yet.
    pub async fn process_session_request(
        &mut self,
        client_public_key: &[u8; 32],
        session_id: u16,
    ) -> Result<Vec<u8>> {
        let key_pair =
            self.key_pair
                .as_ref()
                .ok_or_else(|| SecurityError::InvalidConfiguration {
                    details: "Session not initialized".to_string(),
                })?;

        self.session_id = session_id;

        let shared_secret = key_pair.exchange(client_public_key);
        let session_key_full = sha256_hash(&shared_secret);
        let mut session_key = SessionKey::default();
        session_key.key.copy_from_slice(&session_key_full[..16]);
        session_key.initialized = true;
        *self.session_key.write().await = session_key;

        let Some(ref device_auth) = self.device_auth_code else {
            return Ok(vec![0u8; 16]);
        };

        let pub_keys_xor = bytes_xor(&key_pair.public_key, client_public_key)?;

        // SessionResponse header: 06 10 09 52 00 38
        let response_header = [0x06, 0x10, 0x09, 0x52, 0x00, 0x38];
        let mut additional_data = Vec::new();
        additional_data.extend_from_slice(&response_header);
        additional_data.extend_from_slice(&session_id.to_be_bytes());
        additional_data.extend_from_slice(&pub_keys_xor);

        let mac_cbc = calculate_mac_cbc(device_auth, &additional_data, &[], &[0u8; 16])?;
        let (_, mac) = encrypt_ctr(device_auth, &COUNTER_0_HANDSHAKE, &mac_cbc, &[])?;

        Ok(mac)
    }

    /// Verify a client's `SessionAuthenticate` MAC.
    ///
    /// Server-side mirror of `generate_authenticate_mac`'s verification
    /// counterpart — recomputes the expected MAC the same way
    /// [`process_session_response`] verifies a device-auth MAC (via
    /// `decrypt_ctr` and comparison), but using `user_password` and the
    /// `SessionAuthenticate` header. Returns `Ok(false)` (not an error) for a
    /// `user_id` this session isn't configured for, or a MAC mismatch;
    /// transitions to `Authenticated` on success, same as
    /// [`complete_authentication`].
    ///
    /// [`process_session_response`]: SecureSession::process_session_response
    /// [`complete_authentication`]: SecureSession::complete_authentication
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::InvalidConfiguration`] if [`Self::initialize`]
    /// hasn't been called yet.
    pub async fn verify_authenticate_mac(
        &mut self,
        client_public_key: &[u8; 32],
        received_user_id: u8,
        received_mac: &[u8],
    ) -> Result<bool> {
        if received_user_id != self.user_id {
            return Ok(false);
        }

        let key_pair =
            self.key_pair
                .as_ref()
                .ok_or_else(|| SecurityError::InvalidConfiguration {
                    details: "Session not initialized".to_string(),
                })?;

        let pub_keys_xor = bytes_xor(&key_pair.public_key, client_public_key)?;

        // SessionAuthenticate header: 06 10 09 53 00 18
        let auth_header = [0x06, 0x10, 0x09, 0x53, 0x00, 0x18];
        let mut additional_data = Vec::new();
        additional_data.extend_from_slice(&auth_header);
        additional_data.push(0x00); // reserved
        additional_data.push(received_user_id);
        additional_data.extend_from_slice(&pub_keys_xor);

        let mac_cbc = calculate_mac_cbc(&self.user_password, &additional_data, &[], &[0u8; 16])?;
        let (_, mac_tr) =
            decrypt_ctr(&self.user_password, &COUNTER_0_HANDSHAKE, received_mac, &[])?;

        if mac_cbc != mac_tr {
            return Ok(false);
        }

        *self.state.write().await = SecureSessionState::Authenticated;
        Ok(true)
    }

    /// Complete authentication after receiving `SessionStatus`.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::AuthenticationFailed`] if `status_code` is
    /// not `0x00`.
    pub async fn complete_authentication(&mut self, status_code: u8) -> Result<()> {
        if status_code != 0x00 {
            *self.state.write().await = SecureSessionState::Error;
            return Err(SecurityError::AuthenticationFailed {
                reason: format!("Authentication failed with status code: 0x{status_code:02X}"),
            }
            .into());
        }

        *self.state.write().await = SecureSessionState::Authenticated;
        Ok(())
    }

    /// Encrypt a KNX/IP frame for transmission.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::InvalidConfiguration`] if the session key
    /// hasn't been established yet (handshake not complete).
    pub async fn encrypt_frame(&self, plain_frame: &[u8]) -> Result<Vec<u8>> {
        let session_key = self.session_key.read().await;
        if !session_key.initialized {
            return Err(SecurityError::InvalidConfiguration {
                details: "Session key not initialized".to_string(),
            }
            .into());
        }

        let sequence_info = self.get_next_sequence_number();
        let message_tag = MESSAGE_TAG_TUNNELLING;
        let payload_length = plain_frame.len();

        // Total length: 6 header + 2 session_id + 6 sequence + 6 serial + 2 tag + 16 MAC = 38 + payload
        let total_length = 38 + payload_length;

        // SecureWrapper header: 06 10 09 50 + length
        let mut wrapper_header = vec![0x06, 0x10, 0x09, 0x50];
        wrapper_header.extend_from_slice(&(total_length as u16).to_be_bytes());

        // Build block_0 for MAC calculation
        let mut block_0 = Vec::with_capacity(16);
        block_0.extend_from_slice(&sequence_info);
        block_0.extend_from_slice(&DEVICE_SERIAL_NUMBER);
        block_0.extend_from_slice(&message_tag);
        block_0.extend_from_slice(&(payload_length as u16).to_be_bytes());

        // Additional data: wrapper_header + session_id
        let mut additional_data = wrapper_header.clone();
        additional_data.extend_from_slice(&self.session_id.to_be_bytes());

        let mac_cbc = calculate_mac_cbc(&session_key.key, &additional_data, plain_frame, &block_0)?;

        // Build counter_0 for CTR encryption
        let mut counter_0 = Vec::with_capacity(16);
        counter_0.extend_from_slice(&sequence_info);
        counter_0.extend_from_slice(&DEVICE_SERIAL_NUMBER);
        counter_0.extend_from_slice(&message_tag);
        counter_0.extend_from_slice(&[0xFF, 0x00]);

        let (encrypted_data, encrypted_mac) =
            encrypt_ctr(&session_key.key, &counter_0, &mac_cbc, plain_frame)?;

        // Build SecureWrapper frame
        let mut result = wrapper_header;
        result.extend_from_slice(&self.session_id.to_be_bytes());
        result.extend_from_slice(&sequence_info);
        result.extend_from_slice(&DEVICE_SERIAL_NUMBER);
        result.extend_from_slice(&message_tag);
        result.extend_from_slice(&encrypted_data);
        result.extend_from_slice(&encrypted_mac);

        Ok(result)
    }

    /// Decrypt a `SecureWrapper` frame.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::InvalidConfiguration`] if the session key
    /// hasn't been established yet, or [`SecurityError::CryptographicError`]
    /// if `encrypted_frame` is too short to be a valid `SecureWrapper`.
    pub async fn decrypt_frame(&self, encrypted_frame: &[u8]) -> Result<Vec<u8>> {
        let session_key = self.session_key.read().await;
        if !session_key.initialized {
            return Err(SecurityError::InvalidConfiguration {
                details: "Session key not initialized".to_string(),
            }
            .into());
        }

        // Parse SecureWrapper: header(6) + session_id(2) + sequence(6) + serial(6) + tag(2) + data + mac(16)
        if encrypted_frame.len() < 38 {
            return Err(SecurityError::CryptographicError {
                operation: "decrypt".to_string(),
                reason: "Frame too short for SecureWrapper".to_string(),
            }
            .into());
        }

        let frame_session_id = u16::from_be_bytes([encrypted_frame[6], encrypted_frame[7]]);
        if frame_session_id != self.session_id {
            return Err(SecurityError::CryptographicError {
                operation: "decrypt".to_string(),
                reason: format!(
                    "Session ID mismatch: expected {}, got {}",
                    self.session_id, frame_session_id
                ),
            }
            .into());
        }

        let sequence_info = &encrypted_frame[8..14];
        let serial_number = &encrypted_frame[14..20];
        let message_tag = &encrypted_frame[20..22];
        let encrypted_data = &encrypted_frame[22..encrypted_frame.len() - 16];
        let mac = &encrypted_frame[encrypted_frame.len() - 16..];

        // Validate sequence number
        let received_seq = u64::from_be_bytes({
            let mut arr = [0u8; 8];
            arr[2..].copy_from_slice(sequence_info);
            arr
        });
        let last_seq = self.sequence_number_received.load(Ordering::SeqCst);
        if received_seq <= last_seq && last_seq != 0 {
            return Err(SecurityError::CryptographicError {
                operation: "decrypt".to_string(),
                reason: format!("Invalid sequence number: {received_seq} <= {last_seq}"),
            }
            .into());
        }

        // Build counter_0 for CTR decryption
        let mut counter_0 = Vec::with_capacity(16);
        counter_0.extend_from_slice(sequence_info);
        counter_0.extend_from_slice(serial_number);
        counter_0.extend_from_slice(message_tag);
        counter_0.extend_from_slice(&[0xFF, 0x00]);

        let (decrypted_data, mac_tr) =
            decrypt_ctr(&session_key.key, &counter_0, mac, encrypted_data)?;

        // Verify MAC
        let wrapper_header = &encrypted_frame[..6];
        let mut additional_data = wrapper_header.to_vec();
        additional_data.extend_from_slice(&self.session_id.to_be_bytes());

        let mut block_0 = Vec::with_capacity(16);
        block_0.extend_from_slice(sequence_info);
        block_0.extend_from_slice(serial_number);
        block_0.extend_from_slice(message_tag);
        block_0.extend_from_slice(&(decrypted_data.len() as u16).to_be_bytes());

        let mac_cbc = calculate_mac_cbc(
            &session_key.key,
            &additional_data,
            &decrypted_data,
            &block_0,
        )?;

        if mac_cbc != mac_tr {
            return Err(SecurityError::CryptographicError {
                operation: "decrypt".to_string(),
                reason: "MAC verification failed".to_string(),
            }
            .into());
        }

        // Update received sequence number
        self.sequence_number_received
            .store(received_seq, Ordering::SeqCst);

        Ok(decrypted_data)
    }

    /// Get the next sequence number and increment the counter.
    fn get_next_sequence_number(&self) -> [u8; 6] {
        let seq = self.sequence_number.fetch_add(1, Ordering::SeqCst);
        let bytes = seq.to_be_bytes();
        let mut result = [0u8; 6];
        result.copy_from_slice(&bytes[2..]);
        result
    }

    /// Close the session.
    pub async fn close(&mut self) {
        *self.state.write().await = SecureSessionState::Closed;
        // Zeroize session key
        let mut session_key = self.session_key.write().await;
        session_key.key.zeroize();
        session_key.initialized = false;
    }

    /// Check if the session is authenticated and ready.
    pub async fn is_authenticated(&self) -> bool {
        *self.state.read().await == SecureSessionState::Authenticated
    }
}

impl Drop for SecureSession {
    fn drop(&mut self) {
        // Zeroize sensitive data
        self.user_password.zeroize();
        if let Some(ref mut auth) = self.device_auth_code {
            auth.zeroize();
        }
    }
}
