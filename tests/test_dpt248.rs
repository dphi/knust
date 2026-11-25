#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    ConverterInfoFix, DPTConverterInfoFix, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn fixed_converter_info_validity_levels_and_feature_bits() {
    let registry = DptRegistry::new();
    assert_eq!(
        "248.600".parse::<DptType>().ok(),
        Some(DptType::ConverterInfoFix)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("converter_info_fix")
            .unwrap(),
        "248.600"
    );
    for validity in 0..=u8::MAX {
        for flags in 0..=u8::MAX {
            let raw = [validity, 255, 254, 123, flags];
            let typed = DPTConverterInfoFix::decode(&raw);
            let view = DptType::ConverterInfoFix.decode_ref(&raw);
            assert_eq!(typed.is_ok(), validity <= 2);
            assert_eq!(view.is_ok(), validity <= 2);
            if let Ok(typed) = typed {
                let value = typed.value();
                assert_eq!(value.emergency_min_level, None);
                assert_eq!(value.emergency_max_level, Some(254));
                assert_eq!(
                    DptPayload::from(view.unwrap()),
                    DptPayload::ConverterInfoFix(value)
                );
                assert_eq!(DPTConverterInfoFix::new(value).unwrap().as_bytes(), raw);
                assert_eq!(
                    DptType::ConverterInfoFix.parse(&value.to_string()).unwrap(),
                    raw
                );
            }
        }
    }
    let raw = [1, 0, 255, 2, 0x81];
    let value = DPTConverterInfoFix::decode(&raw).unwrap().value();
    assert!(value.integral_emergency_control_gear && value.relight_in_rest_mode_supported);
    assert_eq!(
        Telegram::group_write_value::<DPTConverterInfoFix>(
            GroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        raw
    );
    assert!(registry.decode("248.600", &raw).is_ok());
    assert!(
        DPTConverterInfoFix::new(ConverterInfoFix {
            emergency_min_level: Some(255),
            ..value
        })
        .is_err()
    );
    assert!(DPTConverterInfoFix::decode(&[0; 4]).is_err());
}
