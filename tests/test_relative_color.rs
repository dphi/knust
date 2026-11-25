#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    ColorTemperatureControl, ControlDimming, DPTColorTemperatureControl, DptPayload, DptRegistry,
    DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn relative_color_components_apply_validity_and_ignore_reserved_bits() {
    let registry = DptRegistry::new();
    for (dpt, number, len, masked) in [
        (DptType::ColorTemperatureControl, "250.600", 3, true),
        (DptType::RelativeControlRGBW, "252.600", 5, true),
        (DptType::RelativeControlXYY, "253.600", 4, true),
        (DptType::RelativeControlRGB, "254.600", 3, false),
    ] {
        assert_eq!(number.parse::<DptType>().ok(), Some(dpt));
        for first in 0..=u8::MAX {
            for flags in 0..=u8::MAX {
                let mut raw = vec![0xf9; len];
                raw[0] = first;
                if masked {
                    raw[len - 1] = flags;
                }
                let view = dpt.decode_ref(&raw).unwrap();
                let payload = DptPayload::from(view);
                let canonical = dpt.encode(&payload).unwrap();
                assert_eq!(dpt.parse(&payload.formatted(None)).unwrap(), canonical);
                assert_eq!(
                    DptPayload::from(dpt.decode_ref(&canonical).unwrap()),
                    payload
                );
                if !masked {
                    break;
                }
            }
        }
        assert!(dpt.decode_ref(&vec![0; len - 1]).is_err());
        assert!(registry.decode(number, &vec![0; len]).is_ok());
    }
    let value = ColorTemperatureControl {
        color_temperature: Some(ControlDimming::new(true, 3).unwrap()),
        brightness: None,
    };
    let typed = DPTColorTemperatureControl::new(value.clone());
    assert_eq!(typed.as_bytes(), [11, 0, 2]);
    assert_eq!(
        DPTColorTemperatureControl::decode(typed.as_bytes())
            .unwrap()
            .value(),
        value
    );
    assert_eq!(
        Telegram::group_write_value::<DPTColorTemperatureControl>(
            GroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        [11, 0, 2]
    );
    assert!(DptType::RelativeControlRGB.parse("--1").is_err());
}
