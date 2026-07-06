//! Tests for the modern DPT system

use super::*;

#[test]
fn test_basic_zero_copy() {
    use super::dpt1::Switch;
    use super::dpt9::Temperature;

    // Test switch zero-copy
    let switch = Switch::new(true);
    let bytes = switch.as_bytes();
    assert_eq!(bytes, &[0x01]);

    let decoded = Switch::from_bytes(bytes).unwrap();
    assert!(decoded.value());

    // Test temperature zero-copy
    let temp = Temperature::new(20.0).unwrap();
    let bytes = temp.as_bytes();
    assert_eq!(bytes, &[7, 208]);

    let decoded = Temperature::from_bytes(bytes).unwrap();
    assert!((decoded.value() - 20.0).abs() < 0.01);
}

#[test]
fn test_color_rgb_python_parity() {
    let color = ColorRGB::new(1, 2, 3);
    assert_eq!(color.as_bytes(), &[1, 2, 3]);

    let decoded = ColorRGB::from_bytes(&[255, 128, 0]).unwrap();
    assert_eq!(decoded.red, 255);
    assert_eq!(decoded.green, 128);
    assert_eq!(decoded.blue, 0);
    assert_eq!(decoded.as_bytes(), &[255, 128, 0]);

    assert!(ColorRGB::from_bytes(&[1, 2]).is_err());
}

#[test]
fn test_active_energy_8byte_python_parity() {
    let energy = ActiveEnergy8Byte::new(1);
    assert_eq!(energy.value(), 1);
    assert_eq!(energy.as_bytes(), &[0, 0, 0, 0, 0, 0, 0, 1]);

    let negative = ActiveEnergy8Byte::new(-1);
    assert_eq!(
        negative.as_bytes(),
        &[255, 255, 255, 255, 255, 255, 255, 255]
    );

    let decoded = ActiveEnergy8Byte::from_bytes(&[255, 255, 255, 255, 255, 255, 255, 254]).unwrap();
    assert_eq!(decoded.value(), -2);
    assert_eq!(
        decoded.as_bytes(),
        &[255, 255, 255, 255, 255, 255, 255, 254]
    );

    assert!(ActiveEnergy8Byte::from_bytes(&[0, 0, 0, 1]).is_err());
}

#[test]
fn test_color_rgbw_python_parity() {
    let color = ColorRGBW::new(1, 2, 3, 4);
    assert_eq!(color.as_bytes(), &[1, 2, 3, 4, 0, 0x0F]);

    let partial = ColorRGBW::new_with_validity(
        1,
        2,
        3,
        4,
        ColorRGBWValidity {
            red: true,
            green: false,
            blue: true,
            white: false,
        },
    );
    assert_eq!(partial.as_bytes(), &[1, 0, 3, 0, 0, 0x0A]);

    let decoded = ColorRGBW::from_bytes(&[9, 8, 7, 6, 0, 0x05]).unwrap();
    assert_eq!(decoded.red, 9);
    assert_eq!(decoded.green, 8);
    assert_eq!(decoded.blue, 7);
    assert_eq!(decoded.white, 6);
    assert!(!decoded.red_valid);
    assert!(decoded.green_valid);
    assert!(!decoded.blue_valid);
    assert!(decoded.white_valid);
    assert_eq!(decoded.as_bytes(), &[0, 8, 0, 6, 0, 0x05]);

    assert!(ColorRGBW::from_bytes(&[1, 2, 3, 4]).is_err());
}

#[test]
fn test_color_xyy_python_parity() {
    let color = ColorXYY::new(1, 2, 3);
    assert_eq!(color.as_bytes(), &[0, 1, 0, 2, 3, 0x03]);

    let partial = ColorXYY::new_with_validity(1, 2, 3, false, true);
    assert_eq!(partial.as_bytes(), &[0, 0, 0, 0, 3, 0x01]);

    let decoded = ColorXYY::from_bytes(&[0, 10, 0, 20, 30, 0x02]).unwrap();
    assert_eq!(decoded.x, 10);
    assert_eq!(decoded.y, 20);
    assert_eq!(decoded.brightness, 30);
    assert!(decoded.color_valid);
    assert!(!decoded.brightness_valid);
    assert_eq!(decoded.as_bytes(), &[0, 10, 0, 20, 0, 0x02]);

    assert!(ColorXYY::from_bytes(&[0, 1, 0, 2, 3]).is_err());
}

