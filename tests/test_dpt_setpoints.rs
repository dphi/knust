#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTRoomTemperatureSetpointShifts3, DPTRoomTemperatureSetpointShifts4,
    DPTRoomTemperatureSetpoints3, DPTRoomTemperatureSetpoints4, DptPayload, DptRegistry, DptType,
    DptValue, RoomTemperatureSetpoints3, RoomTemperatureSetpoints4,
};
use knust::protocol::{Telegram, address::RawGroupAddress};

#[test]
fn optional_temperature_fields_and_subtype_bounds() {
    let a = RoomTemperatureSetpoints3 {
        comfort: Some(21.5),
        standby: None,
        economy: Some(18.0),
    };
    let b = RoomTemperatureSetpoints4 {
        comfort: a.comfort,
        standby: a.standby,
        economy: a.economy,
        building_protection: Some(8.0),
    };
    let reg = DptRegistry::new();
    let cases = [
        (
            DptType::RoomTemperatureSetpoints3,
            "222.100",
            DptPayload::Setpoints3(a),
        ),
        (
            DptType::RoomTemperatureSetpointShifts3,
            "222.101",
            DptPayload::Setpoints3(a),
        ),
        (
            DptType::RoomTemperatureSetpoints4,
            "275.100",
            DptPayload::Setpoints4(b),
        ),
        (
            DptType::RoomTemperatureSetpointShifts4,
            "275.101",
            DptPayload::Setpoints4(b),
        ),
    ];
    for (dpt, number, payload) in cases {
        assert_eq!(number.parse::<DptType>().ok(), Some(dpt));
        let raw = dpt.encode(&payload).unwrap();
        assert_eq!(&raw[2..4], [0x7f, 0xff]);
        let view = dpt.decode_ref(&raw).unwrap();
        assert_eq!(DptPayload::from(view), payload);
        assert_eq!(dpt.parse(&view.formatted(dpt)).unwrap(), raw);
        assert!(reg.decode(number, &raw).is_ok());
        assert!(dpt.decode_ref(&raw[..raw.len() - 1]).is_err());
    }
    let delta = RoomTemperatureSetpoints3 {
        comfort: Some(-300.0),
        standby: None,
        economy: None,
    };
    assert!(DPTRoomTemperatureSetpoints3::new(delta).is_err());
    assert!(DPTRoomTemperatureSetpointShifts3::new(delta).is_ok());
    let delta4 = RoomTemperatureSetpoints4 {
        comfort: Some(-300.0),
        standby: None,
        economy: None,
        building_protection: None,
    };
    assert!(DPTRoomTemperatureSetpoints4::new(delta4).is_err());
    assert!(DPTRoomTemperatureSetpointShifts4::new(delta4).is_ok());
    assert!(
        DptType::RoomTemperatureSetpoints3
            .parse("NaN,none,none")
            .is_err()
    );
    for mask in 0..16 {
        let fields = std::array::from_fn::<_, 4, _>(|bit| {
            if mask & (1 << bit) == 0 {
                None
            } else {
                Some((bit + 1) as f64)
            }
        });
        let value = RoomTemperatureSetpoints4 {
            comfort: fields[0],
            standby: fields[1],
            economy: fields[2],
            building_protection: fields[3],
        };
        let typed = DPTRoomTemperatureSetpoints4::new(value).unwrap();
        assert_eq!(typed.value(), value);
        for (bit, field) in typed.as_bytes().chunks_exact(2).enumerate() {
            assert_eq!(field == [0x7f, 0xff], fields[bit].is_none());
        }
    }
    let address = RawGroupAddress::try_from_raw(1).unwrap();
    let telegram = Telegram::group_write_value::<DPTRoomTemperatureSetpoints4>(address, b).unwrap();
    assert_eq!(
        telegram.payload,
        DPTRoomTemperatureSetpoints4::new(b).unwrap().as_bytes()
    );
}
