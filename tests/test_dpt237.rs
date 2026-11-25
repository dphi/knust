#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DALIControlGearDiagnostic, DPTDALIControlGearDiagnostic, DptPayload, DptRegistry, DptType,
    DptValue,
};
use knust::protocol::{Telegram, address::RawGroupAddress};

#[test]
fn dali_gear_validates_group_addresses_and_ignores_reserved_bits() {
    let registry = DptRegistry::new();
    assert_eq!(
        "237.600".parse::<DptType>().ok(),
        Some(DptType::DALIControlGearDiagnostic)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("dali_control_gear_diagnostic")
            .unwrap(),
        "237.600"
    );
    for raw in 0..=u16::MAX {
        let bytes = raw.to_be_bytes();
        let valid = bytes[1] & 64 == 0 || bytes[1] & 63 <= 15;
        let typed = DPTDALIControlGearDiagnostic::decode(&bytes);
        let view = DptType::DALIControlGearDiagnostic.decode_ref(&bytes);
        assert_eq!(typed.is_ok(), valid);
        assert_eq!(view.is_ok(), valid);
        if valid {
            let value = DALIControlGearDiagnostic {
                address: bytes[1] & 63,
                group: bytes[1] & 64 != 0,
                read_request: bytes[1] & 128 != 0,
                lamp_failure: bytes[0] & 1 != 0,
                ballast_failure: bytes[0] & 2 != 0,
                converter_error: bytes[0] & 4 != 0,
            };
            assert_eq!(typed.unwrap().value(), value);
            assert_eq!(
                DptPayload::from(view.unwrap()),
                DptPayload::DALIControlGearDiagnostic(value)
            );
            assert_eq!(
                DptType::DALIControlGearDiagnostic
                    .parse(&value.to_string())
                    .unwrap(),
                [bytes[0] & 7, bytes[1]]
            );
            assert!(registry.decode("237.600", &bytes).is_ok());
        }
    }
    let invalid = DALIControlGearDiagnostic {
        address: 16,
        group: true,
        read_request: false,
        lamp_failure: false,
        ballast_failure: false,
        converter_error: false,
    };
    assert!(DPTDALIControlGearDiagnostic::new(invalid).is_err());
    assert!(
        Telegram::group_write_value::<DPTDALIControlGearDiagnostic>(
            RawGroupAddress::try_from_raw(1).unwrap(),
            invalid
        )
        .is_err()
    );
    assert!(DPTDALIControlGearDiagnostic::decode(&[0]).is_err());
}
