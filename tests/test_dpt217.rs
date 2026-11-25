#![cfg(feature = "dpt")]

use knust::protocol::address::RawGroupAddress;
use knust::protocol::dpt::{DPTVersion, DptPayload, DptRegistry, DptType, DptValue, KnxVersion};
use knust::protocol::{GroupValueService, Telegram};

#[test]
fn version_wire_vectors_and_sentinel() {
    for (magic_number, version_number, revision_number, raw) in [
        (0, 0, 0, [0x00, 0x00]),
        (1, 2, 3, [0x08, 0x83]),
        (31, 0, 0, [0xf8, 0x00]),
        (0, 31, 0, [0x07, 0xc0]),
        (0, 0, 63, [0x00, 0x3f]),
        (31, 31, 63, [0xff, 0xff]),
    ] {
        let value = KnxVersion {
            magic_number,
            version_number,
            revision_number,
        };
        let typed = DPTVersion::new(value).unwrap();
        assert_eq!(typed.as_bytes(), raw);
        assert_eq!(typed.value(), value);
        assert_eq!(
            DptType::Version
                .encode(&DptPayload::Version(value))
                .unwrap(),
            raw
        );
        assert_eq!(DptType::Version.parse(&value.to_string()).unwrap(), raw);
        assert_eq!(
            DptPayload::Version(value).formatted(None),
            value.to_string()
        );
        assert_eq!(value.is_invalid(), raw == [0xff, 0xff]);
    }
    assert_eq!(
        DPTVersion::new(KnxVersion::invalid()).unwrap().as_bytes(),
        [0xff, 0xff]
    );
}

#[test]
fn version_decodes_every_wire_value_without_losing_bits() {
    for raw in 0..=u16::MAX {
        let bytes = raw.to_be_bytes();
        let typed = DPTVersion::decode(&bytes).unwrap();
        let value = typed.value();
        assert_eq!(value.magic_number, (raw >> 11) as u8);
        assert_eq!(value.version_number, ((raw >> 6) & 31) as u8);
        assert_eq!(value.revision_number, (raw & 63) as u8);
        let view = DptType::Version.decode_ref(&bytes).unwrap();
        assert_eq!(view.raw().as_ptr(), bytes.as_ptr());
        assert_eq!(DptPayload::from(view), DptPayload::Version(value));
        assert_eq!(view.formatted(DptType::Version), value.to_string());
        assert_eq!(
            DptType::Version.encode(&DptPayload::from(view)).unwrap(),
            bytes
        );
        assert_eq!(view.as_f64(), None);
        assert_eq!(view.as_bool(), None);
    }
}

#[test]
fn version_rejects_malformed_values_and_registers_typed_writes() {
    for value in [
        KnxVersion {
            magic_number: 32,
            version_number: 0,
            revision_number: 0,
        },
        KnxVersion {
            magic_number: 0,
            version_number: 32,
            revision_number: 0,
        },
        KnxVersion {
            magic_number: 0,
            version_number: 0,
            revision_number: 64,
        },
        KnxVersion {
            magic_number: 255,
            version_number: 255,
            revision_number: 255,
        },
    ] {
        assert!(DPTVersion::new(value).is_err());
        assert!(
            DptType::Version
                .encode(&DptPayload::Version(value))
                .is_err()
        );
        assert!(
            Telegram::group_write_value::<DPTVersion>(
                RawGroupAddress::try_from_raw(1).unwrap(),
                value
            )
            .is_err()
        );
    }
    for input in [
        "", "1", "1.2", "1.2.3.4", "1..3", "-1.2.3", "1.2.256", "32.0.0", "0.32.0", "0.0.64",
        "1.2.a",
    ] {
        assert!(DptType::Version.parse(input).is_err());
    }
    for bytes in [&[][..], &[0][..], &[0; 3][..]] {
        assert!(DPTVersion::from_bytes(bytes).is_err());
        assert!(DptType::Version.decode_ref(bytes).is_err());
    }
    assert!(
        DptType::Version
            .encode(&DptPayload::UnsignedInt(1))
            .is_err()
    );
    let registry = DptRegistry::new();
    assert_eq!("217.001".parse::<DptType>().ok(), Some(DptType::Version));
    assert_eq!(DPTVersion::dpt_type().unwrap(), DptType::Version);
    assert_eq!(
        registry.dpt_number_for_value_type("version").unwrap(),
        "217.001"
    );
    assert_eq!(registry.metadata("217.001").unwrap().byte_length, 2);
    let decoded = registry.decode("217.001", &[0x08, 0x83]).unwrap();
    let typed = decoded.downcast_ref::<DPTVersion>().unwrap();
    assert_eq!(registry.encode("217.001", typed).unwrap(), [0x08, 0x83]);
    let address = RawGroupAddress::try_from_raw(1).unwrap();
    let telegram = Telegram::group_write_value::<DPTVersion>(address, "1.2.3").unwrap();
    assert_eq!(telegram.payload, [0x08, 0x83]);
    assert!(!telegram.compact_payload);
    assert_eq!(
        GroupValueService::Write(telegram.payload).encode_with_compact_payload(false),
        [0x80, 0x08, 0x83]
    );
}
