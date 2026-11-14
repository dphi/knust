#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    CombinedInfoOnOff, DPTCombinedInfoOnOff, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn output_masks_and_states_round_trip() {
    let registry = DptRegistry::new();
    assert_eq!(
        DptType::from_str("27.001"),
        Some(DptType::CombinedInfoOnOff)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("combined_info_on_off")
            .unwrap(),
        "27.001"
    );
    for bit in 0..16 {
        for state in [None, Some(false), Some(true)] {
            let mut outputs = [None; 16];
            outputs[bit] = state;
            let value = CombinedInfoOnOff { outputs };
            let typed = DPTCombinedInfoOnOff::new(value);
            let mask = if state.is_some() {
                1u32 << (16 + bit)
            } else {
                0
            };
            let bits = if state == Some(true) { 1u32 << bit } else { 0 };
            assert_eq!(typed.as_bytes(), (mask | bits).to_be_bytes());
            assert_eq!(
                DPTCombinedInfoOnOff::decode(typed.as_bytes())
                    .unwrap()
                    .value(),
                value
            );
            let view = DptType::CombinedInfoOnOff
                .decode_ref(typed.as_bytes())
                .unwrap();
            assert_eq!(DptPayload::from(view), DptPayload::CombinedInfoOnOff(value));
            assert_eq!(
                DptType::CombinedInfoOnOff
                    .parse(&value.to_string())
                    .unwrap(),
                typed.as_bytes()
            );
            assert!(registry.decode("27.001", typed.as_bytes()).is_ok());
            let telegram = Telegram::group_write_value::<DPTCombinedInfoOnOff>(
                GroupAddress::try_from_raw(1).unwrap(),
                value,
            )
            .unwrap();
            assert_eq!(telegram.payload, typed.as_bytes());
        }
    }
    let raw = [0, 0, 255, 255];
    let value = DPTCombinedInfoOnOff::decode(&raw).unwrap().value();
    assert_eq!(value.outputs, [None; 16]);
    assert_eq!(DPTCombinedInfoOnOff::new(value).as_bytes(), [0; 4]);
    assert!(DPTCombinedInfoOnOff::decode(&[0; 3]).is_err());
    assert!(DptType::CombinedInfoOnOff.parse("not valid").is_err());
}
