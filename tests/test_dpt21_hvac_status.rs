#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTForceSign, DPTForceSignCool, DPTFuelTypeSet, DPTLightActuatorErrorInfo, DPTStatusAHU,
    DPTStatusRCC, DPTStatusRHC, DPTStatusSDHWC, DptPayload, DptRegistry, DptType, DptValue,
};
use knust::protocol::{Telegram, address::GroupAddress};

macro_rules! check_typed_status {
    ($wrapper:ty, $number:literal, $mask:expr) => {{
        let wrapper = <$wrapper>::decode(&[0xff]).unwrap();
        assert_eq!(
            wrapper.value().to_string().len(),
            ($mask as u8).count_ones() as usize
        );
        assert_eq!(<$wrapper>::new(wrapper.value()).as_bytes(), &[$mask]);
        let dpt = DptType::from_str($number).unwrap();
        let registry = DptRegistry::new();
        assert_eq!(registry.metadata($number).unwrap().byte_length, 1);
        for raw in 0..=u8::MAX {
            let bytes = [raw];
            let value = <$wrapper>::decode(&[raw]).unwrap();
            assert_eq!(<$wrapper>::new(value.value()).as_bytes(), &[raw & $mask]);
            let telegram = Telegram::group_write_value::<$wrapper>(
                GroupAddress::try_from_raw(1).unwrap(),
                value.value(),
            )
            .unwrap();
            assert_eq!(telegram.payload, [raw & $mask]);
            let any = registry.decode($number, &[raw]).unwrap();
            assert!(any.downcast_ref::<$wrapper>().is_some());

            let view = dpt.decode_ref(&bytes).unwrap();
            let payload = DptPayload::from(view);
            assert_eq!(dpt.encode(&payload).unwrap(), [raw & $mask]);
            assert_eq!(dpt.parse(&view.formatted(dpt)).unwrap(), [raw & $mask]);
        }
        assert!(<$wrapper>::decode(&[]).is_err());
        assert!(<$wrapper>::decode(&[0, 1]).is_err());
    }};
}

#[test]
fn hvac_controller_status_subtypes_preserve_defined_bits() {
    check_typed_status!(DPTForceSign, "21.100", 0xff);
    check_typed_status!(DPTForceSignCool, "21.101", 0x01);
    check_typed_status!(DPTStatusRHC, "21.102", 0xff);
    check_typed_status!(DPTStatusSDHWC, "21.103", 0x07);
    check_typed_status!(DPTFuelTypeSet, "21.104", 0x07);
    check_typed_status!(DPTStatusRCC, "21.105", 0x01);
    check_typed_status!(DPTStatusAHU, "21.106", 0x0f);
}

#[test]
fn light_actuator_error_info_preserves_defined_bits() {
    check_typed_status!(DPTLightActuatorErrorInfo, "21.601", 0x7f);
}
