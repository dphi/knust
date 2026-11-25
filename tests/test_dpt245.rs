#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    ConverterTestResult, DPTConverterTestResult, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::RawGroupAddress};

#[test]
fn converter_results_validate_fields_and_preserve_unknown_charge() {
    let registry = DptRegistry::new();
    assert_eq!(
        "245.600".parse::<DptType>().ok(),
        Some(DptType::ConverterTestResult)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("converter_test_result")
            .unwrap(),
        "245.600"
    );
    for first in 0..=u8::MAX {
        for methods in 0..=u8::MAX {
            let raw = [first, 0x5f, methods, 1, 0xfe, 255];
            let valid = first >> 4 <= 5
                && first & 15 <= 5
                && methods >> 6 <= 2
                && (methods >> 4) & 3 <= 2
                && (methods >> 2) & 3 <= 2;
            let typed = DPTConverterTestResult::decode(&raw);
            let view = DptType::ConverterTestResult.decode_ref(&raw);
            assert_eq!(typed.is_ok(), valid);
            assert_eq!(view.is_ok(), valid);
            if valid {
                let value = typed.unwrap().value();
                assert_eq!(value.duration_test_discharge_time, 510);
                assert_eq!(value.partial_duration_test_charge_level, None);
                assert_eq!(
                    DptPayload::from(view.unwrap()),
                    DptPayload::ConverterTestResult(value)
                );
                let canonical = [first, 0x50, methods & 0xfc, 1, 0xfe, 255];
                assert_eq!(
                    DPTConverterTestResult::new(value).unwrap().as_bytes(),
                    canonical
                );
                assert_eq!(
                    DptType::ConverterTestResult
                        .parse(&value.to_string())
                        .unwrap(),
                    canonical
                );
            }
        }
    }
    let value = ConverterTestResult {
        function_test_result: 1,
        duration_test_result: 2,
        partial_duration_test_result: 3,
        function_test_start_method: 1,
        duration_test_start_method: 2,
        partial_duration_test_start_method: 0,
        duration_test_discharge_time: 42,
        partial_duration_test_charge_level: Some(254),
    };
    let raw = [0x12, 0x30, 0x60, 0, 42, 254];
    assert_eq!(DPTConverterTestResult::new(value).unwrap().as_bytes(), raw);
    assert_eq!(
        Telegram::group_write_value::<DPTConverterTestResult>(
            RawGroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        raw
    );
    assert!(registry.decode("245.600", &raw).is_ok());
    assert!(
        DPTConverterTestResult::new(ConverterTestResult {
            duration_test_discharge_time: 511,
            ..value
        })
        .is_err()
    );
    assert!(
        DPTConverterTestResult::new(ConverterTestResult {
            partial_duration_test_charge_level: Some(255),
            ..value
        })
        .is_err()
    );
    assert!(DPTConverterTestResult::decode(&[0; 5]).is_err());
}
