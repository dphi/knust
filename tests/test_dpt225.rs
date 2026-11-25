#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTScalingSpeed, DPTScalingStepTime, DptPayload, DptRegistry, DptType, DptValue, ScalingTime,
};
use knust::protocol::{Telegram, address::RawGroupAddress};

#[test]
fn scaling_times_validate_writes_but_decode_zero_wire_values() {
    let registry = DptRegistry::new();
    for (dpt, number, resolution) in [
        (DptType::ScalingSpeed, "225.001", 100u32),
        (DptType::ScalingStepTime, "225.002", 1u32),
    ] {
        assert_eq!(number.parse::<DptType>().ok(), Some(dpt));
        for ticks in [1u32, 2, 255, 65535] {
            for percent in 1..=100 {
                let value = ScalingTime {
                    time_period: ticks * resolution,
                    percent,
                };
                let raw = dpt.encode(&DptPayload::ScalingTime(value)).unwrap();
                assert_eq!(&raw[..2], (ticks as u16).to_be_bytes());
                let view = dpt.decode_ref(&raw).unwrap();
                assert_eq!(DptPayload::from(view), DptPayload::ScalingTime(value));
                assert_eq!(dpt.parse(&value.to_string()).unwrap(), raw);
                assert!(registry.decode(number, &raw).is_ok());
            }
        }
        assert!(dpt.decode_ref(&[0; 3]).is_ok());
        assert!(
            dpt.encode(&DptPayload::ScalingTime(ScalingTime {
                time_period: 0,
                percent: 1
            }))
            .is_err()
        );
        assert!(
            dpt.encode(&DptPayload::ScalingTime(ScalingTime {
                time_period: resolution,
                percent: 0
            }))
            .is_err()
        );
        assert!(
            dpt.encode(&DptPayload::ScalingTime(ScalingTime {
                time_period: resolution,
                percent: 101
            }))
            .is_err()
        );
        assert!(dpt.decode_ref(&[0; 2]).is_err());
    }
    let value = ScalingTime {
        time_period: 1000,
        percent: 50,
    };
    assert_eq!(
        DPTScalingSpeed::new(value).unwrap().as_bytes(),
        [0, 10, 128]
    );
    assert_eq!(
        DPTScalingStepTime::new(value).unwrap().as_bytes(),
        [3, 232, 128]
    );
    assert_eq!(
        Telegram::group_write_value::<DPTScalingSpeed>(
            RawGroupAddress::try_from_raw(1).unwrap(),
            value
        )
        .unwrap()
        .payload,
        [0, 10, 128]
    );
}
