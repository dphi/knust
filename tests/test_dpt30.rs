#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    ChannelActivation24, DPTChannelActivation24, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn all_channel_bits_use_knx_wire_order() {
    let registry = DptRegistry::new();
    assert_eq!(
        DptType::from_str("30.1010"),
        Some(DptType::ChannelActivation24)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("channel_activation_24")
            .unwrap(),
        "30.1010"
    );
    for bit in 0..24 {
        let mut channels = [false; 24];
        channels[bit] = true;
        let value = ChannelActivation24 { channels };
        let typed = DPTChannelActivation24::new(value);
        assert_eq!(typed.as_bytes(), &(1u32 << bit).to_be_bytes()[1..]);
        assert_eq!(
            DPTChannelActivation24::decode(typed.as_bytes())
                .unwrap()
                .value(),
            value
        );
        let view = DptType::ChannelActivation24
            .decode_ref(typed.as_bytes())
            .unwrap();
        assert_eq!(
            DptPayload::from(view),
            DptPayload::ChannelActivation24(value)
        );
        assert_eq!(
            DptType::ChannelActivation24
                .parse(&value.to_string())
                .unwrap(),
            typed.as_bytes()
        );
        assert!(registry.decode("30.1010", typed.as_bytes()).is_ok());
        let telegram = Telegram::group_write_value::<DPTChannelActivation24>(
            GroupAddress::try_from_raw(1).unwrap(),
            value,
        )
        .unwrap();
        assert_eq!(telegram.payload, typed.as_bytes());
    }
    assert!(DPTChannelActivation24::decode(&[0; 2]).is_err());
    assert!(DptType::ChannelActivation24.parse("invalid").is_err());
}
