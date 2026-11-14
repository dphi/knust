#![cfg(feature = "dpt")]
use knust::protocol::dpt::{
    DPTGeographicalLocation, DptPayload, DptRegistry, DptType, DptValue, GeographicalLocation,
};
use knust::protocol::{Telegram, address::GroupAddress};

#[test]
fn coordinates_validate_and_round_trip_wire_values() {
    let registry = DptRegistry::new();
    assert_eq!(
        DptType::from_str("255.001"),
        Some(DptType::GeographicalLocation)
    );
    assert_eq!(
        registry
            .dpt_number_for_value_type("geographical_location")
            .unwrap(),
        "255.001"
    );
    for (longitude, latitude) in [(0.0, 0.0), (-180.0, -90.0), (180.0, 90.0), (13.405, 52.52)] {
        let value = GeographicalLocation {
            longitude,
            latitude,
        };
        let typed = DPTGeographicalLocation::new(value).unwrap();
        let mut raw = longitude.to_be_bytes().to_vec();
        raw.extend(latitude.to_be_bytes());
        assert_eq!(typed.as_bytes(), raw);
        assert_eq!(
            DPTGeographicalLocation::decode(&raw).unwrap().value(),
            value
        );
        let view = DptType::GeographicalLocation.decode_ref(&raw).unwrap();
        assert_eq!(
            DptPayload::from(view),
            DptPayload::GeographicalLocation(value)
        );
        assert_eq!(
            DptType::GeographicalLocation
                .parse(&value.to_string())
                .unwrap(),
            raw
        );
        assert!(registry.decode("255.001", &raw).is_ok());
        let telegram = Telegram::group_write_value::<DPTGeographicalLocation>(
            GroupAddress::try_from_raw(1).unwrap(),
            value,
        )
        .unwrap();
        assert_eq!(telegram.payload, raw);
    }
    for value in [
        GeographicalLocation {
            longitude: f32::NAN,
            latitude: 0.0,
        },
        GeographicalLocation {
            longitude: 181.0,
            latitude: 0.0,
        },
        GeographicalLocation {
            longitude: 0.0,
            latitude: f32::INFINITY,
        },
        GeographicalLocation {
            longitude: 0.0,
            latitude: -91.0,
        },
    ] {
        assert!(DPTGeographicalLocation::new(value).is_err());
        let mut raw = value.longitude.to_be_bytes().to_vec();
        raw.extend(value.latitude.to_be_bytes());
        assert!(DptType::GeographicalLocation.decode_ref(&raw).is_err());
    }
    assert!(DPTGeographicalLocation::decode(&[0; 7]).is_err());
}
