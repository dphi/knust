#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    ColorTemperatureTransition, DPTColorTemperatureTransition, DptPayload, DptRegistry, DptType,
    DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn color_temperature_transition_tracks_validity_and_rounding() {
    let reg = DptRegistry::new();
    assert_eq!(
        DptType::from_str("249.600"),
        Some(DptType::ColorTemperatureTransition)
    );
    assert_eq!(
        reg.dpt_number_for_value_type("color_temperature_transition")
            .unwrap(),
        "249.600"
    );
    for flags in 0..=u8::MAX {
        let raw = [0, 15, 0x12, 0x34, 200, flags];
        let value = ColorTemperatureTransition {
            fade_time: (flags & 4 != 0).then_some(1.5),
            color_temperature: (flags & 2 != 0).then_some(0x1234),
            brightness: (flags & 1 != 0).then_some(200),
        };
        assert_eq!(
            DPTColorTemperatureTransition::decode(&raw).unwrap().value(),
            value
        );
        let view = DptType::ColorTemperatureTransition
            .decode_ref(&raw)
            .unwrap();
        assert_eq!(
            DptPayload::from(view),
            DptPayload::ColorTemperatureTransition(value)
        );
        let canonical = DPTColorTemperatureTransition::new(value).unwrap();
        assert_eq!(canonical.as_bytes()[5], flags & 7);
        assert_eq!(
            DptType::ColorTemperatureTransition
                .parse(&value.to_string())
                .unwrap(),
            canonical.as_bytes()
        );
    }
    let value = ColorTemperatureTransition {
        fade_time: Some(0.25),
        color_temperature: Some(65535),
        brightness: Some(255),
    };
    assert_eq!(
        DPTColorTemperatureTransition::new(value)
            .unwrap()
            .as_bytes(),
        [0, 2, 255, 255, 255, 7]
    );
    assert_eq!(
        Telegram::group_write_value::<DPTColorTemperatureTransition>(
            GroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        [0, 2, 255, 255, 255, 7]
    );
    for fade in [-1.0, 6553.6, f64::NAN, f64::INFINITY] {
        assert!(
            DPTColorTemperatureTransition::new(ColorTemperatureTransition {
                fade_time: Some(fade),
                ..value
            })
            .is_err()
        );
    }
    assert!(DPTColorTemperatureTransition::decode(&[0; 5]).is_err());
}