#[test]
fn test_tariff_active_energy_python_parity() {
    let value = TariffActiveEnergy::new(1, 2);
    assert_eq!(value.as_bytes(), &[0, 0, 0, 1, 2, 0]);

    let invalid_energy = TariffActiveEnergy::new_with_validity(1, 2, false, true);
    assert_eq!(invalid_energy.as_bytes(), &[0, 0, 0, 0, 2, 0x02]);

    let decoded = TariffActiveEnergy::from_bytes(&[255, 255, 255, 254, 9, 0x01]).unwrap();
    assert_eq!(decoded.energy, -2);
    assert_eq!(decoded.tariff, 9);
    assert!(decoded.energy_valid);
    assert!(!decoded.tariff_valid);
    assert_eq!(decoded.as_bytes(), &[255, 255, 255, 254, 0, 0x01]);

    assert!(TariffActiveEnergy::from_bytes(&[0, 0, 0, 1, 1]).is_err());
}

#[test]
fn test_dpt16_ascii_padding() {
    let value = StringAscii::new("Hi").unwrap();

    assert_eq!(
        value.as_bytes(),
        &[b'H', b'i', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );

    let decoded = StringAscii::from_bytes(value.as_bytes()).unwrap();
    assert_eq!(decoded.value(), "Hi");
    assert_eq!(decoded.as_bytes(), value.as_bytes());
}

#[test]
fn test_dpt16_ascii_replacement() {
    let value = StringAscii::new("hé🙂").unwrap();

    assert_eq!(
        value.as_bytes(),
        &[b'h', b'?', b'?', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn test_dpt16_latin1_byte_preservation() {
    let value = StringLatin1::new("Grüße").unwrap();

    assert_eq!(
        value.as_bytes(),
        &[b'G', b'r', 0xFC, 0xDF, b'e', 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(value.value(), "Grüße");

    let decoded = StringLatin1::from_bytes(value.as_bytes()).unwrap();
    assert_eq!(decoded.value(), "Grüße");
    assert_eq!(decoded.as_bytes(), value.as_bytes());
}

#[test]
fn test_dpt16_decode_zero_stripping() {
    let raw = [b'H', 0, b'i', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let decoded = StringAscii::from_bytes(&raw).unwrap();

    assert_eq!(decoded.value(), "Hi");
    assert_eq!(decoded.as_bytes(), &raw);
}

#[test]
fn test_dpt16_rejects_too_long_strings() {
    let fourteen = "é".repeat(14);
    let fifteen = "é".repeat(15);

    assert!(StringAscii::new(&fourteen).is_ok());
    assert!(StringAscii::new(&fifteen).is_err());
    assert!(StringLatin1::new(&fifteen).is_err());
}

#[test]
fn test_switch_encode_decode() {
    let switch_on = Switch::new(true);
    let bytes = switch_on.as_bytes();
    assert_eq!(bytes, &[0x01]);

    let decoded = Switch::from_bytes(bytes).unwrap();
    assert_eq!(decoded, switch_on);
}

#[test]
fn test_switch_encode_decode_off() {
    let switch_off = Switch::new(false);
    let bytes = switch_off.as_bytes();
    assert_eq!(bytes, &[0x00]);

    let decoded = Switch::from_bytes(bytes).unwrap();
    assert_eq!(decoded, switch_off);
}

#[test]
fn test_scaling_encode_decode() {
    let scaling = Scaling::new(128);
    let bytes = scaling.as_bytes();
    assert_eq!(bytes, &[128]);

    let decoded = Scaling::from_bytes(bytes).unwrap();
    assert_eq!(decoded, scaling);
}

#[test]
fn test_control_dimming() {
    let control = ControlDimming::new(true, 5).unwrap();
    let bytes = control.as_bytes();
    assert_eq!(bytes, &[0x08 | 0x05]); // control bit + step code

    let decoded = ControlDimming::from_bytes(bytes).unwrap();
    assert_eq!(decoded, control);
}

#[test]
fn test_scene_number_python_parity() {
    let scene = SceneNumber::new(1).unwrap();
    assert_eq!(scene.as_bytes(), &[0]);

    let decoded = SceneNumber::from_bytes(&[0]).unwrap();
    assert_eq!(decoded.value(), 1);

    let scene = SceneNumber::new(64).unwrap();
    assert_eq!(scene.as_bytes(), &[63]);

    assert!(SceneNumber::new(0).is_err());
    assert!(SceneNumber::new(65).is_err());
}

#[test]
fn test_scene_control_python_parity() {
    let scene = SceneControl::new(1, false).unwrap();
    assert_eq!(scene.as_bytes(), &[0]);

    let scene = SceneControl::new(64, true).unwrap();
    assert_eq!(scene.as_bytes(), &[0xBF]);

    let decoded = SceneControl::from_bytes(&[0x80]).unwrap();
    assert_eq!(decoded.scene_number, 1);
    assert!(decoded.learn);

    assert!(SceneControl::new(0, false).is_err());
    assert!(SceneControl::new(65, false).is_err());
}

#[test]
fn test_temperature() {
    let temp = Temperature::new(20.0).unwrap(); // Use a simpler value
    let bytes = temp.as_bytes();

    let decoded = Temperature::from_bytes(bytes).unwrap();
    // Allow larger floating point differences for 2-byte float
    assert!((decoded.value() - 20.0).abs() < 1.0);
}

#[test]
fn test_value_4byte_unsigned() {
    let value = Value4ByteUnsigned::new(0x1234_5678);
    let bytes = value.as_bytes();
    assert_eq!(bytes, &[0x12, 0x34, 0x56, 0x78]);

    let decoded = Value4ByteUnsigned::from_bytes(bytes).unwrap();
    assert_eq!(decoded, value);
}

#[test]
fn test_active_energy_encode_decode() {
    let energy = ActiveEnergy::new(1000);
    let bytes = energy.as_bytes();

    let decoded = ActiveEnergy::from_bytes(bytes).unwrap();
    assert_eq!(decoded, energy);
}

#[test]
fn test_power_encode_decode() {
    use super::dpt14::DPTPower;
    let power = DPTPower::new(1500.0);
    let bytes = power.as_bytes();

    let decoded = DPTPower::from_bytes(bytes).unwrap();
    assert_eq!(decoded, power);

    // Test metadata
    let metadata = DPTPower::metadata();
    assert_eq!(metadata.unit, Some("W"));
    assert_eq!(metadata.ha_device_class, Some("power"));
}

#[test]
fn test_dpt_wrapper() {
    let switch = Switch::new(true);
    let dpt_switch = Dpt::new(switch.clone()).unwrap();

    assert_eq!(dpt_switch.value(), &switch);

    let bytes = dpt_switch.as_bytes();
    assert_eq!(bytes, &[0x01]);

    let decoded = Dpt::<Switch>::decode(bytes).unwrap();
    assert_eq!(decoded.value(), &switch);
}

#[test]
fn test_dpt_registry() {
    let registry = DptRegistry::new();

    // Test DPTSwitch (using alias)
    let switch = DPTSwitch::new(true);
    let bytes = registry.encode("1.001", &switch).unwrap();
    assert_eq!(bytes, &[0x01]);

    let decoded = registry.decode("1.001", &bytes).unwrap();
    let decoded_switch = decoded.downcast_ref::<DPTSwitch>().unwrap();
    assert_eq!(decoded_switch, &switch);

    // Test DPTTemperature
    let temp = DPTTemperature::new(25.0);
    let bytes = registry.encode("9.001", &temp).unwrap();

    let decoded = registry.decode("9.001", &bytes).unwrap();
    let decoded_temp = decoded.downcast_ref::<DPTTemperature>().unwrap();
    assert!((decoded_temp.value() - 25.0).abs() < 0.1);
}

#[test]
fn test_dpt_aliases() {
    // Test DPTSwitch alias
    let dpt_switch = DPTSwitch::new(true);
    let bytes = dpt_switch.as_bytes();
    assert_eq!(bytes, &[0x01]);

    let decoded = DPTSwitch::from_bytes(bytes).unwrap();
    assert_eq!(decoded, dpt_switch);

    // Verify different DPT number
    assert_eq!(DPTSwitch::DPT_NUMBER, "1.001");
    assert_eq!(Switch::DPT_NUMBER, "1.001"); // Same as base

    // Test DPTBool alias with different DPT number
    assert_eq!(DPTBool::DPT_NUMBER, "1.002");

    // Test DPTTemperature alias
    let dpt_temp = DPTTemperature::new(20.0);
    let bytes = dpt_temp.as_bytes();

    let decoded = DPTTemperature::from_bytes(bytes).unwrap();
    assert!((decoded.value() - 20.0).abs() < 1.0);

    assert_eq!(DPTTemperature::DPT_NUMBER, "9.001");
    assert_eq!(DPTTemperature::HA_DEVICE_CLASS, Some("temperature"));
}

#[test]
fn test_dpt8_percent_v16_python_parity() {
    let negative_one = DPTPercentV16::new(-1.0).unwrap();
    assert_eq!(negative_one.as_bytes(), &[255, 156]);
    assert!((negative_one.value() - -1.0).abs() < f64::EPSILON);

    let positive_one = DPTPercentV16::new(1.0).unwrap();
    assert_eq!(positive_one.as_bytes(), &[0, 100]);
    assert!((positive_one.value() - 1.0).abs() < f64::EPSILON);

    let negative_large = DPTPercentV16::new(-123.0).unwrap();
    assert_eq!(negative_large.as_bytes(), &[207, 244]);
    assert!((negative_large.value() - -123.0).abs() < f64::EPSILON);

    let decoded = DPTPercentV16::from_bytes(&[207, 244]).unwrap();
    assert!((decoded.value() - -123.0).abs() < f64::EPSILON);
    assert_eq!(decoded.as_bytes(), &[207, 244]);
}

#[test]
fn test_dpt8_raw_aliases_remain_unscaled() {
    let count = DPTValue2Count::new(-123);
    assert_eq!(count.as_bytes(), &[255, 133]);
    assert_eq!(count.value(), -123);

    let angle = DPTRotationAngle::new(123);
    assert_eq!(angle.as_bytes(), &[0, 123]);
    assert_eq!(angle.value(), 123);
}

#[test]
fn test_dpt_alias_registry() {
    let registry = DptRegistry::new();

    // Test that aliases are registered with different DPT numbers
    let dpt_switch = DPTSwitch::new(true);
    let bytes = registry.encode("1.001", &dpt_switch).unwrap();
    assert_eq!(bytes, &[0x01]);

    let dpt_bool = DPTBool::new(false);
    let bytes = registry.encode("1.002", &dpt_bool).unwrap();
    assert_eq!(bytes, &[0x00]);

    // Verify they decode correctly
    let decoded = registry.decode("1.001", &[0x01]).unwrap();
    let decoded_switch = decoded.downcast_ref::<DPTSwitch>().unwrap();
    assert!(decoded_switch.value());
}

#[test]
fn test_hvac_mode_python_parity() {
    let mode = super::dpt20::HVACMode::new(4).unwrap();
    assert_eq!(mode.as_bytes(), &[4]);

    let decoded = super::dpt20::HVACMode::from_bytes(&[4]).unwrap();
    assert_eq!(decoded.value(), 4);

    assert!(super::dpt20::HVACMode::from_bytes(&[5]).is_err());
}

#[test]
fn test_hvac_controller_mode_python_parity() {
    let mode = super::dpt20::HVACControllerMode::new(20).unwrap();
    assert_eq!(mode.as_bytes(), &[20]);

    let decoded = super::dpt20::HVACControllerMode::from_bytes(&[17]).unwrap();
    assert_eq!(decoded.value(), 17);

    assert!(super::dpt20::HVACControllerMode::from_bytes(&[18]).is_err());

    let decoded = super::dpt20::HVACControllerMode::from_bytes(&[20]).unwrap();
    assert_eq!(decoded.value(), 20);

    assert!(super::dpt20::HVACControllerMode::from_bytes(&[21]).is_err());
}

#[test]
fn test_time_of_day_python_parity() {
    let time = TimeOfDay::new(1, 23, 59, 58).unwrap();
    assert_eq!(time.as_bytes(), &[(1 << 5) | 0x17, 59, 58]);

    let decoded = TimeOfDay::from_bytes(&[(7 << 5) | 0x0c, 34, 56]).unwrap();
    assert_eq!(decoded.day, 7);
    assert_eq!(decoded.hour, 12);
    assert_eq!(decoded.minute, 34);
    assert_eq!(decoded.second, 56);
    assert_eq!(decoded.as_bytes(), &[(7 << 5) | 0x0c, 34, 56]);

    assert!(TimeOfDay::from_bytes(&[24, 0, 0]).is_err());
    assert!(TimeOfDay::from_bytes(&[0, 60, 0]).is_err());
    assert!(TimeOfDay::from_bytes(&[0, 0, 60]).is_err());
}

#[test]
fn test_date_python_parity() {
    let date = Date::new(31, 12, 89).unwrap();
    assert_eq!(date.as_bytes(), &[31, 12, 89]);

    let decoded = Date::from_bytes(&[1, 1, 24]).unwrap();
    assert_eq!(decoded.day, 1);
    assert_eq!(decoded.month, 1);
    assert_eq!(decoded.year, 24);
    assert_eq!(decoded.as_bytes(), &[1, 1, 24]);

    assert!(Date::from_bytes(&[0, 1, 24]).is_err());
    assert!(Date::from_bytes(&[1, 0, 24]).is_err());
    assert!(Date::from_bytes(&[1, 13, 24]).is_err());
}

#[test]
fn test_datetime_python_parity_encode_midnight() {
    let datetime = DateTime::new(24, 1, 1, 0, 0, 0, 0).unwrap();

    assert_eq!(datetime.as_bytes(), &[24, 1, 1, 0, 0, 0, 0, 0]);
}

#[test]
fn test_datetime_python_parity_decode_external_sync_source_reliable() {
    let raw = [24, 1, 1, 0, 0, 0, 0, 0xC0];
    let datetime = DateTime::from_bytes(&raw).unwrap();

    assert_eq!(datetime.as_bytes(), &raw);

    let debug = format!("{datetime:?}");
    assert!(debug.contains("external_sync: true"));
    assert!(debug.contains("source_reliable: true"));
}

#[test]
fn test_datetime_python_parity_invalid_group_flags() {
    let datetime = DateTime::new_with_flags(
        DateTimeParts {
            year: 24,
            month: 1,
            day: 1,
            day_of_week: 1,
            hour: 12,
            minute: 34,
            second: 56,
        },
        DateTimeFlags {
            no_wd: true,
            no_year: true,
            no_date: true,
            no_dow: true,
            no_time: true,
            ..DateTimeFlags::default()
        },
    )
    .unwrap();

    assert_eq!(datetime.as_bytes(), &[0, 0, 0, 0, 0, 0, 0x3E, 0]);
}

#[test]
fn test_datetime_python_parity_24_hour_validation() {
    let midnight = DateTime::from_bytes(&[24, 1, 1, 24, 0, 0, 0, 0]).unwrap();
    assert_eq!(midnight.as_bytes(), &[24, 1, 1, 24, 0, 0, 0, 0]);

    assert!(DateTime::from_bytes(&[24, 1, 1, 24, 1, 0, 0, 0]).is_err());
}
