#![cfg(feature = "dpt")]

use knust::protocol::dpt::{
    DPTChangeoverMode, DPTEnableHCStage, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn modes_validate_and_round_trip_all_wire_values() {
    let registry = DptRegistry::new();
    for (dpt, number, kind, names) in [
        (
            DptType::ChangeoverMode,
            "20.107",
            "changeover_mode",
            &["Auto", "Cooling Only", "Heating Only"][..],
        ),
        (
            DptType::EnableHCStage,
            "20.116",
            "enable_hc_stage",
            &["Disabled", "Stage A", "Stage B", "Both Stages"][..],
        ),
    ] {
        assert_eq!(DptType::from_str(number), Some(dpt));
        assert_eq!(registry.dpt_number_for_value_type(kind).unwrap(), number);
        for raw in 0..=u8::MAX {
            let bytes = [raw];
            let payload = DptPayload::Enum(raw);
            if (raw as usize) < names.len() {
                let view = dpt.decode_ref(&bytes).unwrap();
                assert_eq!(view.formatted(dpt), names[raw as usize]);
                assert_eq!(dpt.parse(names[raw as usize]).unwrap(), bytes);
                assert_eq!(dpt.encode(&payload).unwrap(), bytes);
                assert!(registry.decode(number, &bytes).is_ok());
            } else {
                assert!(dpt.decode_ref(&bytes).is_err());
                assert!(dpt.encode(&payload).is_err());
                assert!(registry.decode(number, &bytes).is_err());
            }
        }
        assert!(dpt.decode_ref(&[]).is_err());
    }
    let address = GroupAddress::try_from_raw(1).unwrap();
    assert_eq!(
        Telegram::group_write_value::<DPTChangeoverMode>(address, 2u8)
            .unwrap()
            .payload,
        [2]
    );
    assert!(Telegram::group_write_value::<DPTChangeoverMode>(address, 3u8).is_err());
    assert_eq!(DPTEnableHCStage::decode(&[3]).unwrap().value(), 3);
    assert!(DPTEnableHCStage::decode(&[4]).is_err());
}
