#![cfg(feature = "dpt")]
use knust::protocol::dpt::{AlarmInfo, DPTAlarmInfo, DptPayload, DptRegistry, DptType, DptValue};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn alarm_info_checks_priority_area_and_ignores_reserved_bits() {
    let reg = DptRegistry::new();
    assert_eq!("219.001".parse::<DptType>().ok(), Some(DptType::AlarmInfo));
    assert_eq!(
        reg.dpt_number_for_value_type("alarm_info").unwrap(),
        "219.001"
    );
    for priority in 0..=u8::MAX {
        for area in 0..=u8::MAX {
            let raw = [7, priority, area, 42, 0xff, 0xff];
            let valid = priority <= 3 && matches!(area, 0 | 1 | 10..=14 | 20 | 30 | 40 | 50);
            let typed = DPTAlarmInfo::decode(&raw);
            let view = DptType::AlarmInfo.decode_ref(&raw);
            assert_eq!(typed.is_ok(), valid);
            assert_eq!(view.is_ok(), valid);
            if valid {
                let value = typed.unwrap().value();
                assert_eq!(
                    value.alarm_priority,
                    if priority == 3 { None } else { Some(priority) }
                );
                assert_eq!(
                    DptPayload::from(view.unwrap()),
                    DptPayload::AlarmInfo(value)
                );
                assert_eq!(
                    DPTAlarmInfo::new(value).unwrap().as_bytes(),
                    [7, priority, area, 42, 15, 7]
                );
                assert_eq!(
                    DptType::AlarmInfo.parse(&value.to_string()).unwrap(),
                    [7, priority, area, 42, 15, 7]
                );
            }
        }
    }
    let value = AlarmInfo {
        log_number: 1,
        alarm_priority: None,
        application_area: 10,
        error_class: 255,
        ack_sup: true,
        time_stamp_sup: false,
        alarm_text_sup: false,
        error_code_sup: false,
        in_alarm: true,
        alarm_un_ack: false,
        locked: true,
    };
    assert_eq!(
        DPTAlarmInfo::new(value).unwrap().as_bytes(),
        [1, 3, 10, 255, 1, 5]
    );
    assert_eq!(
        Telegram::group_write_value::<DPTAlarmInfo>(GroupAddress::try_from_raw(1).unwrap(), value)
            .unwrap()
            .payload,
        [1, 3, 10, 255, 1, 5]
    );
    assert!(reg.decode("219.001", &[1, 3, 10, 255, 1, 5]).is_ok());
    assert!(DPTAlarmInfo::decode(&[0; 5]).is_err());
}
