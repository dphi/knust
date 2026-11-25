//! Protocol layer for KNX/IP frame handling and data processing.
//!
//! This module contains the core protocol implementations including CEMI frame
//! parsing, Data Point Type (DPT) handling, and address management.

pub mod address;
pub mod apci;
pub mod cemi;
#[cfg(feature = "dpt")]
pub mod dpt;
pub mod knxip;
pub mod management;
pub mod profile;
pub mod telegram;
pub mod tpci;

pub use address::{Address, IndividualAddress, MainGroup, MiddleGroup, RawGroupAddress};
pub use apci::GroupValueService;
pub use cemi::{
    AdditionalInfo, AddressType, CemiFrame, ControlField, ExtendedControlField, FrameType,
    MessageCode,
};
#[cfg(feature = "dpt")]
pub use dpt::{
    ActiveEnergy,
    Brightness,
    CharacterAscii,
    CharacterLatin1,
    ControlBlinds,

    // DPT 3.xxx - Control
    ControlDimming,
    DPT2Ucount,
    DPTActiveEnergy,

    DPTAngle,

    DPTBool,
    DPTBrightness,

    DPTCharacter,
    DPTCharacterLatin1,
    DPTEnable,

    DPTPercentV8,
    DPTPower,
    DPTScaling,
    DPTSwitch,
    DPTTemperature,

    DPTValue4Ucount,

    DPTVersion,
    // Core DPT types and traits
    Dpt,
    // DPT 14.xxx - 4-byte float
    Dpt14Float,
    DptMetadata,
    DptRegistry,
    DptValue,

    KnxVersion,
    // DPT 6.xxx - Signed 8-bit
    PercentV8,
    // DPT 5.xxx - Unsigned 8-bit
    Scaling,
    // DPT 1.xxx - Boolean
    Switch,
    // DPT 9.xxx - 2-byte float
    Temperature,
    Value1Count,

    // DPT 8.xxx - Signed 16-bit
    Value2ByteSigned,

    // DPT 7.xxx - Unsigned 16-bit
    Value2ByteUnsigned,
    // DPT 13.xxx - Signed 32-bit
    Value4ByteSigned,
    // DPT 12.xxx - Unsigned 32-bit
    Value4ByteUnsigned,
    Version,
};
pub use knxip::{ConnectRequest, ConnectResponse, Hpai, KnxIpFrame, ServiceType};
pub use management::{
    DeviceDescriptorRead, DeviceDescriptorResponse, IndividualAddressSerialRead,
    IndividualAddressSerialResponse, IndividualAddressSerialWrite,
};
pub use profile::PropertyDataType;
pub use telegram::{Priority, Telegram, TelegramType};
pub use tpci::TpciFrame;
