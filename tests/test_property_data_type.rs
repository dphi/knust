use knust::protocol::PropertyDataType;

#[test]
fn property_data_types_cover_standard_codes_and_sizes() {
    let reserved = (0x26..=0x2e).chain(0x37..=0x3b).collect::<Vec<_>>();
    for raw in 0..=0x3f {
        match PropertyDataType::try_from(raw) {
            Ok(pdt) => assert_eq!(pdt as u8, raw),
            Err(value) => {
                assert_eq!(value, raw);
                assert!(reserved.contains(&raw));
            }
        }
    }
    assert!(PropertyDataType::try_from(0x40).is_err());

    for (raw, size) in [
        (0x01, Some(1)),
        (0x02, Some(1)),
        (0x03, Some(2)),
        (0x04, Some(2)),
        (0x05, Some(2)),
        (0x06, Some(3)),
        (0x07, Some(3)),
        (0x08, Some(4)),
        (0x09, Some(4)),
        (0x0a, Some(4)),
        (0x0b, Some(8)),
        (0x0c, Some(10)),
        (0x0d, Some(3)),
        (0x0e, Some(5)),
        (0x0f, Some(8)),
        (0x10, None),
        (0x11, Some(1)),
        (0x24, Some(20)),
        (0x25, None),
        (0x2f, None),
        (0x30, Some(2)),
        (0x31, Some(6)),
        (0x32, Some(1)),
        (0x33, Some(1)),
        (0x34, Some(2)),
        (0x35, Some(1)),
        (0x36, Some(1)),
        (0x3c, None),
        (0x3d, None),
        (0x3e, None),
        (0x3f, None),
    ] {
        assert_eq!(
            PropertyDataType::try_from(raw).unwrap().size(),
            size,
            "PDT {raw:#04x}"
        );
    }
    for raw in 0x12..0x24 {
        assert_eq!(
            PropertyDataType::try_from(raw).unwrap().size(),
            Some(usize::from(raw - 0x10))
        );
    }
}
