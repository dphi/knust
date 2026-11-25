#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTColorXYYTransition, DptPayload, DptRegistry, DptType, DptValue, XYYColorTransition,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn xyy_transition_validity_rounding_and_bounds() {
    let reg = DptRegistry::new();
    assert_eq!(
        "243.600".parse::<DptType>().ok(),
        Some(DptType::XYYColorTransition)
    );
    assert_eq!(
        reg.dpt_number_for_value_type("color_xyy_transition")
            .unwrap(),
        "243.600"
    );
    for flags in 0..=u8::MAX {
        let raw = [0, 15, 0, 0, 255, 255, 200, flags];
        let value = XYYColorTransition {
            fade_time: 1.5,
            color: (flags & 2 != 0).then_some((0.0, 1.0)),
            brightness: (flags & 1 != 0).then_some(200),
        };
        assert_eq!(DPTColorXYYTransition::decode(&raw).unwrap().value(), value);
        let view = DptType::XYYColorTransition.decode_ref(&raw).unwrap();
        assert_eq!(
            DptPayload::from(view),
            DptPayload::XYYColorTransition(value)
        );
        let canonical = DPTColorXYYTransition::new(value).unwrap();
        assert_eq!(canonical.as_bytes()[7], flags & 3);
        assert_eq!(
            DptType::XYYColorTransition
                .parse(&value.to_string())
                .unwrap(),
            canonical.as_bytes()
        );
    }
    let value = XYYColorTransition {
        fade_time: 0.25,
        color: Some((0.5, 1.0)),
        brightness: Some(255),
    };
    assert_eq!(
        DPTColorXYYTransition::new(value).unwrap().as_bytes(),
        [0, 2, 128, 0, 255, 255, 255, 3]
    );
    assert_eq!(
        Telegram::group_write_value::<DPTColorXYYTransition>(
            GroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        [0, 2, 128, 0, 255, 255, 255, 3]
    );
    for invalid in [
        XYYColorTransition {
            fade_time: f64::NAN,
            ..value
        },
        XYYColorTransition {
            fade_time: 6553.6,
            ..value
        },
        XYYColorTransition {
            color: Some((-0.1, 0.0)),
            ..value
        },
        XYYColorTransition {
            color: Some((0.0, f64::INFINITY)),
            ..value
        },
    ] {
        assert!(DPTColorXYYTransition::new(invalid).is_err());
    }
    assert!(
        DptType::XYYColorTransition
            .parse("1,0.5,none,none")
            .is_err()
    );
    assert!(DPTColorXYYTransition::decode(&[0; 7]).is_err());
}
