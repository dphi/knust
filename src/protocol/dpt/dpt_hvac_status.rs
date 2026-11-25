//! Combined room-control and valve actuator status bitsets.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

fn bad(number: &str) -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: number.into(),
        details: "Invalid status bitset".into(),
    }
    .into()
}
macro_rules! status {
    ($value:ident,$dpt:ident,$length:expr,$number:literal,$kind:literal,$($field:ident:$bit:expr),+) => {
        #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $value { $(pub $field: bool,)+ }
        impl $value {
            pub(super) fn encode(self) -> [u8; $length] {
                let mut raw = 0u16; $(if self.$field { raw |= 1 << $bit; })+
                let bytes = raw.to_be_bytes(); let mut result = [0; $length]; result.copy_from_slice(&bytes[2-$length..]); result
            }
            pub(super) fn decode(bytes: &[u8]) -> Self {
                let raw = if $length == 1 { u16::from(bytes[0]) } else { u16::from_be_bytes([bytes[0], bytes[1]]) };
                Self { $($field: raw & (1 << $bit) != 0,)+ }
            }
        }
        impl std::fmt::Display for $value {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { $(write!(f, "{}", u8::from(self.$field))?;)+ Ok(()) }
        }
        impl std::str::FromStr for $value {
            type Err = crate::error::KnxError;
            fn from_str(s: &str) -> Result<Self> {
                let mut bytes = s.bytes();
                let mut next = || -> Result<bool> { match bytes.next() { Some(b'0') => Ok(false), Some(b'1') => Ok(true), _ => Err(bad($number)) } };
                let result = Self { $($field: next()?,)+ };
                if bytes.next().is_some() { return Err(bad($number)); } Ok(result)
            }
        }
        #[derive(Debug, Clone, PartialEq, Eq, Hash)] pub struct $dpt([u8; $length]);
        impl $dpt {
            #[must_use] pub fn new(value: $value) -> Self { Self(value.encode()) }
            #[must_use] pub fn value(&self) -> $value { $value::decode(&self.0) }
        }
        impl DptValue for $dpt {
            const DPT_NUMBER: &'static str = $number; const VALUE_TYPE: &'static str = $kind; const BYTE_LENGTH: usize = $length;
            fn from_bytes(bytes: &[u8]) -> Result<Self> { Ok(Self(bytes.try_into().map_err(|_| bad($number))?)) }
            fn as_bytes(&self) -> &[u8] { &self.0 }
            fn validate(&self) -> Result<()> { Ok(()) }
            fn value_range() -> (f64,f64) { (0.0,0.0) }
        }
        impl WriteValue<$dpt> for $value { fn resolve(self, _: DptType) -> Result<$dpt> { Ok($dpt::new(self)) } }
    };
}
status!(ForcingSignal,DPTForceSign,1,"21.100","forcing_signal",force_request:0,protection:1,oversupply:2,overrun:3,dhw_norm:4,dhw_legio:5,room_h_comf:6,room_h_max:7);
status!(ForcingSignalCool,DPTForceSignCool,1,"21.101","forcing_signal_cool",force_request:0);
status!(RoomHeatingControllerStatus,DPTStatusRHC,1,"21.102","room_heating_controller_status",fault:0,status_eco:1,temp_flow_limit:2,temp_return_limit:3,status_morning_boost:4,status_start_optim:5,status_stop_optim:6,summer_mode:7);
status!(SolarDHWControllerStatus,DPTStatusSDHWC,1,"21.103","solar_dhw_controller_status",fault:0,sdhw_load_active:1,solar_load_sufficient:2);
status!(FuelTypeSet,DPTFuelTypeSet,1,"21.104","fuel_type_set",oil:0,gas:1,solid_state:2);
status!(RoomCoolingControllerStatus,DPTStatusRCC,1,"21.105","room_cooling_controller_status",fault:0);
status!(VentilationControllerStatus,DPTStatusAHU,1,"21.106","ventilation_controller_status",fault:0,fan_active:1,heat:2,cool:3);
status!(LightActuatorErrorInfo,DPTLightActuatorErrorInfo,1,"21.601","light_actuator_error_info",load_detection_error:0,undervoltage:1,overcurrent:2,underload:3,defective_load:4,lamp_failure:5,overheat:6);
status!(RFFilterInfo,DPTRFFilterInfo,1,"21.1001","rf_filter_info",doa:0,knx_sn:1,doa_and_knx_sn:2);
status!(SecurityReport,DPTSecurityReport,1,"21.1002","security_report",security_failure:0);
status!(ChannelActivation8,DPTChannelActivation8,1,"21.1010","channel_activation_8",channel_1:0,channel_2:1,channel_3:2,channel_4:3,channel_5:4,channel_6:5,channel_7:6,channel_8:7);
status!(VirtualDryContact,DPTVirtualDryContact,1,"21.1200","virtual_dry_contact",contact_1:0,contact_2:1,contact_3:2,contact_4:3,contact_5:4,contact_6:5,contact_7:6,contact_8:7);
status!(PhasesStatus,DPTPhasesStatus,1,"21.1201","phases_status",phase_1:0,phase_2:1,phase_3:2);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RFModeInfo {
    pub asynchronous: bool,
    pub bibat_master: bool,
    pub bibat_slave: bool,
}
impl Default for RFModeInfo {
    fn default() -> Self {
        Self {
            asynchronous: true,
            bibat_master: false,
            bibat_slave: false,
        }
    }
}
impl RFModeInfo {
    pub fn encode(self) -> Result<[u8; 1]> {
        if !self.asynchronous {
            return Err(bad("21.1000"));
        }
        Ok([u8::from(self.asynchronous)
            | (u8::from(self.bibat_master) << 1)
            | (u8::from(self.bibat_slave) << 2)])
    }
    pub(super) fn decode(bytes: &[u8]) -> Self {
        Self {
            asynchronous: bytes[0] & 1 != 0,
            bibat_master: bytes[0] & 2 != 0,
            bibat_slave: bytes[0] & 4 != 0,
        }
    }
}
impl std::fmt::Display for RFModeInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}{}",
            u8::from(self.asynchronous),
            u8::from(self.bibat_master),
            u8::from(self.bibat_slave)
        )
    }
}
impl std::str::FromStr for RFModeInfo {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let mut bytes = s.bytes();
        let mut next = || -> Result<bool> {
            match bytes.next() {
                Some(b'0') => Ok(false),
                Some(b'1') => Ok(true),
                _ => Err(bad("21.1000")),
            }
        };
        let result = Self {
            asynchronous: next()?,
            bibat_master: next()?,
            bibat_slave: next()?,
        };
        if bytes.next().is_some() {
            return Err(bad("21.1000"));
        }
        Ok(result)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTRFModeInfo([u8; 1]);
impl DPTRFModeInfo {
    pub fn new(value: RFModeInfo) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    pub fn value(&self) -> RFModeInfo {
        RFModeInfo::decode(&self.0)
    }
}
impl DptValue for DPTRFModeInfo {
    const DPT_NUMBER: &'static str = "21.1000";
    const VALUE_TYPE: &'static str = "rf_mode_info";
    const BYTE_LENGTH: usize = 1;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(bytes.try_into().map_err(|_| bad("21.1000"))?))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        Ok(())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
    fn encode(&self) -> Result<Vec<u8>> {
        self.value().encode().map(|bytes| bytes.to_vec())
    }
}
impl WriteValue<DPTRFModeInfo> for RFModeInfo {
    fn resolve(self, _: DptType) -> Result<DPTRFModeInfo> {
        DPTRFModeInfo::new(self)
    }
}
status!(GeneralStatus,DPTStatusGen,1,"21.001","general_status",out_of_service:0,fault:1,overridden:2,in_alarm:3,alarm_un_ack:4);
status!(DeviceControl,DPTDeviceControl,1,"21.002","device_control",user_stopped:0,own_ia:1,verify_mode:2);
status!(CombinedStatusRTSM,DPTCombinedStatusRTSM,1,"21.107","combined_status_rtsm",window_open:0,presence:1,comfort_push_button:2,comfort_prolongation_user:3,hvac_mode_user:4);
status!(DHWControllerStatus,DPTStatusDHWC,2,"22.100","dhw_controller_status",fault:0,dhw_load_active:1,legio_prot_active:2,dhw_push_active:3,other_energy_source_active:4,solar_energy_only:5,solar_energy_support:6,temp_optim_shift_active:7);
status!(SupportedMedia,DPTMedia,2,"22.1000","supported_media",tp1:1,pl110:2,rf:4,knx_ip:5);
status!(ChannelActivation16,DPTChannelActivation16,2,"22.1010","channel_activation_16",channel_1:0,channel_2:1,channel_3:2,channel_4:3,channel_5:4,channel_6:5,channel_7:6,channel_8:7,channel_9:8,channel_10:9,channel_11:10,channel_12:11,channel_13:12,channel_14:13,channel_15:14,channel_16:15);
status!(RoomHeatingCoolingControllerStatus,DPTStatusRHCC,2,"22.101","room_heating_cooling_controller_status",fault:0,status_eco_h:1,temp_flow_limit:2,temp_return_limit:3,status_morning_boost_h:4,status_start_optim:5,status_stop_optim:6,heating_disabled:7,heat_cool_mode:8,status_eco_c:9,status_pre_cool:10,cooling_disabled:11,dew_point_status:12,frost_alarm:13,overheat_alarm:14);
status!(CombinedStatusHVA,DPTCombinedStatusHVA,2,"22.102","combined_status_hva",valve_open:0,short_circuit:1,overload:2,valve_kick:3,service_mode:4,manual_operation:5,forced_position:6,locked_position:7,calibration_mode:8);
// In KNX heat/cool encoding, true selects heating and false selects cooling.
status!(CombinedStatusRTC,DPTCombinedStatusRTC,2,"22.103","combined_status_rtc",fault:0,heating_mode:1,dew_point_status:2,frost_alarm:3,overheat_alarm:4,controller_inactive:5,additional_stage:6,heating_enabled:7,cooling_enabled:8);

