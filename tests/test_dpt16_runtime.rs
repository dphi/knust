#![cfg(feature = "dpt")]

use knust::protocol::dpt::{DptPayload, DptType, DptValue, StringAscii, StringLatin1};

#[test]
fn runtime_strings_match_typed_codecs() {
    for input in ["é", "  café  ", "éééééééééééééé", "A\0B", "😀"] {
        for (dpt, expected) in [
            (
                DptType::StringAscii,
                StringAscii::new(input).unwrap().as_bytes().to_vec(),
            ),
            (
                DptType::StringLatin1,
                StringLatin1::new(input).unwrap().as_bytes().to_vec(),
            ),
        ] {
            assert_eq!(
                dpt.encode(&DptPayload::String(input.into())).unwrap(),
                expected
            );
            assert_eq!(dpt.parse(input).unwrap(), expected);
            let view = dpt.decode_ref(&expected).unwrap();
            let typed = if dpt == DptType::StringAscii {
                StringAscii::from_bytes(&expected)
                    .unwrap()
                    .value()
                    .to_owned()
            } else {
                StringLatin1::from_bytes(&expected)
                    .unwrap()
                    .value()
                    .to_owned()
            };
            assert_eq!(view.formatted(dpt), typed);
            assert_eq!(DptPayload::from(view), DptPayload::String(typed));
        }
    }
    let mut raw = [0; 14];
    raw[..3].copy_from_slice(&[0xe9, 0, b'A']);
    assert_eq!(
        DptType::StringAscii
            .decode_ref(&raw)
            .unwrap()
            .formatted(DptType::StringAscii),
        "�A"
    );
    assert_eq!(
        DptType::StringLatin1
            .decode_ref(&raw)
            .unwrap()
            .formatted(DptType::StringLatin1),
        "éA"
    );
    for dpt in [DptType::StringAscii, DptType::StringLatin1] {
        assert!(dpt.encode(&DptPayload::String("a".repeat(15))).is_err());
        assert!(dpt.parse(&"a".repeat(15)).is_err());
    }
}
