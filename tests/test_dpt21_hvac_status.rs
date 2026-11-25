#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTChannelActivation8, DPTChannelActivation16, DPTForceSign, DPTForceSignCool, DPTFuelTypeSet,
    DPTLightActuatorErrorInfo, DPTMedia, DPTPhasesStatus, DPTRFFilterInfo, DPTRFModeInfo,
    DPTSecurityReport, DPTStatusAHU, DPTStatusDHWC, DPTStatusRCC, DPTStatusRHC, DPTStatusRHCC,
    DPTStatusSDHWC, DPTVirtualDryContact, DptPayload, DptRegistry, DptType, DptValue, RFModeInfo,
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
        let dpt = $number.parse::<DptType>().unwrap();
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

macro_rules! check_typed_status16 {
    ($wrapper:ty, $number:literal, $mask:expr, $fields:expr) => {{
        let dpt = $number.parse::<DptType>().unwrap();
        let registry = DptRegistry::new();
        assert_eq!(registry.metadata($number).unwrap().byte_length, 2);
        for raw in 0..=u16::MAX {
            let bytes = raw.to_be_bytes();
            let canonical = (raw & $mask).to_be_bytes();
            let value = <$wrapper>::decode(&bytes).unwrap().value();
            assert_eq!(value.to_string().len(), $fields);
            assert_eq!(<$wrapper>::new(value).as_bytes(), canonical);
            assert_eq!(
                Telegram::group_write_value::<$wrapper>(
                    GroupAddress::try_from_raw(1).unwrap(),
                    value
                )
                .unwrap()
                .payload,
                canonical
            );
            assert!(
                registry
                    .decode($number, &bytes)
                    .unwrap()
                    .downcast_ref::<$wrapper>()
                    .is_some()
            );
            let view = dpt.decode_ref(&bytes).unwrap();
            let payload = DptPayload::from(view);
            assert_eq!(dpt.encode(&payload).unwrap(), canonical);
            assert_eq!(dpt.parse(&view.formatted(dpt)).unwrap(), canonical);
        }
        assert!(<$wrapper>::decode(&[]).is_err());
        assert!(<$wrapper>::decode(&[0, 0, 0]).is_err());
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

#[test]
fn rf_filter_security_and_channel_activation_preserve_defined_bits() {
    check_typed_status!(DPTRFFilterInfo, "21.1001", 0x07);
    check_typed_status!(DPTSecurityReport, "21.1002", 0x01);
    check_typed_status!(DPTChannelActivation8, "21.1010", 0xff);
}

#[test]
fn dry_contact_and_phases_status_preserve_defined_bits() {
    check_typed_status!(DPTVirtualDryContact, "21.1200", 0xff);
    check_typed_status!(DPTPhasesStatus, "21.1201", 0x07);
}

#[test]
fn rf_mode_info_decodes_invalid_received_flag_but_rejects_writes() {
    let dpt = "21.1000".parse::<DptType>().unwrap();
    let registry = DptRegistry::new();
    assert!(RFModeInfo::default().asynchronous);

    for raw in 0..=u8::MAX {
        let bytes = [raw];
        let typed = DPTRFModeInfo::decode(&bytes).unwrap();
        let value = typed.value();
        assert_eq!(value.asynchronous, raw & 1 != 0);
        assert_eq!(value.bibat_master, raw & 2 != 0);
        assert_eq!(value.bibat_slave, raw & 4 != 0);
        assert!(
            registry
                .decode("21.1000", &bytes)
                .unwrap()
                .downcast_ref::<DPTRFModeInfo>()
                .is_some()
        );

        let view = dpt.decode_ref(&bytes).unwrap();
        let payload = DptPayload::from(view);
        if raw & 1 == 0 {
            assert!(typed.encode().is_err());
            assert!(registry.encode("21.1000", &typed).is_err());
            assert!(DPTRFModeInfo::new(value).is_err());
            assert!(dpt.encode(&payload).is_err());
            assert!(dpt.parse(&view.formatted(dpt)).is_err());
            assert!(
                Telegram::group_write_value::<DPTRFModeInfo>(
                    GroupAddress::try_from_raw(1).unwrap(),
                    value
                )
                .is_err()
            );
        } else {
            let canonical = [raw & 0x07];
            assert_eq!(typed.encode().unwrap(), canonical);
            assert_eq!(registry.encode("21.1000", &typed).unwrap(), canonical);
            assert_eq!(dpt.encode(&payload).unwrap(), canonical);
            assert_eq!(dpt.parse(&view.formatted(dpt)).unwrap(), canonical);
            assert_eq!(
                Telegram::group_write_value::<DPTRFModeInfo>(
                    GroupAddress::try_from_raw(1).unwrap(),
                    value
                )
                .unwrap()
                .payload,
                canonical
            );
        }
    }

    assert!(DPTRFModeInfo::decode(&[]).is_err());
    assert!(DPTRFModeInfo::decode(&[0, 1]).is_err());
    assert!("000".parse::<RFModeInfo>().unwrap().encode().is_err());
    assert!("1000".parse::<RFModeInfo>().is_err());
}

#[test]
fn dhw_media_and_channel_statuses_preserve_defined_bits() {
    check_typed_status16!(DPTStatusDHWC, "22.100", 0x00ff, 8);
    check_typed_status16!(DPTMedia, "22.1000", 0x0036, 4);
    check_typed_status16!(DPTChannelActivation16, "22.1010", 0xffff, 16);
    check_typed_status16!(DPTStatusRHCC, "22.101", 0x7fff, 15);
}
