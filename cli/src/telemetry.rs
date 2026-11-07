use std::ffi::{c_void, CString};
use std::ptr;

use thinkflow_core::ThinkFlowPolicy;

use crate::ffi::{
    cfstr, check_acpi_node_exists, dict_get_string, dict_get_u64, kCFAllocatorDefault,
    CFDictionaryRef, CFMutableDictionaryRef, CFNumberCreate, CFRelease, CFTypeRef, IOObjectRelease,
    IORegistryEntryCreateCFProperties, IORegistryEntrySetCFProperty, IOServiceGetMatchingService,
    IOServiceMatching, K_CF_NUMBER_SINT32_TYPE,
};

// ============================================================================
// LenovoThinkFlow.kext IORegistry Telemetry & Policy Control
// ============================================================================

#[derive(Debug, Default, Clone)]
pub struct LiveTelemetry {
    pub loaded: bool,
    pub policy_mode: u32,
    pub policy_name: String,
    pub active_profile: u32,
    pub profile_name: String,
    pub cpu_temp_c: u32,
    pub tjmax_c: u32,
    pub pkg_power_w: f64,
    pub target_pl1_w: f64,
    pub effective_pl1_w: f64,
    pub core_power_w: f64,
    pub p_freq_ghz: f64,
    pub e_freq_ghz: f64,
    pub ec_fan_target: u32,
    pub ec_temp0_c: u32,
    pub ec_temp1_c: u32,
    pub ec_fan_raw: u32,
    pub ec_fan_rpm: u32,
    pub ec_pfmm: u32,
    pub ec_itsm: u32,
    pub ec_lxos: u32,
    pub ec_tpa1: u32,
    pub hwp_max_ratio: u32,
    pub hwp_epp: u32,
    pub is_lpm_active: bool,
    pub is_throttling: bool,
    pub hysteresis_counter: u32,
    pub total_ticks: u64,
    pub mode_switch_count: u32,
    pub ssdt_present: bool,
    pub vpc0_ok: bool,
    pub tzs0_ok: bool,
    pub ietm_ok: bool,
}

pub(crate) fn read_kext_telemetry() -> Option<LiveTelemetry> {
    let mut t = LiveTelemetry::default();
    let cname = CString::new("ThinkFlow").unwrap();
    let service = unsafe { IOServiceGetMatchingService(0, IOServiceMatching(cname.as_ptr())) };
    if service == 0 {
        return None;
    }

    let mut props: CFMutableDictionaryRef = ptr::null_mut();
    let kr = unsafe {
        IORegistryEntryCreateCFProperties(service, &mut props, kCFAllocatorDefault, 0)
    };
    if kr == 0 && !props.is_null() {
        let dict = props as CFDictionaryRef;
        t.loaded = true;
        t.policy_mode = dict_get_u64(dict, "PolicyMode").unwrap_or(0) as u32;
        t.policy_name = dict_get_string(dict, "PolicyName").unwrap_or_else(|| "Auto".into());
        t.active_profile = dict_get_u64(dict, "ActiveProfile").unwrap_or(0) as u32;
        t.profile_name = dict_get_string(dict, "ProfileName").unwrap_or_else(|| "Unknown".into());
        t.cpu_temp_c = dict_get_u64(dict, "CPUTempC").unwrap_or(0) as u32;
        t.tjmax_c = dict_get_u64(dict, "CPUTjMaxC").unwrap_or(100) as u32;
        let mw = dict_get_u64(dict, "PackagePowerMW").unwrap_or(0) as f64;
        t.pkg_power_w = mw / 1000.0;
        t.target_pl1_w = dict_get_u64(dict, "TargetPL1MW").unwrap_or(0) as f64 / 1000.0;
        t.effective_pl1_w = dict_get_u64(dict, "EffectivePL1MW").unwrap_or(0) as f64 / 1000.0;
        t.ec_fan_target = dict_get_u64(dict, "ECFanTarget").unwrap_or(0) as u32;
        t.ec_temp0_c = dict_get_u64(dict, "ECTemp0C").unwrap_or(0) as u32;
        t.ec_temp1_c = dict_get_u64(dict, "ECTemp1C").unwrap_or(0) as u32;
        let raw_fan = dict_get_u64(dict, "ECFanRaw").unwrap_or(0) as u32;
        t.ec_fan_raw = raw_fan;
        // Lenovo EC raw fan value 0..100 maps to ~ 0..4800 RPM
        t.ec_fan_rpm = if raw_fan > 0 { (raw_fan as f64 * 48.0) as u32 } else { 0 };
        t.ec_pfmm = dict_get_u64(dict, "ECPFMM").unwrap_or(0) as u32;
        t.ec_itsm = dict_get_u64(dict, "ECITSM").unwrap_or(0) as u32;
        t.ec_lxos = dict_get_u64(dict, "ECLXOS").unwrap_or(0) as u32;
        t.ec_tpa1 = dict_get_u64(dict, "ECTPA1").unwrap_or(0) as u32;
        t.hwp_max_ratio = dict_get_u64(dict, "HWPMaxRatio").unwrap_or(0) as u32;
        t.hwp_epp = dict_get_u64(dict, "HWPEPP").unwrap_or(0) as u32;
        t.is_lpm_active = dict_get_u64(dict, "LowPowerMode").unwrap_or(0) != 0;
        t.is_throttling = dict_get_u64(dict, "ThermalThrottling").unwrap_or(0) != 0;
        t.hysteresis_counter = dict_get_u64(dict, "HysteresisTicks").unwrap_or(0) as u32;
        t.mode_switch_count = dict_get_u64(dict, "ModeSwitchCount").unwrap_or(0) as u32;
        t.total_ticks = dict_get_u64(dict, "TotalTicks").unwrap_or(0);
        t.ssdt_present = dict_get_u64(dict, "SSDTHelperPresent").unwrap_or(0) != 0;
        unsafe { CFRelease(props as CFTypeRef) };
    }
    unsafe { IOObjectRelease(service) };

    t.vpc0_ok = check_acpi_node_exists("IOACPIPlane:/_SB/PC00/LPCB/EC0/VPC0");
    t.tzs0_ok = check_acpi_node_exists("IOACPIPlane:/TZS0");
    t.ietm_ok = check_acpi_node_exists("IOACPIPlane:/_SB/IETM");

    Some(t)
}

pub(crate) fn set_kext_policy(policy: ThinkFlowPolicy) -> Result<(), String> {
    let cname = CString::new("ThinkFlow").unwrap();
    let service = unsafe { IOServiceGetMatchingService(0, IOServiceMatching(cname.as_ptr())) };
    if service == 0 {
        return Err("LenovoThinkFlow.kext is not loaded in the running kernel.".into());
    }

    let key = cfstr("PolicyMode");
    let val_i32 = policy as i32;
    let num = unsafe {
        CFNumberCreate(
            kCFAllocatorDefault,
            K_CF_NUMBER_SINT32_TYPE,
            &val_i32 as *const i32 as *const c_void,
        )
    };
    let kr = unsafe { IORegistryEntrySetCFProperty(service, key, num) };
    unsafe {
        CFRelease(num);
        CFRelease(key);
        IOObjectRelease(service);
    }

    if kr == 0 {
        Ok(())
    } else {
        Err(format!("Error setting property: 0x{:08x}", kr))
    }
}
