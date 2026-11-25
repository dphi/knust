#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTDeviceControl, DPTStatusGen, DeviceControl, DptPayload, DptRegistry, DptType, DptValue,
    GeneralStatus,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn general_and_device_status_preserve_known_bits() {
    let reg = DptRegistry::new();
    for (dpt, number, mask) in [
        (DptType::GeneralStatus, "21.001", 31),
        (DptType::DeviceControl, "21.002", 7),
    ] {
        assert_eq!(number.parse::<DptType>().ok(), Some(dpt));
        for raw in 0..=u8::MAX {
            let bytes = [raw];
            let view = dpt.decode_ref(&bytes).unwrap();
            let payload = DptPayload::from(view);
            assert_eq!(dpt.encode(&payload).unwrap(), [raw & mask]);
            assert_eq!(dpt.parse(&view.formatted(dpt)).unwrap(), [raw & mask]);
            assert!(reg.decode(number, &bytes).is_ok());
        }
        assert!(dpt.decode_ref(&[]).is_err());
    }
    let status = GeneralStatus {
        out_of_service: true,
        fault: false,
        overridden: true,
        in_alarm: false,
        alarm_un_ack: true,
    };
    assert_eq!(DPTStatusGen::new(status).as_bytes(), [21]);
    assert_eq!(DPTStatusGen::decode(&[0xf5]).unwrap().value(), status);
    assert_eq!(
        Telegram::group_write_value::<DPTStatusGen>(GroupAddress::try_from_raw(1).unwrap(), status)
            .unwrap()
            .payload,
        [21]
    );
    let control = DeviceControl {
        user_stopped: true,
        own_ia: false,
        verify_mode: true,
    };
    assert_eq!(DPTDeviceControl::new(control).as_bytes(), [5]);
    assert!(
        DptType::DeviceControl
            .encode(&DptPayload::GeneralStatus(status))
            .is_err()
    );
}
