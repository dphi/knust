#![cfg(feature = "dpt")]

use knust::protocol::dpt::{
    CharacterAscii, CharacterLatin1, DPTCharacter, DPTCharacterLatin1, DptPayload, DptRegistry,
    DptType, DptValue, WriteValue,
};

#[test]
fn character_encoding_matches_xknx_vectors() {
    for (input, ascii, latin1) in [
        ("", 0x00, 0x00),
        ("\0", 0x00, 0x00),
        ("A", 0x41, 0x41),
        (" ", 0x20, 0x20),
        ("\n", 0x0a, 0x0a),
        ("\u{7f}", 0x7f, 0x7f),
        ("é", b'?', 0xe9),
        ("ÿ", b'?', 0xff),
        ("€", b'?', b'?'),
        ("😀", b'?', b'?'),
    ] {
        assert_eq!(DPTCharacter::new(input).unwrap().as_bytes(), [ascii]);
        assert_eq!(DPTCharacterLatin1::new(input).unwrap().as_bytes(), [latin1]);
        for (dpt, expected) in [
            (DptType::Character, ascii),
            (DptType::CharacterLatin1, latin1),
        ] {
            assert_eq!(
                dpt.encode(&DptPayload::String(input.into())).unwrap(),
                [expected]
            );
            assert_eq!(dpt.parse(input).unwrap(), [expected]);
        }
    }
    let value: CharacterLatin1 = "é".resolve(DptType::CharacterLatin1).unwrap();
    assert_eq!(value.value(), "é");
}

#[test]
fn all_character_octets_decode_consistently() {
    for byte in 0..=u8::MAX {
        let raw = [byte];
        for (dpt, expected) in [
            (
                DptType::Character,
                match byte {
                    0 => String::new(),
                    1..=127 => char::from(byte).to_string(),
                    _ => "�".into(),
                },
            ),
            (
                DptType::CharacterLatin1,
                if byte == 0 {
                    String::new()
                } else {
                    char::from(byte).to_string()
                },
            ),
        ] {
            let view = dpt.decode_ref(&raw).unwrap();
            assert_eq!(view.raw().as_ptr(), raw.as_ptr());
            assert_eq!(view.formatted(dpt), expected);
            assert_eq!(DptPayload::from(view), DptPayload::String(expected.clone()));
            assert_eq!(view.as_f64(), None);
            assert_eq!(view.as_bool(), None);
            let typed = if dpt == DptType::Character {
                CharacterAscii::from_bytes(&raw).unwrap().value()
            } else {
                CharacterLatin1::from_bytes(&raw).unwrap().value()
            };
            assert_eq!(typed, expected);
        }
    }
}

#[test]
fn character_length_validation_and_registry() {
    let registry = DptRegistry::new();
    for (dpt, name) in [
        (DptType::Character, "character"),
        (DptType::CharacterLatin1, "character_latin_1"),
    ] {
        let number = dpt.number_str();
        assert_eq!(DptType::from_str(&number), Some(dpt));
        assert_eq!(dpt.byte_length(), 1);
        assert_eq!(registry.metadata(&number).unwrap().byte_length, 1);
        assert_eq!(registry.dpt_number_for_value_type(name).unwrap(), number);
        assert!(dpt.encode(&DptPayload::UnsignedInt(65)).is_err());
        for input in ["AB", "éé", "A ", "  ", "e\u{301}"] {
            assert!(dpt.parse(input).is_err());
            assert!(dpt.encode(&DptPayload::String(input.into())).is_err());
            assert!(CharacterAscii::new(input).is_err());
            assert!(CharacterLatin1::new(input).is_err());
        }
        for raw in [&[][..], &[0, 1][..], &[0; 14][..]] {
            assert!(dpt.decode_ref(raw).is_err());
            assert!(registry.decode(&number, raw).is_err());
            assert!(CharacterAscii::decode(raw).is_err());
            assert!(CharacterLatin1::decode(raw).is_err());
        }
    }
    assert_eq!(CharacterAscii::dpt_type().unwrap(), DptType::Character);
    assert_eq!(
        CharacterLatin1::dpt_type().unwrap(),
        DptType::CharacterLatin1
    );
    let ascii = registry.decode("4.001", &[0x41]).unwrap();
    assert_eq!(ascii.downcast_ref::<CharacterAscii>().unwrap().value(), "A");
    let latin1 = registry.decode("4.002", &[0xe9]).unwrap();
    assert_eq!(
        latin1.downcast_ref::<CharacterLatin1>().unwrap().value(),
        "é"
    );
    assert_eq!(
        registry
            .encode("4.002", &CharacterLatin1::new("é").unwrap())
            .unwrap(),
        [0xe9]
    );
}
