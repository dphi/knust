#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    BatteryInfo, DPTBatteryInfo, DptPayload, DptRegistry, DptType, DptValue, DptView,
    dpt246::BatteryInfoView,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn battery_info_view_borrows_the_input() {
    let bytes = [0x04, 0xff];
    let DptView::BatteryInfo(view) = DptType::BatteryInfo.decode_ref(&bytes).unwrap() else {
        panic!("expected battery information");
    };
    let view: BatteryInfoView<'_> = view;
    let value: BatteryInfo = view.value();
    assert!(std::ptr::eq(view.raw(), bytes.as_slice()));
    assert!(value.battery_fully_charged);
    assert_eq!(value.charge_level, None);
    assert!(DptType::BatteryInfo.decode_ref(&bytes[..1]).is_err());
}

#[test]
fn battery_info_decodes_unknown_levels_and_reserved_flags() {
    let registry = DptRegistry::new();
    assert_eq!(
        "246.600".parse::<DptType>().ok(),
        Some(DptType::BatteryInfo)
    );
    assert_eq!(
        registry.dpt_number_for_value_type("battery_info").unwrap(),
        "246.600"
    );
    for raw in 0..=u16::MAX {
        let bytes = raw.to_be_bytes();
        let value = BatteryInfo {
            battery_failure: bytes[0] & 1 != 0,
            battery_duration_failure: bytes[0] & 2 != 0,
            battery_fully_charged: bytes[0] & 4 != 0,
            charge_level: (bytes[1] != 255).then_some(bytes[1]),
        };
        assert_eq!(DPTBatteryInfo::decode(&bytes).unwrap().value(), value);
        let view = DptType::BatteryInfo.decode_ref(&bytes).unwrap();
        assert_eq!(DptPayload::from(view), DptPayload::BatteryInfo(value));
        let canonical = [bytes[0] & 7, bytes[1]];
        assert_eq!(DPTBatteryInfo::new(value).unwrap().as_bytes(), canonical);
        assert_eq!(
            DptType::BatteryInfo.parse(&value.to_string()).unwrap(),
            canonical
        );
    }
    let value = BatteryInfo {
        battery_failure: false,
        battery_duration_failure: true,
        battery_fully_charged: false,
        charge_level: None,
    };
    assert_eq!(
        Telegram::group_write_value::<DPTBatteryInfo>(
            GroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        [2, 255]
    );
    assert!(
        DPTBatteryInfo::new(BatteryInfo {
            charge_level: Some(255),
            ..value
        })
        .is_err()
    );
    assert!(DPTBatteryInfo::decode(&[0]).is_err());
}
