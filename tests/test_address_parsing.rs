#![cfg(feature = "dpt")]
use knust::protocol::address::{GroupAddress, IndividualAddress};
use knust::protocol::dpt::DptType;

#[test]
fn address_and_dpt_parsers_reject_non_ascii_digits_and_trailing_newlines() {
    assert!("²/2/3".parse::<GroupAddress>().is_err());
    assert!("1/2/3\n".parse::<GroupAddress>().is_err());
    assert!("1.².3".parse::<IndividualAddress>().is_err());
    assert!("1.2.3\n".parse::<IndividualAddress>().is_err());
    assert_eq!(DptType::from_str("9.001"), DptType::from_str("9.1"));
    assert_eq!(DptType::from_str("9.001\n"), None);
    assert_eq!(DptType::from_str("TPD-9"), None);
}
