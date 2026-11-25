#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    BurnerControllerStatus, DPTStatusBUC, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::RawGroupAddress};

#[test]
fn actuator_subtypes_validate_percentages_and_masks() {
    let reg = DptRegistry::new();
    for (dpt, number, optional, scaled, mask) in [
        (
            DptType::BurnerControllerStatus,
            "207.100",
            true,
            false,
            15u8,
        ),
        (DptType::LockingSignal, "207.101", true, false, 3),
        (DptType::BoilerControllerDemand, "207.102", false, false, 3),
        (DptType::ActuatorPositionDemand, "207.104", true, false, 15),
        (DptType::ActuatorPositionStatus, "207.105", false, false, 31),
        (DptType::LightingActuatorStatus, "207.600", true, true, 255),
    ] {
        assert_eq!(number.parse::<DptType>().ok(), Some(dpt));
        for raw in 0..=u16::MAX {
            let bytes = raw.to_be_bytes();
            let valid = scaled || (optional && bytes[1] & 1 == 0) || bytes[0] <= 100;
            let view = dpt.decode_ref(&bytes);
            assert_eq!(view.is_ok(), valid);
            if let Ok(view) = view {
                let payload = DptPayload::from(view);
                let canonical = dpt.encode(&payload).unwrap();
                assert_eq!(canonical[1], bytes[1] & mask);
                if optional && bytes[1] & 1 == 0 {
                    assert_eq!(canonical[0], 0);
                }
                assert_eq!(dpt.parse(&payload.formatted(None)).unwrap(), canonical);
            }
        }
        assert!(dpt.decode_ref(&[0]).is_err());
        assert!(reg.decode(number, &[0, 0]).is_ok());
    }
    let value = BurnerControllerStatus {
        relative_power: Some(75),
        fault: true,
        stage_1_active: false,
        stage_2_active: true,
    };
    let typed = DPTStatusBUC::new(value).unwrap();
    assert_eq!(typed.as_bytes(), [75, 11]);
    assert_eq!(
        DPTStatusBUC::decode(typed.as_bytes()).unwrap().value(),
        value
    );
    assert_eq!(
        Telegram::group_write_value::<DPTStatusBUC>(
            RawGroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        typed.as_bytes()
    );
    assert!(
        DPTStatusBUC::new(BurnerControllerStatus {
            relative_power: Some(101),
            ..value
        })
        .is_err()
    );
}
