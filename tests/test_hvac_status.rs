#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTCombinedStatusHVA, DPTCombinedStatusRTC, DPTCombinedStatusRTSM, DptPayload, DptRegistry,
    DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn combined_hvac_statuses_preserve_known_bits_and_ignore_reserved_bits() {
    let registry = DptRegistry::new();
    for raw in 0..=u16::MAX {
        let bytes = raw.to_be_bytes();
        for (dpt, number) in [
            (DptType::CombinedStatusHVA, "22.102"),
            (DptType::CombinedStatusRTC, "22.103"),
        ] {
            let view = dpt.decode_ref(&bytes).unwrap();
            let payload = DptPayload::from(view);
            assert_eq!(dpt.encode(&payload).unwrap(), (raw & 0x01ff).to_be_bytes());
            assert_eq!(
                dpt.parse(&view.formatted(dpt)).unwrap(),
                (raw & 0x01ff).to_be_bytes()
            );
            assert!(registry.decode(number, &bytes).is_ok());
        }
        let typed = DPTCombinedStatusRTC::decode(&bytes).unwrap();
        assert_eq!(typed.value().heating_mode, raw & 2 != 0);
        assert_eq!(
            DPTCombinedStatusHVA::new(DPTCombinedStatusHVA::decode(&bytes).unwrap().value())
                .as_bytes(),
            (raw & 0x01ff).to_be_bytes()
        );
    }
    for raw in 0..=u8::MAX {
        let bytes = [raw];
        let typed = DPTCombinedStatusRTSM::decode(&bytes).unwrap();
        assert_eq!(
            DPTCombinedStatusRTSM::new(typed.value()).as_bytes(),
            [raw & 0x1f]
        );
        assert_eq!(
            DptPayload::from(DptType::CombinedStatusRTSM.decode_ref(&bytes).unwrap()),
            DptPayload::StatusRTSM(typed.value())
        );
        assert_eq!(
            Telegram::group_write_value::<DPTCombinedStatusRTSM>(
                GroupAddress::try_from_raw(1).unwrap(),
                typed.value()
            )
            .unwrap()
            .payload,
            [raw & 0x1f]
        );
    }
    assert!(DPTCombinedStatusRTSM::decode(&[]).is_err());
    assert!(DPTCombinedStatusRTC::decode(&[0]).is_err());
}
