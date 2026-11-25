#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTDoubleNibble, DoubleNibble, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn repetitions_validate_all_octets_and_round_trip() {
    let registry = DptRegistry::new();
    assert_eq!(
        "25.1000".parse::<DptType>().ok(),
        Some(DptType::DoubleNibble)
    );
    assert_eq!(
        registry.dpt_number_for_value_type("double_nibble").unwrap(),
        "25.1000"
    );
    for byte in 0..=u8::MAX {
        let raw = [byte];
        let decoded = DPTDoubleNibble::decode(&raw);
        let view = DptType::DoubleNibble.decode_ref(&raw);
        let valid = byte >> 4 <= 3 && byte & 15 <= 3;
        assert_eq!(decoded.is_ok(), valid);
        assert_eq!(view.is_ok(), valid);
        if valid {
            let value = DoubleNibble {
                busy: byte >> 4,
                nak: byte & 15,
            };
            assert_eq!(decoded.unwrap().value(), value);
            assert_eq!(
                DptPayload::from(view.unwrap()),
                DptPayload::DoubleNibble(value)
            );
            assert_eq!(
                DptType::DoubleNibble
                    .encode(&DptPayload::DoubleNibble(value))
                    .unwrap(),
                raw
            );
            assert_eq!(
                DptType::DoubleNibble.parse(&value.to_string()).unwrap(),
                raw
            );
            assert!(registry.decode("25.1000", &raw).is_ok());
            let telegram = Telegram::group_write_value::<DPTDoubleNibble>(
                GroupAddress::try_from_raw(1).unwrap(),
                value,
            )
            .unwrap();
            assert!(!telegram.compact_payload);
            assert_eq!(telegram.payload, raw);
        }
    }
    assert!(DPTDoubleNibble::new(DoubleNibble { busy: 4, nak: 0 }).is_err());
    assert!(DPTDoubleNibble::decode(&[]).is_err());
    assert!(DptType::DoubleNibble.parse("1,4").is_err());
}
