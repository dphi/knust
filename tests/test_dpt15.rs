#![cfg(feature = "dpt")]
use knust::protocol::dpt::{AccessData, DPTAccessData, DptPayload, DptRegistry, DptType, DptValue};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn access_data_encodes_decimal_digits_and_validates() {
    let registry = DptRegistry::new();
    assert_eq!("15.000".parse::<DptType>().ok(), Some(DptType::AccessData));
    assert_eq!(
        registry.dpt_number_for_value_type("access_data").unwrap(),
        "15.000"
    );
    for code in [0, 1, 123, 123456, 999999] {
        for flags in 0..=u8::MAX {
            let value = AccessData {
                access_code: code,
                detection_error: flags & 128 != 0,
                permission: flags & 64 != 0,
                read_right_to_left: flags & 32 != 0,
                encrypted: flags & 16 != 0,
                index: flags & 15,
            };
            let typed = DPTAccessData::new(value).unwrap();
            let raw = typed.as_bytes();
            assert_eq!(raw[3], flags);
            assert_eq!(DPTAccessData::decode(raw).unwrap().value(), value);
            let view = DptType::AccessData.decode_ref(raw).unwrap();
            assert_eq!(DptPayload::from(view), DptPayload::AccessData(value));
            assert_eq!(DptType::AccessData.parse(&value.to_string()).unwrap(), raw);
            assert!(registry.decode("15.000", raw).is_ok());
        }
    }
    let value = AccessData {
        access_code: 123456,
        detection_error: true,
        permission: false,
        read_right_to_left: true,
        encrypted: false,
        index: 3,
    };
    assert_eq!(
        DPTAccessData::new(value).unwrap().as_bytes(),
        [0x12, 0x34, 0x56, 0xa3]
    );
    assert!(!format!("{value:?}").contains("123456"));
    assert!(!format!("{:?}", DPTAccessData::new(value).unwrap()).contains("123456"));
    assert!(DPTAccessData::decode(&[0xfa, 0, 0, 0]).is_err());
    assert!(DptType::AccessData.decode_ref(&[0, 0x0a, 0, 0]).is_err());
    assert!(
        DPTAccessData::new(AccessData {
            access_code: 1_000_000,
            ..value
        })
        .is_err()
    );
    assert!(
        Telegram::group_write_value::<DPTAccessData>(
            GroupAddress::try_from_raw(1).unwrap(),
            AccessData { index: 16, ..value }
        )
        .is_err()
    );
}
