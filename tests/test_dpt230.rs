#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTMBusAddress, DptPayload, DptRegistry, DptType, DptValue, MBusAddress,
};
use knust::protocol::{Telegram, address::RawGroupAddress};

#[test]
fn mbus_address_fields_round_trip_and_preserve_unknown_media() {
    let registry = DptRegistry::new();
    assert_eq!(
        "230.1000".parse::<DptType>().ok(),
        Some(DptType::MBusAddress)
    );
    assert_eq!(
        registry.dpt_number_for_value_type("mbus_address").unwrap(),
        "230.1000"
    );
    for medium in 0..=u8::MAX {
        let value = MBusAddress {
            manufacturer_id: 0x1234,
            identification_number: 0x89abcdef,
            version: 0x56,
            medium,
        };
        let raw = [0x12, 0x34, 0x89, 0xab, 0xcd, 0xef, 0x56, medium];
        let typed = DPTMBusAddress::new(value);
        assert_eq!(typed.as_bytes(), raw);
        assert_eq!(DPTMBusAddress::decode(&raw).unwrap().value(), value);
        let view = DptType::MBusAddress.decode_ref(&raw).unwrap();
        assert_eq!(DptPayload::from(view), DptPayload::MBusAddress(value));
        assert_eq!(DptType::MBusAddress.parse(&value.to_string()).unwrap(), raw);
        assert!(registry.decode("230.1000", &raw).is_ok());
        let telegram = Telegram::group_write_value::<DPTMBusAddress>(
            RawGroupAddress::try_from_raw(1).unwrap(),
            value,
        )
        .unwrap();
        assert_eq!(telegram.payload, raw);
    }
    assert!(DPTMBusAddress::decode(&[0; 7]).is_err());
    assert!(DptType::MBusAddress.parse("65536,0,0,0").is_err());
    assert!(DptType::MBusAddress.parse("1,2,3,256").is_err());
}
