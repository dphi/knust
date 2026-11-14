#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DALIDiagnostics, DPTDALIDiagnostics, DPTSceneConfig, DptPayload, DptRegistry, DptType,
    DptValue, SceneConfig,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn scene_configuration_and_dali_diagnostics_round_trip_all_octets() {
    let registry = DptRegistry::new();
    for byte in 0..=u8::MAX {
        let raw = [byte];
        let scene = SceneConfig {
            scene_number: (byte & 63) + 1,
            scene_active: byte & 64 == 0,
            storage_enabled: byte & 128 == 0,
        };
        let dali = DALIDiagnostics {
            address: byte & 63,
            lamp_failure: byte & 64 != 0,
            ballast_failure: byte & 128 != 0,
        };
        assert_eq!(DPTSceneConfig::decode(&raw).unwrap().value(), scene);
        assert_eq!(DPTDALIDiagnostics::decode(&raw).unwrap().value(), dali);
        assert_eq!(DPTSceneConfig::new(scene).unwrap().as_bytes(), raw);
        assert_eq!(DPTDALIDiagnostics::new(dali).unwrap().as_bytes(), raw);
        for (dpt, payload, number) in [
            (
                DptType::SceneConfig,
                DptPayload::SceneConfig(scene),
                "238.001",
            ),
            (
                DptType::DALIDiagnostics,
                DptPayload::DALIDiagnostics(dali),
                "238.600",
            ),
        ] {
            assert_eq!(DptType::from_str(number), Some(dpt));
            assert_eq!(DptPayload::from(dpt.decode_ref(&raw).unwrap()), payload);
            assert_eq!(dpt.encode(&payload).unwrap(), raw);
            assert_eq!(dpt.parse(&payload.formatted(None)).unwrap(), raw);
            assert!(registry.decode(number, &raw).is_ok());
        }
        let address = GroupAddress::try_from_raw(1).unwrap();
        assert_eq!(
            Telegram::group_write_value::<DPTSceneConfig>(address, scene)
                .unwrap()
                .payload,
            raw
        );
        assert_eq!(
            Telegram::group_write_value::<DPTDALIDiagnostics>(address, dali)
                .unwrap()
                .payload,
            raw
        );
    }
    assert!(
        DPTSceneConfig::new(SceneConfig {
            scene_number: 0,
            scene_active: true,
            storage_enabled: true
        })
        .is_err()
    );
    assert!(
        DPTDALIDiagnostics::new(DALIDiagnostics {
            address: 64,
            lamp_failure: false,
            ballast_failure: false
        })
        .is_err()
    );
    assert!(DPTSceneConfig::decode(&[]).is_err());
    assert!(
        DptType::SceneConfig
            .encode(&DptPayload::DALIDiagnostics(DALIDiagnostics {
                address: 1,
                lamp_failure: false,
                ballast_failure: false
            }))
            .is_err()
    );
}
