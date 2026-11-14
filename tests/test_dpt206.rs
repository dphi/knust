#![cfg(feature = "dpt")]
use knust::protocol::dpt::{DPTHVACModeNext, DptPayload, DptRegistry, DptType, DptValue, ModeNext};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn next_modes_validate_per_subtype_and_preserve_undefined_delay() {
    let reg = DptRegistry::new();
    for (dpt, number, max, zero_is_none) in [
        (DptType::HVACModeNext, "206.100", 4, true),
        (DptType::DHWModeNext, "206.102", 4, true),
        (DptType::OccupancyModeNext, "206.104", 2, false),
        (DptType::BuildingModeNext, "206.105", 2, false),
    ] {
        assert_eq!(DptType::from_str(number), Some(dpt));
        for mode in 0..=u8::MAX {
            for delay in [0u16, 1, 65535] {
                let [hi, lo] = delay.to_be_bytes();
                let raw = [hi, lo, mode];
                let view = dpt.decode_ref(&raw);
                assert_eq!(view.is_ok(), mode <= max);
                if let Ok(view) = view {
                    let value = ModeNext {
                        mode: if zero_is_none && mode == 0 {
                            None
                        } else {
                            Some(mode)
                        },
                        delay_time: (delay != 0).then_some(delay),
                    };
                    assert_eq!(DptPayload::from(view), DptPayload::ModeNext(value));
                    assert_eq!(dpt.encode(&DptPayload::ModeNext(value)).unwrap(), raw);
                    assert_eq!(dpt.parse(&value.to_string()).unwrap(), raw);
                    assert!(reg.decode(number, &raw).is_ok());
                }
            }
        }
        assert!(dpt.decode_ref(&[0; 2]).is_err());
        assert!(
            dpt.encode(&DptPayload::ModeNext(ModeNext {
                mode: if zero_is_none { None } else { Some(0) },
                delay_time: Some(0)
            }))
            .is_err()
        );
    }
    let value = ModeNext {
        mode: Some(4),
        delay_time: Some(12),
    };
    let typed = DPTHVACModeNext::new(value).unwrap();
    assert_eq!(typed.as_bytes(), [0, 12, 4]);
    assert_eq!(
        DPTHVACModeNext::decode(typed.as_bytes()).unwrap().value(),
        value
    );
    assert_eq!(
        Telegram::group_write_value::<DPTHVACModeNext>(
            GroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        typed.as_bytes()
    );
    assert!(
        DPTHVACModeNext::new(ModeNext {
            mode: Some(0),
            delay_time: None
        })
        .is_err()
    );
}
