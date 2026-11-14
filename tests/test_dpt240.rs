#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    CombinedPosition, DPTCombinedPosition, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn blind_positions_use_independent_validity_bits() {
    let registry = DptRegistry::new();
    assert_eq!(
        DptType::from_str("240.800"),
        Some(DptType::CombinedPosition)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("combined_position")
            .unwrap(),
        "240.800"
    );
    for percent in 0..=100 {
        for mask in 0..4 {
            let value = CombinedPosition {
                height_position: (mask & 1 != 0).then_some(percent),
                slats_position: (mask & 2 != 0).then_some(100 - percent),
            };
            let typed = DPTCombinedPosition::new(value).unwrap();
            assert_eq!(typed.as_bytes()[2], mask);
            assert_eq!(
                DPTCombinedPosition::decode(typed.as_bytes())
                    .unwrap()
                    .value(),
                value
            );
            let view = DptType::CombinedPosition
                .decode_ref(typed.as_bytes())
                .unwrap();
            assert_eq!(DptPayload::from(view), DptPayload::CombinedPosition(value));
            assert_eq!(
                DptType::CombinedPosition.parse(&value.to_string()).unwrap(),
                typed.as_bytes()
            );
            assert!(registry.decode("240.800", typed.as_bytes()).is_ok());
        }
    }
    let raw = [128, 255, 0xff];
    assert_eq!(
        DPTCombinedPosition::decode(&raw).unwrap().value(),
        CombinedPosition {
            height_position: Some(50),
            slats_position: Some(100)
        }
    );
    assert_eq!(
        DPTCombinedPosition::decode(&[255, 255, 0]).unwrap().value(),
        CombinedPosition {
            height_position: None,
            slats_position: None
        }
    );
    let invalid = CombinedPosition {
        height_position: Some(101),
        slats_position: None,
    };
    assert!(DPTCombinedPosition::new(invalid).is_err());
    assert!(
        Telegram::group_write_value::<DPTCombinedPosition>(
            GroupAddress::try_from_raw(1).unwrap(),
            invalid
        )
        .is_err()
    );
    assert!(DPTCombinedPosition::decode(&[0; 2]).is_err());
}
