#![cfg(feature = "dpt")]
use knust::protocol::dpt::DptType;
use knust::{GroupAddress, IndividualAddress};

#[test]
fn address_and_dpt_parsers_reject_non_ascii_digits_and_trailing_newlines() {
    assert!("²/2/3".parse::<GroupAddress>().is_err());
    assert!("1/2/3\n".parse::<GroupAddress>().is_err());
    assert!("1.².3".parse::<IndividualAddress>().is_err());
    assert!("1.2.3\n".parse::<IndividualAddress>().is_err());
    assert!("9.001\n".parse::<DptType>().is_err());
    assert!("TPD-9".parse::<DptType>().is_err());
}

#[test]
fn dpt_numbers_round_trip_through_standard_rust_traits() {
    for (input, canonical) in [("9.1", "9.001"), ("25.1000", "25.1000")] {
        let dpt: DptType = input.parse().unwrap();
        assert_eq!(dpt.to_string(), canonical);
        assert_eq!(canonical.parse::<DptType>().unwrap(), dpt);
    }
    assert_eq!(DptType::from_number(u16::MAX, u16::MAX), None);
    assert!("65535.65535".parse::<DptType>().is_err());
}
