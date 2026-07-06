//! Color DPT demonstration
//!
//! This example shows how to use the color Data Point Types (DPT 232.xxx, 251.xxx)
//! for RGB and RGBW color representations in KNX systems using the modern type-safe DPT system.

use knust::protocol::dpt::DptValue;

// Create simple RGB and RGBW color types for demonstration
#[derive(Debug, Clone, PartialEq)]
struct RgbColor {
    red: u8,
    green: u8,
    blue: u8,
    bytes: [u8; 3],
}

impl RgbColor {
    fn new(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            bytes: [red, green, blue],
        }
    }
}

impl DptValue for RgbColor {
    const DPT_NUMBER: &'static str = "232.600";
    const VALUE_TYPE: &'static str = "rgb_color";
    const BYTE_LENGTH: usize = 3;

    fn from_bytes(bytes: &[u8]) -> knust::error::Result<Self> {
        if bytes.len() != 3 {
            return Err(knust::error::ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: "Invalid length".to_string(),
            }
            .into());
        }
        Ok(RgbColor::new(bytes[0], bytes[1], bytes[2]))
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn validate(&self) -> knust::error::Result<()> {
        Ok(())
    }

    fn value_range() -> (f64, f64) {
        (0.0, 16_777_215.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RgbwColor {
    red: u8,
    green: u8,
    blue: u8,
    white: u8,
    bytes: [u8; 6],
}

impl RgbwColor {
    fn new(red: u8, green: u8, blue: u8, white: u8) -> Self {
        Self {
            red,
            green,
            blue,
            white,
            bytes: [red, green, blue, white, 0x0F, 0x00],
        }
    }
}

impl DptValue for RgbwColor {
    const DPT_NUMBER: &'static str = "251.600";
    const VALUE_TYPE: &'static str = "rgbw_color";
    const BYTE_LENGTH: usize = 6;

    fn from_bytes(bytes: &[u8]) -> knust::error::Result<Self> {
        if bytes.len() != 6 {
            return Err(knust::error::ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: "Invalid length".to_string(),
            }
            .into());
        }
        Ok(RgbwColor::new(bytes[0], bytes[1], bytes[2], bytes[3]))
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn validate(&self) -> knust::error::Result<()> {
        Ok(())
    }

    fn value_range() -> (f64, f64) {
        (0.0, 4_294_967_295.0)
    }
}

// rgb_color/rgbw_color (and their _bytes) name distinct DPT formats;
// no rename reads clearer than the RGB/RGBW distinction itself.
#[allow(clippy::similar_names)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🎨 KNX Color DPT Demonstration");
    println!("===============================\n");

    // RGB Color (DPT 232.600) - 3 bytes: R, G, B
    println!("🔴 RGB Color (DPT 232.600)");
    println!("---------------------------");

    let rgb_color = RgbColor::new(255, 128, 64);
    println!(
        "Original RGB: R={}, G={}, B={}",
        rgb_color.red, rgb_color.green, rgb_color.blue
    );

    // Encode to bytes
    let rgb_bytes = rgb_color.encode()?;
    println!("Encoded bytes: {rgb_bytes:02X?}");

    // Decode back from bytes
    let decoded_rgb = RgbColor::decode(&rgb_bytes)?;
    println!(
        "Decoded RGB: R={}, G={}, B={}",
        decoded_rgb.red, decoded_rgb.green, decoded_rgb.blue
    );

    println!("✅ RGB encoding/decoding successful!\n");

    // RGBW Color (DPT 251.600) - 6 bytes: R, G, B, W, Valid, Reserved
    println!("🌈 RGBW Color (DPT 251.600)");
    println!("----------------------------");

    let rgbw_color = RgbwColor::new(200, 100, 50, 25);
    println!(
        "Original RGBW: R={}, G={}, B={}, W={}",
        rgbw_color.red, rgbw_color.green, rgbw_color.blue, rgbw_color.white
    );

    // Encode to bytes
    let rgbw_bytes = rgbw_color.encode()?;
    println!("Encoded bytes: {rgbw_bytes:02X?}");

    // Decode back from bytes
    let decoded_rgbw = RgbwColor::decode(&rgbw_bytes)?;
    println!(
        "Decoded RGBW: R={}, G={}, B={}, W={}",
        decoded_rgbw.red, decoded_rgbw.green, decoded_rgbw.blue, decoded_rgbw.white
    );

    println!("✅ RGBW encoding/decoding successful!\n");

    // Demonstrate metadata
    println!("📋 DPT Metadata");
    println!("----------------");
    println!("RGB DPT Number: {}", RgbColor::DPT_NUMBER);
    println!("RGB Value Type: {}", RgbColor::VALUE_TYPE);
    println!("RGB Byte Length: {}", RgbColor::BYTE_LENGTH);

    println!("RGBW DPT Number: {}", RgbwColor::DPT_NUMBER);
    println!("RGBW Value Type: {}", RgbwColor::VALUE_TYPE);
    println!("RGBW Byte Length: {}", RgbwColor::BYTE_LENGTH);

    Ok(())
}