macro_rules! status_view {
    ($view:ident,$value:ident) => {
        #[derive(Debug, Clone, Copy)]
        pub struct $view<'a>(pub(super) &'a [u8]);
        impl<'a> $view<'a> {
            #[must_use]
            pub fn value(&self) -> $value {
                $value::decode(self.0)
            }
            #[must_use]
            pub fn raw(&self) -> &'a [u8] {
                self.0
            }
        }
    };
}
status_view!(LightActuatorErrorInfoView, LightActuatorErrorInfo);
status_view!(RFFilterInfoView, RFFilterInfo);
status_view!(SecurityReportView, SecurityReport);
status_view!(ChannelActivation8View, ChannelActivation8);
status_view!(VirtualDryContactView, VirtualDryContact);
status_view!(PhasesStatusView, PhasesStatus);
status_view!(RFModeInfoView, RFModeInfo);
status_view!(StatusRTSMView, CombinedStatusRTSM);
status_view!(ForcingSignalView, ForcingSignal);
status_view!(ForcingSignalCoolView, ForcingSignalCool);
status_view!(RoomHeatingControllerStatusView, RoomHeatingControllerStatus);
status_view!(SolarDHWControllerStatusView, SolarDHWControllerStatus);
status_view!(FuelTypeSetView, FuelTypeSet);
status_view!(RoomCoolingControllerStatusView, RoomCoolingControllerStatus);
status_view!(VentilationControllerStatusView, VentilationControllerStatus);
status_view!(GeneralStatusView, GeneralStatus);
status_view!(DeviceControlView, DeviceControl);
status_view!(DHWControllerStatusView, DHWControllerStatus);
status_view!(SupportedMediaView, SupportedMedia);
status_view!(ChannelActivation16View, ChannelActivation16);
status_view!(
    RoomHeatingCoolingControllerStatusView,
    RoomHeatingCoolingControllerStatus
);
status_view!(StatusHVAView, CombinedStatusHVA);
status_view!(StatusRTCView, CombinedStatusRTC);
