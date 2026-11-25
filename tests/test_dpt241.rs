#![cfg(feature = "dpt")]
use knust::protocol::dpt::{DPTStatusSAB, DptPayload, DptRegistry, DptType, DptValue};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn shutter_status_bits_and_position_validity_round_trip() {
    let registry = DptRegistry::new();
    assert_eq!("241.800".parse::<DptType>().ok(), Some(DptType::StatusSAB));
    assert_eq!(
        registry.dpt_number_for_value_type("status_sab").unwrap(),
        "241.800"
    );
    for bits in 0..=u16::MAX {
        let [hi, lo] = bits.to_be_bytes();
        let raw = [128, 255, hi, lo];
        let typed = DPTStatusSAB::decode(&raw).unwrap();
        let value = typed.value();
        assert_eq!(value.height_position, (bits & 0x4000 != 0).then_some(50));
        assert_eq!(value.slats_position, (bits & 0x8000 != 0).then_some(100));
        let flags = [
            value.upper_end_position,
            value.lower_end_position,
            value.lower_predefined_position,
            value.target_position_reached,
            value.target_height_position_restricted,
            value.target_slats_position_restricted,
            value.weather_alarm,
            value.forced,
            value.locked,
            value.local_override,
            value.failure,
        ];
        for (bit, flag) in flags.into_iter().enumerate() {
            assert_eq!(flag, bits & (1 << bit) != 0);
        }
        let encoded = DPTStatusSAB::new(value).unwrap();
        assert_eq!(&encoded.as_bytes()[2..], &(bits & 0xc7ff).to_be_bytes());
        let view = DptType::StatusSAB.decode_ref(&raw).unwrap();
        assert_eq!(DptPayload::from(view), DptPayload::StatusSAB(value));
        assert_eq!(
            DptType::StatusSAB.parse(&value.to_string()).unwrap(),
            encoded.as_bytes()
        );
    }
    let raw = [128, 255, 0xc0, 0];
    assert!(registry.decode("241.800", &raw).is_ok());
    let value = DPTStatusSAB::decode(&raw).unwrap().value();
    assert_eq!(
        Telegram::group_write_value::<DPTStatusSAB>(GroupAddress::try_from_raw(1).unwrap(), value)
            .unwrap()
            .payload,
        raw
    );
    assert!(DPTStatusSAB::decode(&[0; 3]).is_err());
    assert!(DptType::StatusSAB.parse("none,none,invalid").is_err());
}
