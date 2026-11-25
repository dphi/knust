#![cfg(feature = "dpt")]
use knust::protocol::dpt::{DPTSceneInfo, DptPayload, DptRegistry, DptType, DptValue, SceneInfo};
use knust::protocol::{Telegram, address::RawGroupAddress};

#[test]
fn scene_info_handles_every_octet_and_writes_canonical_values() {
    let registry = DptRegistry::new();
    assert_eq!("26.001".parse::<DptType>().ok(), Some(DptType::SceneInfo));
    assert_eq!(
        registry.dpt_number_for_value_type("scene_info").unwrap(),
        "26.001"
    );
    for byte in 0..=u8::MAX {
        let raw = [byte];
        let expected = SceneInfo {
            scene_number: (byte & 63) + 1,
            active: byte & 64 == 0,
        };
        let typed = DPTSceneInfo::decode(&raw).unwrap();
        assert_eq!(typed.value(), expected);
        let view = DptType::SceneInfo.decode_ref(&raw).unwrap();
        assert_eq!(DptPayload::from(view), DptPayload::SceneInfo(expected));
        let canonical = [byte & 0x7f];
        assert_eq!(
            DptType::SceneInfo
                .encode(&DptPayload::SceneInfo(expected))
                .unwrap(),
            canonical
        );
        assert_eq!(
            DptType::SceneInfo.parse(&expected.to_string()).unwrap(),
            canonical
        );
        assert!(registry.decode("26.001", &raw).is_ok());
        let telegram = Telegram::group_write_value::<DPTSceneInfo>(
            RawGroupAddress::try_from_raw(1).unwrap(),
            expected,
        )
        .unwrap();
        assert!(!telegram.compact_payload);
        assert_eq!(telegram.payload, canonical);
    }
    for number in [0, 65, 255] {
        assert!(
            DPTSceneInfo::new(SceneInfo {
                scene_number: number,
                active: true
            })
            .is_err()
        );
    }
    assert!(DPTSceneInfo::decode(&[]).is_err());
    assert!(DptType::SceneInfo.parse("65,true").is_err());
}
