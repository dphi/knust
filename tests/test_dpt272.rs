#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    ConverterInfo, DPTConverterInfo, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::RawGroupAddress};

#[test]
fn converter_info_validates_enums_addresses_and_unknown_levels() {
    let registry = DptRegistry::new();
    assert_eq!(
        "272.600".parse::<DptType>().ok(),
        Some(DptType::ConverterInfo)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("converter_info")
            .unwrap(),
        "272.600"
    );
    for valid in 0..=u8::MAX {
        for address in 0..=u8::MAX {
            let raw = [valid, 0x12, 0x34, 0xab, 0xcd, 255, address];
            let expected = valid <= 2 && address <= 63;
            let typed = DPTConverterInfo::decode(&raw);
            let view = DptType::ConverterInfo.decode_ref(&raw);
            assert_eq!(typed.is_ok(), expected);
            assert_eq!(view.is_ok(), expected);
            if expected {
                let value = typed.unwrap().value();
                assert_eq!(value.emergency_level, None);
                assert_eq!(value.lamp_emergency_time, 0x1234);
                assert_eq!(value.lamp_total_operation_time, 0xabcd);
                assert_eq!(
                    DptPayload::from(view.unwrap()),
                    DptPayload::ConverterInfo(value)
                );
                assert_eq!(DPTConverterInfo::new(value).unwrap().as_bytes(), raw);
                assert_eq!(
                    DptType::ConverterInfo.parse(&value.to_string()).unwrap(),
                    raw
                );
            }
        }
    }
    let value = ConverterInfo {
        info_valid: 1,
        lamp_emergency_time: 0,
        lamp_total_operation_time: u16::MAX,
        emergency_level: Some(254),
        short_address: 63,
    };
    let raw = [1, 0, 0, 255, 255, 254, 63];
    assert_eq!(DPTConverterInfo::new(value).unwrap().as_bytes(), raw);
    assert_eq!(
        Telegram::group_write_value::<DPTConverterInfo>(
            RawGroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        raw
    );
    assert!(registry.decode("272.600", &raw).is_ok());
    assert!(
        DPTConverterInfo::new(ConverterInfo {
            emergency_level: Some(255),
            ..value
        })
        .is_err()
    );
    assert!(DPTConverterInfo::decode(&[0; 6]).is_err());
}
