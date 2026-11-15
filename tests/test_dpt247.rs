#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    ConverterTestInfo, DPTConverterTestInfo, DateTime, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn converter_test_info_reuses_date_time_flags_and_encodes_updated_fields() {
    let reg = DptRegistry::new();
    assert_eq!(
        DptType::from_str("247.600"),
        Some(DptType::ConverterTestInfo)
    );
    assert_eq!(
        reg.dpt_number_for_value_type("converter_test_info")
            .unwrap(),
        "247.600"
    );
    let mut date = DateTime::new(125, 11, 15, 6, 10, 20, 30).unwrap();
    date.hour = 11;
    let value = ConverterTestInfo {
        test_start: date,
        automated_test_interval: 65535,
        test_execution_time: 42,
    };
    let typed = DPTConverterTestInfo::new(value.clone()).unwrap();
    assert_eq!(
        typed.as_bytes(),
        [125, 11, 15, 0xcb, 20, 30, 0, 0, 255, 255, 0, 42]
    );
    let decoded = DPTConverterTestInfo::decode(typed.as_bytes())
        .unwrap()
        .value();
    assert_eq!(decoded.test_start.hour, 11);
    let view = DptType::ConverterTestInfo
        .decode_ref(typed.as_bytes())
        .unwrap();
    assert_eq!(
        DptPayload::from(view),
        DptPayload::ConverterTestInfo(decoded.clone())
    );
    assert_eq!(
        DptType::ConverterTestInfo
            .parse(&decoded.to_string())
            .unwrap(),
        typed.as_bytes()
    );
    assert!(reg.decode("247.600", typed.as_bytes()).is_ok());
    assert_eq!(
        Telegram::group_write_value::<DPTConverterTestInfo>(
            GroupAddress::try_from_raw(1).unwrap(),
            value.clone()
        )
        .unwrap()
        .payload,
        typed.as_bytes()
    );
    let mut invalid = value;
    invalid.test_start.month = 13;
    assert!(DPTConverterTestInfo::new(invalid).is_err());
    let mut raw = typed.as_bytes().to_vec();
    raw[1] = 13;
    assert!(DPTConverterTestInfo::decode(&raw).is_err());
    assert!(DptType::ConverterTestInfo.decode_ref(&raw).is_err());
    assert!(DPTConverterTestInfo::decode(&[0; 11]).is_err());
}
