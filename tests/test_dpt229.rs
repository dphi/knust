#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTMeteringValue, DptPayload, DptRegistry, DptType, DptValue, MeteringValue, MeteringValueUnit,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn metering_values_validate_unit_codes_and_status_flags() {
    let registry = DptRegistry::new();
    assert_eq!(
        "229.001".parse::<DptType>().ok(),
        Some(DptType::MeteringValue)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("metering_value")
            .unwrap(),
        "229.001"
    );
    for code in 0..=u8::MAX {
        let valid = matches!(code,0x00..=0x1f|0x28..=0x57|0x6e|0x80..=0x81|0x88..=0x89|0xa8..=0xa9|0xb0..=0xb1|0xba);
        for flags in [0, 1, 31, 255] {
            let raw = [0x80, 0, 0, 1, code, flags];
            let typed = DPTMeteringValue::decode(&raw);
            let view = DptType::MeteringValue.decode_ref(&raw);
            assert_eq!(typed.is_ok(), valid);
            assert_eq!(view.is_ok(), valid);
            if valid {
                let value = typed.unwrap().value();
                assert_eq!(value.counter_value, i32::MIN + 1);
                assert_eq!(value.fault, flags & 2 != 0);
                let canonical = [0x80, 0, 0, 1, code, flags & 31];
                assert_eq!(DPTMeteringValue::new(value).unwrap().as_bytes(), canonical);
                assert_eq!(
                    DptPayload::from(view.unwrap()),
                    DptPayload::MeteringValue(value)
                );
                assert_eq!(
                    DptType::MeteringValue.parse(&value.to_string()).unwrap(),
                    canonical
                );
                assert!(registry.decode("229.001", &raw).is_ok());
            }
        }
    }
    let value = MeteringValue {
        counter_value: 123,
        unit: MeteringValueUnit::EnergyWh,
        exponent: -3,
        out_of_service: false,
        fault: false,
        overridden: false,
        in_alarm: false,
        alarm_un_ack: false,
    };
    assert_eq!(
        DPTMeteringValue::new(value).unwrap().as_bytes(),
        [0, 0, 0, 123, 0, 0]
    );
    assert_eq!(
        Telegram::group_write_value::<DPTMeteringValue>(
            GroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        [0, 0, 0, 123, 0, 0]
    );
    assert!(
        DPTMeteringValue::new(MeteringValue {
            exponent: 7,
            ..value
        })
        .is_err()
    );
    assert!(DPTMeteringValue::decode(&[0; 5]).is_err());
}
