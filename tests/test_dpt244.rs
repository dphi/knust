#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    ConverterStatus, DPTConverterStatus, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn converter_status_validates_enums_and_ignores_reserved_bits() {
    let registry = DptRegistry::new();
    assert_eq!(DptType::from_str("244.600"), Some(DptType::ConverterStatus));
    assert_eq!(
        registry
            .dpt_number_for_value_type("converter_status")
            .unwrap(),
        "244.600"
    );
    for raw in 0..=u16::MAX {
        let bytes = raw.to_be_bytes();
        let valid = bytes[0] >> 4 <= 9
            && bytes[1] >> 6 <= 2
            && (bytes[1] >> 4) & 3 <= 2
            && (bytes[1] >> 2) & 3 <= 2
            && bytes[1] & 3 <= 2;
        let typed = DPTConverterStatus::decode(&bytes);
        let view = DptType::ConverterStatus.decode_ref(&bytes);
        assert_eq!(typed.is_ok(), valid);
        assert_eq!(view.is_ok(), valid);
        if valid {
            let value = typed.unwrap().value();
            assert_eq!(
                DptPayload::from(view.unwrap()),
                DptPayload::ConverterStatus(value)
            );
            let canonical = [bytes[0] & 0xf3, bytes[1]];
            assert_eq!(
                DPTConverterStatus::new(value).unwrap().as_bytes(),
                canonical
            );
            assert_eq!(
                DptType::ConverterStatus.parse(&value.to_string()).unwrap(),
                canonical
            );
        }
    }
    let value = ConverterStatus {
        mode: 5,
        hardwired_inhibit_active: true,
        hardwired_switch_on: false,
        function_test_pending: 2,
        duration_test_pending: 1,
        partial_duration_test_pending: 0,
        failure: 2,
    };
    assert_eq!(
        DPTConverterStatus::new(value).unwrap().as_bytes(),
        [0x51, 0x92]
    );
    assert_eq!(
        Telegram::group_write_value::<DPTConverterStatus>(
            GroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        [0x51, 0x92]
    );
    assert!(DPTConverterStatus::new(ConverterStatus { mode: 10, ..value }).is_err());
    assert!(DPTConverterStatus::decode(&[0]).is_err());
}
