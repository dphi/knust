#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTForecastTemperature, DptPayload, DptRegistry, DptType, DptValue, Forecast,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn forecast_validity_sentinels_and_subtype_ranges() {
    let reg = DptRegistry::new();
    for (dpt, number) in [
        (DptType::ForecastTemperature, "273.001"),
        (DptType::ForecastWindSpeed, "273.002"),
        (DptType::ForecastRelativeHumidity, "273.003"),
        (DptType::ForecastAbsoluteHumidity, "273.004"),
        (DptType::ForecastCO2, "273.005"),
        (DptType::ForecastAirPollutants, "273.006"),
        (DptType::ForecastSunIntensity, "273.007"),
    ] {
        assert_eq!(DptType::from_str(number), Some(dpt));
        for mask in 0..16 {
            let value = Forecast {
                delay_time: (mask & 8 != 0).then_some(60),
                probability: (mask & 4 != 0).then_some(50),
                maximum_value: (mask & 2 != 0).then_some(20.0),
                minimum_value: (mask & 1 != 0).then_some(10.0),
            };
            let payload = DptPayload::Forecast(value);
            let raw = dpt.encode(&payload).unwrap();
            assert_eq!(raw[0], mask);
            if value.maximum_value.is_none() {
                assert_eq!(&raw[4..6], [0x7f, 0xff]);
            }
            if value.minimum_value.is_none() {
                assert_eq!(&raw[6..8], [0x7f, 0xff]);
            }
            let view = dpt.decode_ref(&raw).unwrap();
            assert_eq!(DptPayload::from(view), payload);
            assert_eq!(dpt.parse(&view.formatted(dpt)).unwrap(), raw);
            assert!(reg.decode(number, &raw).is_ok());
        }
        assert!(dpt.decode_ref(&[0; 7]).is_err());
        assert!(dpt.decode_ref(&[8, 0, 0, 0, 0, 0, 0, 0]).is_err());
        let sentinel = [3, 0, 0, 0, 0x7f, 0xff, 0x7f, 0xff];
        assert_eq!(
            DptPayload::from(dpt.decode_ref(&sentinel).unwrap()),
            DptPayload::Forecast(Forecast {
                delay_time: None,
                probability: None,
                maximum_value: None,
                minimum_value: None
            })
        );
    }
    let invalid = Forecast {
        delay_time: Some(0),
        probability: Some(101),
        maximum_value: Some(1.0),
        minimum_value: Some(2.0),
    };
    assert!(DPTForecastTemperature::new(invalid).is_err());
    let below_zero = Forecast {
        delay_time: None,
        probability: None,
        maximum_value: Some(-1.0),
        minimum_value: None,
    };
    assert!(
        DptType::ForecastWindSpeed
            .encode(&DptPayload::Forecast(below_zero))
            .is_err()
    );
    assert!(
        DptType::ForecastTemperature
            .encode(&DptPayload::Forecast(below_zero))
            .is_ok()
    );
    let value = Forecast {
        delay_time: Some(1),
        probability: Some(100),
        maximum_value: Some(10.0),
        minimum_value: Some(10.0),
    };
    let raw = DPTForecastTemperature::new(value).unwrap();
    assert_eq!(
        DPTForecastTemperature::decode(raw.as_bytes())
            .unwrap()
            .value(),
        value
    );
    assert_eq!(
        Telegram::group_write_value::<DPTForecastTemperature>(
            GroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        raw.as_bytes()
    );
}
