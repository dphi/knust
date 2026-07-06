//! Tests for the security layer.

use super::keys::*;
use super::primitives::*;
use super::session::*;
use crate::protocol::address::{GroupAddress, IndividualAddress};

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_byte_pad() {
        // Empty data
        assert_eq!(byte_pad(&[], 16), vec![0u8; 0]);

        // Already aligned
        let data = vec![1u8; 16];
        assert_eq!(byte_pad(&data, 16), data);

        // Needs padding
        let data = vec![1u8; 10];
        let padded = byte_pad(&data, 16);
        assert_eq!(padded.len(), 16);
        assert_eq!(&padded[..10], &data[..]);
        assert_eq!(&padded[10..], &[0u8; 6]);
    }

    #[test]
    fn test_bytes_xor() {
        let a = vec![0xFF, 0x00, 0xAA, 0x55];
        let b = vec![0x0F, 0xF0, 0x55, 0xAA];
        let result = bytes_xor(&a, &b).unwrap();
        assert_eq!(result, vec![0xF0, 0xF0, 0xFF, 0xFF]);
    }

    #[test]
    fn test_bytes_xor_length_mismatch() {
        let a = vec![0xFF, 0x00];
        let b = vec![0x0F];
        assert!(bytes_xor(&a, &b).is_err());
    }

    #[test]
    fn test_sha256_hash() {
        // Test vector: SHA256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let hash = sha256_hash(&[]);
        assert_eq!(hash.len(), 32);
        assert_eq!(
            hex::encode(&hash),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_derive_device_authentication_password() {
        // Test that derivation produces consistent results
        let password = "test_password";
        let derived1 = derive_device_authentication_password(password);
        let derived2 = derive_device_authentication_password(password);
        assert_eq!(derived1, derived2);
        assert_eq!(derived1.len(), 16);
    }

    #[test]
    fn test_derive_user_password() {
        // Test that derivation produces consistent results
        let password = "user_password";
        let derived1 = derive_user_password(password);
        let derived2 = derive_user_password(password);
        assert_eq!(derived1, derived2);
        assert_eq!(derived1.len(), 16);
    }

    #[test]
    fn test_ecdh_key_pair_generation() {
        let key_pair = EcdhKeyPair::generate();
        assert_eq!(key_pair.public_key.len(), 32);
    }

    #[test]
    fn test_ecdh_key_exchange() {
        let alice = EcdhKeyPair::generate();
        let bob = EcdhKeyPair::generate();

        let alice_shared = alice.exchange(&bob.public_key);
        let bob_shared = bob.exchange(&alice.public_key);

        assert_eq!(alice_shared, bob_shared);
        assert_eq!(alice_shared.len(), 32);
    }

    #[test]
    fn test_security_key_from_hex() {
        let key = SecurityKey::from_hex("00112233445566778899aabbccddeeff").unwrap();
        assert_eq!(key.len(), 16);
        assert_eq!(key.as_bytes()[0], 0x00);
        assert_eq!(key.as_bytes()[15], 0xff);
    }

    #[test]
    fn test_security_key_from_hex_with_separators() {
        let key = SecurityKey::from_hex("00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff").unwrap();
        assert_eq!(key.len(), 16);
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    #[tokio::test]
    async fn test_secure_session_creation() {
        let config = SessionConfig {
            user_id: 1,
            user_password: "test_password".to_string(),
            device_auth_password: Some("device_auth".to_string()),
            keepalive_interval: 60,
        };

        let session = SecureSession::new(&config);
        assert_eq!(session.session_id(), 0);
        assert_eq!(session.state().await, SecureSessionState::Uninitialized);
    }

    #[tokio::test]
    async fn test_secure_session_initialize() {
        let config = SessionConfig::default();
        let mut session = SecureSession::new(&config);

        let public_key = session.initialize().await;
        assert_eq!(public_key.len(), 32);
        assert_eq!(session.state().await, SecureSessionState::Handshaking);
    }

    #[tokio::test]
    async fn test_secure_session_close() {
        let config = SessionConfig::default();
        let mut session = SecureSession::new(&config);
        session.initialize().await;

        session.close().await;
        assert_eq!(session.state().await, SecureSessionState::Closed);
    }

    // The full SessionRequest/Response/Authenticate/Status handshake and the
    // SecureWrapper frame format have never been exercised end to end before
    // (only individual primitives were unit-tested) — this is the first
    // real test of it, driving both the pre-existing client-side methods
    // (`process_session_response`, `generate_authenticate_mac`) and the new
    // server-side methods (`process_session_request`,
    // `verify_authenticate_mac`) against each other directly, with no
    // sockets involved.
    #[tokio::test]
    async fn secure_handshake_and_frame_round_trip_between_two_sessions() {
        let device_auth_password = "device-secret".to_string();
        let user_password = "user-secret".to_string();

        let client_config = SessionConfig {
            user_id: 1,
            user_password: user_password.clone(),
            device_auth_password: Some(device_auth_password.clone()),
            keepalive_interval: 60,
        };
        let server_config = client_config.clone();

        let mut client = SecureSession::new(&client_config);
        let mut server = SecureSession::new(&server_config);

        // 1. Client builds SessionRequest (its public key).
        let client_pub: [u8; 32] = client.initialize().await.try_into().unwrap();

        // 2. Server processes it: assigns a session_id, derives the session
        // key, and generates the device-auth MAC for SessionResponse.
        let server_pub: [u8; 32] = server.initialize().await.try_into().unwrap();
        let session_id = 42u16;
        let response_mac = server
            .process_session_request(&client_pub, session_id)
            .await
            .unwrap();

        // 3. Client processes SessionResponse: verifies the device-auth MAC,
        // derives the same session key, and generates its
        // SessionAuthenticate MAC.
        let auth_mac = client
            .process_session_response(session_id, &server_pub, &response_mac)
            .await
            .unwrap();

        // 4. Server verifies SessionAuthenticate.
        let ok = server
            .verify_authenticate_mac(&client_pub, client_config.user_id, &auth_mac)
            .await
            .unwrap();
        assert!(ok, "server should accept a valid SessionAuthenticate MAC");

        // 5. Client completes on SessionStatus = OK.
        client.complete_authentication(0x00).await.unwrap();

        assert!(client.is_authenticated().await);
        assert!(server.is_authenticated().await);

        // 6. Both sides now hold the same session key — frames round-trip
        // in both directions.
        let plain = b"hello secure knx".to_vec();
        let encrypted_by_client = client.encrypt_frame(&plain).await.unwrap();
        let decrypted_by_server = server.decrypt_frame(&encrypted_by_client).await.unwrap();
        assert_eq!(decrypted_by_server, plain);

        let plain2 = b"reply from server".to_vec();
        let encrypted_by_server = server.encrypt_frame(&plain2).await.unwrap();
        let decrypted_by_client = client.decrypt_frame(&encrypted_by_server).await.unwrap();
        assert_eq!(decrypted_by_client, plain2);
    }

    #[tokio::test]
    async fn secure_handshake_rejects_wrong_user_password() {
        let device_auth_password = "device-secret".to_string();

        let client_config = SessionConfig {
            user_id: 1,
            user_password: "correct-password".to_string(),
            device_auth_password: Some(device_auth_password.clone()),
            keepalive_interval: 60,
        };
        let server_config = SessionConfig {
            user_id: 1,
            user_password: "different-password".to_string(),
            device_auth_password: Some(device_auth_password),
            keepalive_interval: 60,
        };

        let mut client = SecureSession::new(&client_config);
        let mut server = SecureSession::new(&server_config);

        let client_pub: [u8; 32] = client.initialize().await.try_into().unwrap();
        let server_pub: [u8; 32] = server.initialize().await.try_into().unwrap();
        let session_id = 7u16;
        let response_mac = server
            .process_session_request(&client_pub, session_id)
            .await
            .unwrap();
        let auth_mac = client
            .process_session_response(session_id, &server_pub, &response_mac)
            .await
            .unwrap();

        let ok = server
            .verify_authenticate_mac(&client_pub, client_config.user_id, &auth_mac)
            .await
            .unwrap();
        assert!(
            !ok,
            "server must reject a SessionAuthenticate MAC signed with the wrong user password"
        );
    }

    #[tokio::test]
    async fn secure_handshake_rejects_wrong_device_auth_password() {
        let client_config = SessionConfig {
            user_id: 1,
            user_password: "user-secret".to_string(),
            device_auth_password: Some("client-side-secret".to_string()),
            keepalive_interval: 60,
        };
        let server_config = SessionConfig {
            user_id: 1,
            user_password: "user-secret".to_string(),
            device_auth_password: Some("server-side-secret".to_string()),
            keepalive_interval: 60,
        };

        let mut client = SecureSession::new(&client_config);
        let mut server = SecureSession::new(&server_config);

        let client_pub: [u8; 32] = client.initialize().await.try_into().unwrap();
        let server_pub: [u8; 32] = server.initialize().await.try_into().unwrap();
        let response_mac = server
            .process_session_request(&client_pub, 1)
            .await
            .unwrap();

        let result = client
            .process_session_response(1, &server_pub, &response_mac)
            .await;
        assert!(
            result.is_err(),
            "client must reject a SessionResponse signed with the wrong device-auth password"
        );
    }
}

#[cfg(test)]
mod keyring_tests {
    use super::*;

    #[test]
    fn test_keyring_group_keys() {
        let mut keyring = KeyRing::new();
        let addr = GroupAddress::from_parts(1, 2, 3).unwrap();
        let key = SecurityKey::new(vec![0u8; 16]);

        keyring.add_group_key(addr, key);

        assert!(keyring.is_group_secured(&addr));
        assert!(keyring.get_group_key(&addr).is_some());
        assert_eq!(keyring.group_key_count(), 1);
    }

    #[test]
    fn test_keyring_sequence_validation() {
        let mut keyring = KeyRing::new();
        let addr = IndividualAddress::new(1, 2, 3);

        // First sequence should succeed
        assert!(keyring.validate_sequence(addr, 100).is_ok());

        // Higher sequence should succeed
        assert!(keyring.validate_sequence(addr, 200).is_ok());

        // Lower sequence should fail (replay attack)
        assert!(keyring.validate_sequence(addr, 150).is_err());

        // Equal sequence should fail
        assert!(keyring.validate_sequence(addr, 200).is_err());
    }
}

/// Property-based tests for security layer
#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;

    // For any secure session and valid data, encrypting then decrypting
    // should produce the original data.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_encrypt_decrypt_round_trip(
            payload in vec(any::<u8>(), 1..256),
            key in vec(any::<u8>(), 16..=16),
        ) {
            // Test CTR encryption/decryption round trip
            let counter_0 = [0u8; 16];
            let mac_input = [0u8; 16];

            let (encrypted, encrypted_mac) = encrypt_ctr(&key, &counter_0, &mac_input, &payload).unwrap();
            let (decrypted, decrypted_mac) = decrypt_ctr(&key, &counter_0, &encrypted_mac, &encrypted).unwrap();

            prop_assert_eq!(decrypted, payload, "Decrypted data should match original");
            prop_assert_eq!(decrypted_mac.as_slice(), &mac_input[..], "Decrypted MAC should match original");
        }

        #[test]
        fn prop_xor_self_inverse(data in vec(any::<u8>(), 1..64)) {
            // XOR with same data twice should return original
            let xored = bytes_xor(&data, &data).unwrap();
            prop_assert!(xored.iter().all(|&b| b == 0), "XOR with self should be all zeros");
        }

        #[test]
        fn prop_xor_commutative(
            a in vec(any::<u8>(), 16..=16),
            b in vec(any::<u8>(), 16..=16),
        ) {
            let ab = bytes_xor(&a, &b).unwrap();
            let ba = bytes_xor(&b, &a).unwrap();
            prop_assert_eq!(ab, ba, "XOR should be commutative");
        }

        #[test]
        fn prop_byte_pad_alignment(
            data in vec(any::<u8>(), 0..100),
            block_size in 1usize..32,
        ) {
            let padded = byte_pad(&data, block_size);
            prop_assert_eq!(padded.len() % block_size, 0, "Padded length should be multiple of block size");
            prop_assert!(padded.len() >= data.len(), "Padded data should not be shorter");
            prop_assert_eq!(&padded[..data.len()], &data[..], "Original data should be preserved");
        }
    }

    // Separate proptest block for expensive key derivation operations
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(5))]

        #[test]
        fn prop_key_derivation_deterministic(password in "[a-zA-Z0-9]{1,32}") {
            let derived1 = derive_user_password(&password);
            let derived2 = derive_user_password(&password);
            prop_assert_eq!(derived1.clone(), derived2, "Key derivation should be deterministic");
            prop_assert_eq!(derived1.len(), 16, "Derived key should be 16 bytes");
        }
    }

    // Separate proptest block for expensive ECDH operations
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn prop_ecdh_shared_secret_symmetric(_seed in any::<u64>()) {
            // Generate two key pairs
            let alice = EcdhKeyPair::generate();
            let bob = EcdhKeyPair::generate();

            // Both parties should derive the same shared secret
            let alice_shared = alice.exchange(&bob.public_key);
            let bob_shared = bob.exchange(&alice.public_key);

            prop_assert_eq!(alice_shared.clone(), bob_shared, "ECDH shared secrets should match");
            prop_assert_eq!(alice_shared.len(), 32, "Shared secret should be 32 bytes");
        }
    }
}
