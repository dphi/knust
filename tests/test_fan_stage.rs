#![cfg(feature = "dpt")]

use knust::protocol::Telegram;
use knust::protocol::address::GroupAddress;
use knust::protocol::dpt::{DPTFanStage, DptPayload, DptRegistry, DptType, DptValue};

#[test]
fn fan_stage_preserves_every_octet() {
    let registry = DptRegistry::new();
    assert_eq!(DptType::from_str("5.100"), Some(DptType::FanStage));
    assert_eq!(
        registry.dpt_number_for_value_type("fan_stage").unwrap(),
        "5.100"
    );
    for value in 0..=u8::MAX {
        let typed = DPTFanStage::new(value);
        assert_eq!(typed.as_bytes(), [value]);
        assert_eq!(DPTFanStage::decode(&[value]).unwrap().value(), value);
        let payload = DptPayload::UnsignedInt(value.into());
        assert_eq!(DptType::FanStage.encode(&payload).unwrap(), [value]);
        assert_eq!(
            DptPayload::from(DptType::FanStage.decode_ref(&[value]).unwrap()),
            payload
        );
        let telegram = Telegram::group_write_value::<DPTFanStage>(
            GroupAddress::try_from_raw(1).unwrap(),
            value,
        )
        .unwrap();
        assert_eq!(telegram.payload, [value]);
        assert!(!telegram.compact_payload);
    }
    assert!(DPTFanStage::decode(&[]).is_err());
    assert!(DPTFanStage::decode(&[0, 0]).is_err());
}
