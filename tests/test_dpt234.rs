#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTLanguageCodeAlpha2ASCII, DPTRegionCodeAlpha2ASCII, DptPayload, DptRegistry, DptType,
    DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn language_and_region_codes_validate_and_normalize() {
    let registry = DptRegistry::new();
    for (dpt, number, kind, expected) in [
        (
            DptType::LanguageCodeAlpha2,
            "234.001",
            "language_code_alpha2",
            "de",
        ),
        (
            DptType::RegionCodeAlpha2,
            "234.002",
            "region_code_alpha2",
            "DE",
        ),
    ] {
        assert_eq!(number.parse::<DptType>().ok(), Some(dpt));
        assert_eq!(registry.dpt_number_for_value_type(kind).unwrap(), number);
        for input in ["de", "DE", "De", "dE"] {
            let view = dpt.decode_ref(input.as_bytes()).unwrap();
            assert_eq!(view.raw(), input.as_bytes());
            assert_eq!(view.formatted(dpt), expected);
            assert_eq!(DptPayload::from(view), DptPayload::String(expected.into()));
            assert_eq!(dpt.parse(input).unwrap(), expected.as_bytes());
            assert!(registry.decode(number, input.as_bytes()).is_ok());
        }
        for input in ["", "a", "abc", "éa", "1a", " a", " de "] {
            assert!(dpt.parse(input).is_err());
            assert!(dpt.decode_ref(input.as_bytes()).is_err());
        }
    }
    assert_eq!(
        DPTLanguageCodeAlpha2ASCII::decode(b"DE").unwrap().value(),
        "de"
    );
    assert_eq!(
        DPTRegionCodeAlpha2ASCII::decode(b"de").unwrap().value(),
        "DE"
    );
    let address = GroupAddress::try_from_raw(1).unwrap();
    assert_eq!(
        Telegram::group_write_value::<DPTRegionCodeAlpha2ASCII>(address, "de")
            .unwrap()
            .payload,
        b"DE"
    );
    assert!(Telegram::group_write_value::<DPTRegionCodeAlpha2ASCII>(address, "1a").is_err());
}
