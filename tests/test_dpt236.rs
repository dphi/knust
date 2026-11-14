#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTPrioritisedModeControl, DptPayload, DptRegistry, DptType, DptValue, PrioritisedModeControl,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn prioritised_mode_round_trips_every_octet() {
    let registry = DptRegistry::new();
    assert_eq!(
        DptType::from_str("236.001"),
        Some(DptType::PrioritisedModeControl)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("prioritised_mode_control")
            .unwrap(),
        "236.001"
    );
    for byte in 0..=u8::MAX {
        let raw = [byte];
        let value = PrioritisedModeControl {
            priority_level: (byte >> 4) & 7,
            mode_level: byte & 15,
            deactivate_priority: byte & 128 != 0,
        };
        assert_eq!(
            DPTPrioritisedModeControl::decode(&raw).unwrap().value(),
            value
        );
        assert_eq!(
            DPTPrioritisedModeControl::new(value).unwrap().as_bytes(),
            raw
        );
        let view = DptType::PrioritisedModeControl.decode_ref(&raw).unwrap();
        assert_eq!(
            DptPayload::from(view),
            DptPayload::PrioritisedModeControl(value)
        );
        assert_eq!(
            DptType::PrioritisedModeControl
                .parse(&value.to_string())
                .unwrap(),
            raw
        );
        assert!(registry.decode("236.001", &raw).is_ok());
        let telegram = Telegram::group_write_value::<DPTPrioritisedModeControl>(
            GroupAddress::try_from_raw(1).unwrap(),
            value,
        )
        .unwrap();
        assert!(!telegram.compact_payload);
        assert_eq!(telegram.payload, raw);
    }
    assert!(
        DPTPrioritisedModeControl::new(PrioritisedModeControl {
            priority_level: 8,
            mode_level: 0,
            deactivate_priority: false
        })
        .is_err()
    );
    assert!(
        DPTPrioritisedModeControl::new(PrioritisedModeControl {
            priority_level: 0,
            mode_level: 16,
            deactivate_priority: false
        })
        .is_err()
    );
    assert!(DPTPrioritisedModeControl::decode(&[]).is_err());
}
