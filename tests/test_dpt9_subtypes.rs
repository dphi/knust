#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTPartsPerMillion, DPTTemperature, DPTTemperatureDifference2Byte, DptPayload, DptType,
    DptValue,
};
use knust::protocol::{Telegram, address::RawGroupAddress};

#[test]
fn subtype_ranges_and_failed_writes_are_never_silently_zeroed() {
    let address = RawGroupAddress::try_from_raw(1).unwrap();
    for value in [f32::NAN, f32::INFINITY, -300.0] {
        assert!(DPTTemperature::new(value).is_err());
        assert!(Telegram::group_write_value::<DPTTemperature>(address, value).is_err());
    }
    assert!(DPTTemperatureDifference2Byte::new(-300.0).is_ok());
    assert!(
        Telegram::group_write_value::<DPTTemperatureDifference2Byte>(address, -300.0f32).is_ok()
    );
    assert!(DPTPartsPerMillion::new(-1.0).is_err());
    assert!(Telegram::group_write_value::<DPTPartsPerMillion>(address, -1.0f32).is_err());
    let raw = DPTTemperatureDifference2Byte::new(-300.0)
        .unwrap()
        .as_bytes()
        .to_vec();
    assert!(DPTTemperature::decode(&raw).is_err());
    assert!(DptType::Temperature.decode_ref(&raw).is_err());
    assert!(DptType::TemperatureDifference2Byte.decode_ref(&raw).is_ok());
    assert!(
        DptType::PartsPerMillion
            .encode(&DptPayload::Float(-1.0))
            .is_err()
    );
    assert!(DptType::PartsPerMillion.parse("-1").is_err());
    for raw in 0..=u16::MAX {
        let bytes = raw.to_be_bytes();
        assert_eq!(
            DPTTemperature::decode(&bytes).is_ok(),
            DptType::Temperature.decode_ref(&bytes).is_ok()
        );
        assert_eq!(
            DPTTemperatureDifference2Byte::decode(&bytes).is_ok(),
            DptType::TemperatureDifference2Byte
                .decode_ref(&bytes)
                .is_ok()
        );
        assert_eq!(
            DPTPartsPerMillion::decode(&bytes).is_ok(),
            DptType::PartsPerMillion.decode_ref(&bytes).is_ok()
        );
    }
    assert!(DPTPartsPerMillion::new(-0.001).is_err());
    assert!(DPTTemperature::new(-273.0).is_ok());
    assert!(knust::protocol::dpt::DPTTemperatureF::new(-459.6).is_ok());
    assert!(DPTTemperatureDifference2Byte::new(-671_088.6).is_ok());
    assert!(DPTTemperature::new(670_433.28).is_ok());
}
