use std::collections::{HashMap, VecDeque};
use std::env;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::stdout;
use std::ptr;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Paragraph, Sparkline,
    },
    Frame, Terminal,
};

use thinkflow_core::{
    ThinkFlowEngine, ThinkFlowPolicy, ThinkFlowTelemetryInput,
};

// ============================================================================
// CoreFoundation & IOKit / libIOReport FFI Bindings
// ============================================================================

type CFAllocatorRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDataRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFMutableDictionaryRef = *mut c_void;
type CFNumberRef = *const c_void;
type CFTypeRef = *const c_void;
type CFIndex = isize;
type CFStringEncoding = u32;

const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;
const K_CF_NUMBER_SINT32_TYPE: CFIndex = 3;
const K_CF_NUMBER_SINT64_TYPE: CFIndex = 4;

type MachPortT = u32;
type IoObjectT = u32;
type IoRegistryEntryT = IoObjectT;
type IoServiceT = IoObjectT;
type KernReturnT = c_int;

type IOReportSubscriptionRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFAllocatorDefault: CFAllocatorRef;

    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: CFStringEncoding,
    ) -> CFStringRef;
    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: CFStringEncoding,
    ) -> bool;
    fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type: CFIndex,
        value_ptr: *const c_void,
    ) -> CFNumberRef;
    fn CFNumberGetValue(
        number: CFNumberRef,
        the_type: CFIndex,
        value_ptr: *mut c_void,
    ) -> bool;
    fn CFDictionaryGetValue(the_dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
    fn CFDataGetLength(the_data: CFDataRef) -> CFIndex;
    fn CFDataGetBytePtr(the_data: CFDataRef) -> *const u8;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(boolean: CFTypeRef) -> bool;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
    fn IOServiceGetMatchingService(
        main_port: MachPortT,
        matching: CFDictionaryRef,
    ) -> IoServiceT;
    fn IORegistryEntryFromPath(
        main_port: MachPortT,
        path: *const c_char,
    ) -> IoRegistryEntryT;
    fn IORegistryEntryCreateCFProperties(
        entry: IoRegistryEntryT,
        properties: *mut CFMutableDictionaryRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> KernReturnT;
    fn IORegistryEntrySetCFProperty(
        entry: IoRegistryEntryT,
        property_name: CFStringRef,
        property: CFTypeRef,
    ) -> KernReturnT;
    fn IOObjectRelease(object: IoObjectT) -> KernReturnT;
}

#[link(name = "IOReport")]
extern "C" {
    fn IOReportCopyChannelsInGroup(
        group: CFStringRef,
        subgroup: CFStringRef,
        a: u64,
        b: u64,
        c: u64,
    ) -> CFMutableDictionaryRef;
    fn IOReportCreateSubscription(
        a: *const c_void,
        channels: CFMutableDictionaryRef,
        subbed_channels: *mut CFMutableDictionaryRef,
        channel_id: u64,
        b: CFTypeRef,
    ) -> IOReportSubscriptionRef;
    fn IOReportCreateSamples(
        sub: IOReportSubscriptionRef,
        subbed_channels: CFMutableDictionaryRef,
        a: CFTypeRef,
    ) -> CFDictionaryRef;
}

extern "C" {
    fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *const c_void,
        newlen: usize,
    ) -> c_int;
}

// ============================================================================
// Helper Wrappers for CoreFoundation / sysctl / IOKit
// ============================================================================

fn cfstr(s: &str) -> CFStringRef {
    let c = CString::new(s).unwrap();
    unsafe { CFStringCreateWithCString(kCFAllocatorDefault, c.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
}

fn cfstr_to_string(cf: CFStringRef) -> Option<String> {
    if cf.is_null() {
        return None;
    }
    let mut buf = [0u8; 256];
    let ok = unsafe {
        CFStringGetCString(
            cf,
            buf.as_mut_ptr() as *mut c_char,
            buf.len() as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if !ok {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) };
    Some(cstr.to_string_lossy().into_owned())
}

fn read_sysctl_i32(name: &str) -> Option<i32> {
    let cname = CString::new(name).ok()?;
    let mut val: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    let ret = unsafe {
        sysctlbyname(
            cname.as_ptr(),
            &mut val as *mut i32 as *mut c_void,
            &mut len,
            ptr::null(),
            0,
        )
    };
    if ret == 0 {
        Some(val)
    } else {
        None
    }
}

fn read_sysctl_string(name: &str) -> Option<String> {
    let cname = CString::new(name).ok()?;
    let mut buf = [0u8; 256];
    let mut len = buf.len();
    let ret = unsafe {
        sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr() as *mut c_void,
            &mut len,
            ptr::null(),
            0,
        )
    };
    if ret == 0 && len > 0 {
        let s = CStr::from_bytes_until_nul(&buf).ok()?;
        Some(s.to_string_lossy().into_owned())
    } else {
        None
    }
}

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
    pub core_power_w: f64,
    pub p_freq_ghz: f64,
    pub e_freq_ghz: f64,
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
    pub s0ix_handoff: bool,
    pub sleep_handoff_count: u32,
    pub vpc0_ok: bool,
    pub tzs0_ok: bool,
    pub ietm_ok: bool,
}

fn dict_get_u64(dict: CFDictionaryRef, key: &str) -> Option<u64> {
    let k = cfstr(key);
    let val_ptr = unsafe { CFDictionaryGetValue(dict, k) };
    unsafe { CFRelease(k) };
    if val_ptr.is_null() {
        return None;
    }
    let tid = unsafe { CFGetTypeID(val_ptr) };
    if tid == unsafe { CFNumberGetTypeID() } {
        let mut out: i64 = 0;
        if unsafe {
            CFNumberGetValue(
                val_ptr as CFNumberRef,
                K_CF_NUMBER_SINT64_TYPE,
                &mut out as *mut i64 as *mut c_void,
            )
        } {
            return Some(out as u64);
        }
    } else if tid == unsafe { CFBooleanGetTypeID() } {
        return Some(if unsafe { CFBooleanGetValue(val_ptr) } { 1 } else { 0 });
    }
    None
}

fn dict_get_string(dict: CFDictionaryRef, key: &str) -> Option<String> {
    let k = cfstr(key);
    let val_ptr = unsafe { CFDictionaryGetValue(dict, k) };
    unsafe { CFRelease(k) };
    if val_ptr.is_null() {
        return None;
    }
    let tid = unsafe { CFGetTypeID(val_ptr) };
    if tid == unsafe { CFStringGetTypeID() } {
        return cfstr_to_string(val_ptr as CFStringRef);
    }
    None
}

fn check_acpi_node_exists(path: &str) -> bool {
    let cpath = CString::new(path).unwrap();
    let entry = unsafe { IORegistryEntryFromPath(0, cpath.as_ptr()) };
    if entry != 0 {
        unsafe { IOObjectRelease(entry) };
        true
    } else {
        false
    }
}

fn read_kext_telemetry() -> Option<LiveTelemetry> {
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
        t.s0ix_handoff = dict_get_u64(dict, "S0ixHandoffSupported").unwrap_or(0) != 0;
        t.sleep_handoff_count = dict_get_u64(dict, "SleepHandoffCount").unwrap_or(0) as u32;
        unsafe { CFRelease(props as CFTypeRef) };
    }
    unsafe { IOObjectRelease(service) };

    t.vpc0_ok = check_acpi_node_exists("IOACPIPlane:/_SB/PC00/LPCB/EC0/VPC0");
    t.tzs0_ok = check_acpi_node_exists("IOACPIPlane:/TZS0");
    t.ietm_ok = check_acpi_node_exists("IOACPIPlane:/_SB/IETM");

    Some(t)
}

fn set_kext_policy(policy: ThinkFlowPolicy) -> Result<(), String> {
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

// ============================================================================
// Direct CFArray / CFData `pmtelemetry_cpu` Sampler (Fallback & Frequencies)
// ============================================================================

struct PmTelemetrySampler {
    sub: IOReportSubscriptionRef,
    subbed: CFMutableDictionaryRef,
    prev_map: HashMap<String, u64>,
    prev_time: Instant,
}

#[derive(Debug, Default, Clone)]
pub struct CpuFreqMetrics {
    pub pkg_power_w: f64,
    pub core_power_w: f64,
    pub p_freq_ghz: f64,
    pub e_freq_ghz: f64,
    pub therm_limit_delta: u64,
    pub raw_pkg_energy: u64,
}

impl PmTelemetrySampler {
    fn new() -> Option<Self> {
        let grp = cfstr("pmtelemetry_cpu");
        let ch = unsafe { IOReportCopyChannelsInGroup(grp, ptr::null(), 0, 0, 0) };
        unsafe { CFRelease(grp) };
        if ch.is_null() {
            return None;
        }
        let mut subbed: CFMutableDictionaryRef = ptr::null_mut();
        let sub = unsafe { IOReportCreateSubscription(ptr::null(), ch, &mut subbed, 0, ptr::null()) };
        unsafe { CFRelease(ch as CFTypeRef) };
        if sub.is_null() || subbed.is_null() {
            return None;
        }
        let prev_map = Self::take_snapshot(sub, subbed);
        Some(Self {
            sub,
            subbed,
            prev_map,
            prev_time: Instant::now(),
        })
    }

    fn take_snapshot(sub: IOReportSubscriptionRef, subbed: CFMutableDictionaryRef) -> HashMap<String, u64> {
        let mut out = HashMap::new();
        let sample_dict = unsafe { IOReportCreateSamples(sub, subbed, ptr::null()) };
        if sample_dict.is_null() {
            return out;
        }

        let k_channels = cfstr("IOReportChannels");
        let k_legend = cfstr("LegendChannel");
        let k_raw = cfstr("RawElements");

        let arr = unsafe { CFDictionaryGetValue(sample_dict, k_channels) as CFArrayRef };
        if !arr.is_null() {
            let count = unsafe { CFArrayGetCount(arr) };
            for i in 0..count {
                let item = unsafe { CFArrayGetValueAtIndex(arr, i) as CFDictionaryRef };
                if item.is_null() {
                    continue;
                }
                let legend = unsafe { CFDictionaryGetValue(item, k_legend) as CFArrayRef };
                let raw_data = unsafe { CFDictionaryGetValue(item, k_raw) as CFDataRef };
                if legend.is_null() || raw_data.is_null() {
                    continue;
                }
                if unsafe { CFArrayGetCount(legend) } >= 3 && unsafe { CFDataGetLength(raw_data) } >= 40 {
                    let name_cf = unsafe { CFArrayGetValueAtIndex(legend, 2) as CFStringRef };
                    let bytes_ptr = unsafe { CFDataGetBytePtr(raw_data) };
                    if let Some(name) = cfstr_to_string(name_cf) {
                        let val = unsafe {
                            let mut buf = [0u8; 8];
                            ptr::copy_nonoverlapping(bytes_ptr.add(32), buf.as_mut_ptr(), 8);
                            u64::from_le_bytes(buf)
                        };
                        out.insert(name, val);
                    }
                }
            }
        }

        unsafe {
            CFRelease(k_channels);
            CFRelease(k_legend);
            CFRelease(k_raw);
            CFRelease(sample_dict as CFTypeRef);
        }
        out
    }

    fn sample_delta(&mut self) -> CpuFreqMetrics {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_time).as_secs_f64().max(0.05);
        let curr_map = Self::take_snapshot(self.sub, self.subbed);

        let pkg0 = *self.prev_map.get("MSR_IA32_PKG_ENERGY_STATUS").unwrap_or(&0) as u32;
        let pkg1 = *curr_map.get("MSR_IA32_PKG_ENERGY_STATUS").unwrap_or(&0) as u32;
        let pp0_0 = *self.prev_map.get("MSR_IA32_PP0_ENERGY_STATUS").unwrap_or(&0) as u32;
        let pp0_1 = *curr_map.get("MSR_IA32_PP0_ENERGY_STATUS").unwrap_or(&0) as u32;

        let pkg_j = pkg1.wrapping_sub(pkg0) as f64 / 16384.0;
        let pp0_j = pp0_1.wrapping_sub(pp0_0) as f64 / 16384.0;

        const TSC_GHZ: f64 = 2.9184;
        let mut p_sum = 0.0;
        let mut p_cnt = 0;
        for i in (0..12).step_by(2) {
            let da = curr_map
                .get(&format!("MSR_IA32_ACNT {}", i))
                .unwrap_or(&0)
                .wrapping_sub(*self.prev_map.get(&format!("MSR_IA32_ACNT {}", i)).unwrap_or(&0));
            let dm = curr_map
                .get(&format!("MSR_IA32_MCNT {}", i))
                .unwrap_or(&0)
                .wrapping_sub(*self.prev_map.get(&format!("MSR_IA32_MCNT {}", i)).unwrap_or(&0));
            if dm > 0 {
                p_sum += TSC_GHZ * (da as f64) / (dm as f64);
                p_cnt += 1;
            }
        }

        let mut e_sum = 0.0;
        let mut e_cnt = 0;
        for i in 12..20 {
            let da = curr_map
                .get(&format!("MSR_IA32_ACNT {}", i))
                .unwrap_or(&0)
                .wrapping_sub(*self.prev_map.get(&format!("MSR_IA32_ACNT {}", i)).unwrap_or(&0));
            let dm = curr_map
                .get(&format!("MSR_IA32_MCNT {}", i))
                .unwrap_or(&0)
                .wrapping_sub(*self.prev_map.get(&format!("MSR_IA32_MCNT {}", i)).unwrap_or(&0));
            if dm > 0 {
                e_sum += TSC_GHZ * (da as f64) / (dm as f64);
                e_cnt += 1;
            }
        }

        let therm_delta = curr_map
            .get("CPU LIMIT THERMAL")
            .unwrap_or(&0)
            .saturating_sub(*self.prev_map.get("CPU LIMIT THERMAL").unwrap_or(&0));

        self.prev_map = curr_map;
        self.prev_time = now;

        CpuFreqMetrics {
            pkg_power_w: pkg_j / dt,
            core_power_w: pp0_j / dt,
            p_freq_ghz: if p_cnt > 0 { p_sum / (p_cnt as f64) } else { 0.0 },
            e_freq_ghz: if e_cnt > 0 { e_sum / (e_cnt as f64) } else { 0.0 },
            therm_limit_delta: therm_delta,
            raw_pkg_energy: pkg1 as u64,
        }
    }
}

// ============================================================================
// Interactive TUI Application State & Visual Helpers
// ============================================================================

pub(crate) struct AppState {
    pub hostname: String,
    pub cpu_brand: String,
    pub telemetry: LiveTelemetry,
    pub temp_history: VecDeque<u64>,
    pub status_message: String,
    pub status_time: Instant,
    pub engine_fallback: ThinkFlowEngine,
    pub sampler: Option<PmTelemetrySampler>,
}

impl AppState {
    pub fn new() -> Self {
        let hostname = read_sysctl_string("kern.hostname")
            .unwrap_or_else(|| "MacBook-Pro".into())
            .trim_end_matches(".local")
            .to_string();
        let cpu_brand = read_sysctl_string("machdep.cpu.brand_string")
            .unwrap_or_else(|| "13th Gen Intel Core i7-13700H".into());
        let sampler = PmTelemetrySampler::new();

        let mut app = Self {
            hostname,
            cpu_brand,
            telemetry: LiveTelemetry::default(),
            temp_history: VecDeque::with_capacity(60),
            status_message: "NOMINAL".into(),
            status_time: Instant::now(),
            engine_fallback: ThinkFlowEngine::new(),
            sampler,
        };
        app.update_telemetry();
        app
    }

    pub fn set_status(&mut self, msg: &str) {
        self.status_message = msg.to_string();
        self.status_time = Instant::now();
    }

    pub fn update_telemetry(&mut self) {
        let freq_m = if let Some(ref mut s) = self.sampler {
            s.sample_delta()
        } else {
            CpuFreqMetrics::default()
        };

        if let Some(mut kext) = read_kext_telemetry() {
            kext.p_freq_ghz = freq_m.p_freq_ghz;
            kext.e_freq_ghz = freq_m.e_freq_ghz;
            kext.core_power_w = freq_m.core_power_w;
            if kext.pkg_power_w < 0.1 && freq_m.pkg_power_w > 0.1 {
                kext.pkg_power_w = freq_m.pkg_power_w;
            }
            self.telemetry = kext;
        } else {
            // Standalone Fallback Simulation (kext not loaded yet)
            let lpm = read_sysctl_i32("machdep.xcpm.lpm_enabled").unwrap_or(0);
            let mut inp = ThinkFlowTelemetryInput::default();
            inp.msr_temp_target = 100 << 16;
            let est_temp = if freq_m.therm_limit_delta > 0 {
                98
            } else {
                (42.0 + freq_m.pkg_power_w * 0.95).min(97.0) as u32
            };
            inp.msr_pkg_therm = (1 << 31)
                | ((100u32.saturating_sub(est_temp) as u64) << 16)
                | (if freq_m.therm_limit_delta > 0 { 1 } else { 0 });
            inp.msr_pkg_energy = freq_m.raw_pkg_energy;
            inp.msr_rapl_unit = 14 << 8;
            inp.lpm_sysctl = lpm;
            inp.elapsed_ms = 1000;

            let out = self.engine_fallback.step(&inp);

            let profile_name = match out.target_profile {
                1 => "Quiet",
                2 => "Balance",
                3 => "Performance",
                4 => "Cool",
                _ => "Unknown",
            };

            let policy_name = match self.engine_fallback.policy {
                ThinkFlowPolicy::Quiet => "Quiet",
                ThinkFlowPolicy::Balance => "Balance",
                ThinkFlowPolicy::Performance => "Performance",
                ThinkFlowPolicy::Cool => "Cool",
                ThinkFlowPolicy::Auto => "Auto",
            };

            self.telemetry = LiveTelemetry {
                loaded: false,
                policy_mode: self.engine_fallback.policy as u32,
                policy_name: policy_name.into(),
                active_profile: out.target_profile,
                profile_name: profile_name.into(),
                cpu_temp_c: est_temp,
                tjmax_c: 100,
                pkg_power_w: freq_m.pkg_power_w,
                core_power_w: freq_m.core_power_w,
                p_freq_ghz: freq_m.p_freq_ghz,
                e_freq_ghz: freq_m.e_freq_ghz,
                ec_temp0_c: est_temp.saturating_sub(2),
                ec_temp1_c: est_temp.saturating_sub(8),
                ec_fan_raw: if out.target_profile == 3 { 79 } else if out.target_profile == 2 || out.target_profile == 4 { 45 } else { 0 },
                ec_fan_rpm: if out.target_profile == 3 { 3800 } else if out.target_profile == 2 || out.target_profile == 4 { 2200 } else { 0 },
                ec_pfmm: if out.target_profile == 3 { 1 } else if out.target_profile == 2 || out.target_profile == 4 { 2 } else { 0 },
                ec_itsm: if out.target_profile == 3 { 4 } else if out.target_profile == 2 || out.target_profile == 4 { 2 } else { 1 },
                ec_lxos: 0x00,
                ec_tpa1: 1,
                hwp_max_ratio: if lpm != 0 { 24 } else { 50 },
                hwp_epp: if lpm != 0 { 0xC0 } else { 0x80 },
                is_lpm_active: lpm != 0,
                is_throttling: freq_m.therm_limit_delta > 0,
                hysteresis_counter: out.hysteresis_counter,
                total_ticks: self.engine_fallback.total_ticks,
                mode_switch_count: self.engine_fallback.mode_switch_count,
                ssdt_present: check_acpi_node_exists("IOACPIPlane:/_SB/PC00/LPCB/EC0/VPC0"),
                s0ix_handoff: false,
                sleep_handoff_count: 0,
                vpc0_ok: check_acpi_node_exists("IOACPIPlane:/_SB/PC00/LPCB/EC0/VPC0"),
                tzs0_ok: check_acpi_node_exists("IOACPIPlane:/TZS0"),
                ietm_ok: check_acpi_node_exists("IOACPIPlane:/_SB/IETM"),
            };
        }

        // Push current CPU temperature to rolling 60-sample sparkline history
        if self.temp_history.len() >= 60 {
            self.temp_history.pop_front();
        }
        self.temp_history.push_back(self.telemetry.cpu_temp_c as u64);
    }

    pub fn cycle_policy(&mut self) {
        let next_policy = match self.telemetry.policy_mode {
            0 => ThinkFlowPolicy::Cool,
            4 => ThinkFlowPolicy::Quiet,
            1 => ThinkFlowPolicy::Balance,
            2 => ThinkFlowPolicy::Performance,
            _ => ThinkFlowPolicy::Auto,
        };
        self.set_policy(next_policy);
    }

    pub fn set_policy(&mut self, policy: ThinkFlowPolicy) {
        if self.telemetry.loaded {
            match set_kext_policy(policy) {
                Ok(()) => {
                    self.set_status(&format!("Policy set to {:?}", policy));
                }
                Err(e) => {
                    self.set_status(&format!("Kext err: {}", e));
                }
            }
        } else {
            self.engine_fallback.set_policy(policy);
            self.set_status(&format!("Fallback policy set to {:?}", policy));
        }
        self.update_telemetry();
    }

    pub fn reset_counters(&mut self) {
        self.temp_history.clear();
        for _ in 0..30 {
            self.temp_history.push_back(self.telemetry.cpu_temp_c as u64);
        }
        self.set_status("Counters & History buffer reset");
    }
}

// ============================================================================
// Block Gauge Drawing Routine
// ============================================================================

fn make_bar(ratio: f64, width: usize, color: Color) -> Line<'static> {
    let clamped = ratio.clamp(0.0, 1.0);
    let filled_len = (clamped * width as f64).round() as usize;
    let empty_len = width.saturating_sub(filled_len);

    let filled = "█".repeat(filled_len);
    let empty = "░".repeat(empty_len);

    Line::from(vec![
        Span::raw("["),
        Span::styled(filled, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(empty, Style::default().fg(Color::DarkGray)),
        Span::raw("]"),
    ])
}

fn temp_color(c: u32) -> Color {
    if c >= 85 {
        Color::LightRed
    } else if c >= 70 {
        Color::LightYellow
    } else {
        Color::Cyan
    }
}

fn power_color(w: f64) -> Color {
    if w >= 45.0 {
        Color::LightRed
    } else if w >= 25.0 {
        Color::LightYellow
    } else {
        Color::Green
    }
}

// ============================================================================
// UI Render Function (Pixel-Perfect Alignment with User Mockup)
// ============================================================================

fn draw_ui(frame: &mut Frame, state: &AppState) {
    let size = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header Banner
            Constraint::Length(10), // CPU Telemetry & EC Fan Cards
            Constraint::Length(7),  // 60s Sparkline Chart Card
            Constraint::Length(5),  // 3-Column ACPI/EC Cards
            Constraint::Length(1),  // Key Navigation Bar
        ])
        .split(size);

    // ------------------------------------------------------------------------
    // 1. Top Header Banner
    // ------------------------------------------------------------------------
    let status_tag = if state.telemetry.loaded {
        Span::styled(" [ ACTIVE: LenovoThinkFlow.kext ] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" [ STANDBY: Direct EC Mode ] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    };

    let header_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Blue))
        .title(Line::from(vec![
            Span::styled(concat!(" LenovoThinkFlow v", env!("CARGO_PKG_VERSION"), " "), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            status_tag,
        ]));

    let short_cpu = if state.cpu_brand.contains("i7-13700H") {
        "i7-13700H (14C/20T)"
    } else {
        &state.cpu_brand
    };

    let header_line = Line::from(vec![
        Span::raw("  Host: "),
        Span::styled(&state.hostname, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::raw("  │  CPU: "),
        Span::styled(short_cpu, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::raw("  │  Ticks: "),
        Span::styled(format!("{}", state.telemetry.total_ticks), Style::default().fg(Color::Yellow)),
        Span::raw("  │  Hysteresis: "),
        Span::styled(format!("{}t", state.telemetry.hysteresis_counter), Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
    ]);

    let header_p = Paragraph::new(header_line).block(header_block);
    frame.render_widget(header_p, chunks[0]);

    // ------------------------------------------------------------------------
    // 2. Middle Row: CPU Telemetry (Left) & EC Fan / Policy (Right)
    // ------------------------------------------------------------------------
    let mid_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    // Left Card: CPU Telemetry & Throttle
    let left_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(" CPU Telemetry & Throttle ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));

    let t_c = state.telemetry.cpu_temp_c;
    let tjmax = state.telemetry.tjmax_c.max(80);
    let t_pct = ((t_c as f64 / tjmax as f64) * 100.0).round().min(100.0) as u32;
    let t_color = temp_color(t_c);

    let pkg_w = state.telemetry.pkg_power_w;
    let p_pct = ((pkg_w / 65.0) * 100.0).round().min(100.0) as u32;
    let p_color = power_color(pkg_w);

    let bar_width = 30;
    let t_bar = make_bar(t_c as f64 / tjmax as f64, bar_width, t_color);
    let p_bar = make_bar(pkg_w / 65.0, bar_width, p_color);

    let throttle_span = if state.telemetry.is_throttling {
        Span::styled("[YES]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("[NO]", Style::default().fg(Color::Green))
    };

    let cpu_lines = vec![
        Line::from(vec![
            Span::raw(" Temp:  "),
            Span::styled(format!("{}°C", t_c), Style::default().fg(t_color).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" / {}°C TjMax", tjmax), Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            t_bar.spans[0].clone(),
            t_bar.spans[1].clone(),
            t_bar.spans[2].clone(),
            t_bar.spans[3].clone(),
            Span::styled(format!("  {:>2}%", t_pct), Style::default().fg(t_color)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::raw(" Power: "),
            Span::styled(format!("{:.2} W", pkg_w), Style::default().fg(p_color).add_modifier(Modifier::BOLD)),
            Span::styled(" (Package)", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            p_bar.spans[0].clone(),
            p_bar.spans[1].clone(),
            p_bar.spans[2].clone(),
            p_bar.spans[3].clone(),
            Span::styled(format!("  {:>2}%", p_pct), Style::default().fg(p_color)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::raw(" HWP:   Max "),
            Span::styled(format!("{}x", state.telemetry.hwp_max_ratio), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::raw("  │  EPP: "),
            Span::styled(format!("0x{:02X}", state.telemetry.hwp_epp), Style::default().fg(Color::White)),
            Span::raw("  │  Throttle: "),
            throttle_span,
        ]),
    ];

    let cpu_p = Paragraph::new(cpu_lines).block(left_block);
    frame.render_widget(cpu_p, mid_chunks[0]);

    // Right Card: EC Fan & Policy Control
    let right_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green))
        .title(Span::styled(" EC Fan & Policy Control ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));

    let mode_str = match state.telemetry.policy_mode {
        1 => "[ QUIET ] Manual Force",
        2 => "[ BALANCE ] Manual Force",
        3 => "[ PERFORMANCE ] Manual Force",
        4 => "[ COOL ] Low Power + Adaptive Fan",
        _ => "[ AUTO ] Adaptive Scheduler",
    };

    let profile_badge = match state.telemetry.active_profile {
        1 => Span::styled("[ QUIET / SILENT ]", Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD)),
        2 => Span::styled("[ BALANCE / AUTO ]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        3 => Span::styled("[ BEAST / PERFORMANCE ]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        4 => Span::styled("[ COOL / BREEZY ]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        _ => Span::styled("[ UNKNOWN ]", Style::default().fg(Color::Yellow)),
    };

    let raw_fan = state.telemetry.ec_fan_raw;
    let fan_pct = raw_fan.min(100);
    let fan_bar = make_bar(fan_pct as f64 / 100.0, 24, Color::Cyan);

    let lpm_status = if state.telemetry.is_lpm_active {
        Span::styled("ON  (machdep.xcpm.lpm = 1)", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("OFF (machdep.xcpm.lpm = 0)", Style::default().fg(Color::DarkGray))
    };

    let fan_lines = vec![
        Line::from(vec![
            Span::raw(" Mode:      "),
            Span::styled(mode_str, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw(" Profile:   "),
            profile_badge,
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::raw(" Fan Speed: "),
            Span::styled(format!("0x{:02X} ({}%)", raw_fan, fan_pct), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            fan_bar.spans[0].clone(),
            fan_bar.spans[1].clone(),
            fan_bar.spans[2].clone(),
            fan_bar.spans[3].clone(),
            Span::styled(format!(" {:>4} RPM", state.telemetry.ec_fan_rpm), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::raw(" Low Power: "),
            lpm_status,
        ]),
    ];

    let fan_p = Paragraph::new(fan_lines).block(right_block);
    frame.render_widget(fan_p, mid_chunks[1]);

    // ------------------------------------------------------------------------
    // 3. Middle-Bottom Card: Temperature Trend (Last 60s Sparkline Chart)
    // ------------------------------------------------------------------------
    let trend_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(" Temperature Trend (Last 60s) ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));

    let spark_inner = trend_block.inner(chunks[2]);
    frame.render_widget(trend_block, chunks[2]);

    let spark_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(8), Constraint::Min(10)])
        .split(spark_inner);

    let y_axis = vec![
        Line::from(Span::styled(" 85°C ┤", Style::default().fg(Color::LightRed))),
        Line::from(Span::styled(" 80°C ┤", Style::default().fg(Color::LightYellow))),
        Line::from(Span::styled(" 75°C ┤", Style::default().fg(Color::Cyan))),
        Line::from(Span::styled(" 70°C ┤", Style::default().fg(Color::Blue))),
        Line::from(Span::styled(" 65°C ┤", Style::default().fg(Color::DarkGray))),
    ];
    let y_p = Paragraph::new(y_axis);
    frame.render_widget(y_p, spark_layout[0]);

    let data_vec: Vec<u64> = state.temp_history.iter().copied().collect();
    let sparkline = Sparkline::default()
        .data(&data_vec)
        .max(100)
        .style(Style::default().fg(temp_color(state.telemetry.cpu_temp_c)));
    frame.render_widget(sparkline, spark_layout[1]);

    // ------------------------------------------------------------------------
    // 4. Bottom 3-Column Cards: ACPI Nodes / EC Raw Registers / Hardware Flags
    // ------------------------------------------------------------------------
    let bot_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(chunks[3]);

    // Col 1: ACPI Nodes
    let acpi_b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Blue))
        .title(Span::styled(" ACPI Nodes ", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)));

    let node_dot = |ok: bool| {
        if ok {
            Span::styled("● OK", Style::default().fg(Color::Green))
        } else {
            Span::styled("● Missing", Style::default().fg(Color::Red))
        }
    };

    let acpi_lines = vec![
        Line::from(vec![
            Span::raw(" VPC0 (VPC2004)    "),
            node_dot(state.telemetry.vpc0_ok),
        ]),
        Line::from(vec![
            Span::raw(" TZS0 / TZS1       "),
            node_dot(state.telemetry.tzs0_ok),
        ]),
        Line::from(vec![
            Span::raw(" IETM Driver       "),
            node_dot(state.telemetry.ietm_ok),
        ]),
    ];
    let acpi_p = Paragraph::new(acpi_lines).block(acpi_b);
    frame.render_widget(acpi_p, bot_cols[0]);

    // Col 2: EC Raw Registers
    let ec_b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(" EC Raw Registers ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)));

    let ec_lines = vec![
        Line::from(vec![
            Span::raw(" THS0 (CPU Die)      "),
            Span::styled(format!("{}°C", state.telemetry.ec_temp0_c), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::raw(" THS1 (GPU/Skin)     "),
            Span::styled(format!("{}°C", state.telemetry.ec_temp1_c), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::raw(" PFMM / ITSM         "),
            Span::styled(format!("{} / {}", state.telemetry.ec_pfmm, state.telemetry.ec_itsm), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
    ];
    let ec_p = Paragraph::new(ec_lines).block(ec_b);
    frame.render_widget(ec_p, bot_cols[1]);

    // Col 3: Hardware Flags
    let hw_b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Hardware Flags ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));

    let ssdt_badge = if state.telemetry.ssdt_present && state.telemetry.s0ix_handoff {
        Span::styled(
            format!("S0ix OK ({})", state.telemetry.sleep_handoff_count),
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )
    } else if state.telemetry.ssdt_present {
        Span::styled("LOADED", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("DIRECT-EC", Style::default().fg(Color::Yellow))
    };

    let hw_lines = vec![
        Line::from(vec![
            Span::raw(" LXOS Status         "),
            Span::styled(format!("0x{:02X}", state.telemetry.ec_lxos), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::raw(" TPA1 Flag           "),
            Span::styled(format!("{}", state.telemetry.ec_tpa1), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::raw(" SSDT-THINKFLOW      "),
            ssdt_badge,
        ]),
    ];
    let hw_p = Paragraph::new(hw_lines).block(hw_b);
    frame.render_widget(hw_p, bot_cols[2]);

    // ------------------------------------------------------------------------
    // 5. Bottom Navigation Key Bar
    // ------------------------------------------------------------------------
    let nav_line = Line::from(vec![
        Span::styled(" [Q]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" Quit  │ "),
        Span::styled("[M]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" Mode  │ "),
        Span::styled("[D]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" Default (Auto)  │ "),
        Span::styled("[F]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" Max Fan  │ "),
        Span::styled("[R]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" Reset  │ "),
        Span::raw("Status: "),
        Span::styled(&state.status_message, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
    ]);
    let nav_p = Paragraph::new(nav_line);
    frame.render_widget(nav_p, chunks[4]);
}

// ============================================================================
// Interactive TUI Event Loop
// ============================================================================

fn run_tui() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = AppState::new();
    let mut last_sample = Instant::now();

    loop {
        terminal.draw(|f| draw_ui(f, &state))?;

        // Handle keyboard input events (poll for 250ms max)
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                            break;
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            state.cycle_policy();
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            state.set_policy(ThinkFlowPolicy::Auto);
                            state.set_status("Restored Default (Auto) mode");
                        }
                        KeyCode::Char('f') | KeyCode::Char('F') => {
                            state.set_policy(ThinkFlowPolicy::Performance);
                            state.set_status("Forced Performance / Beast mode!");
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') => {
                            state.reset_counters();
                        }
                        _ => {}
                    }
                }
            }
        }

        // Periodic telemetry sample update (every 1 second)
        if last_sample.elapsed() >= Duration::from_millis(1000) {
            state.update_telemetry();
            last_sample = Instant::now();

            // Clear status message back to NOMINAL after 4 seconds
            if state.status_time.elapsed() >= Duration::from_secs(4) && state.status_message != "NOMINAL" {
                state.status_message = "NOMINAL".into();
            }
        }
    }

    // Clean up and restore terminal state
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

// ============================================================================
// CLI Subcommands (Plain Status & Stress Benchmark)
// ============================================================================

fn print_status() {
    let mut state = AppState::new();
    state.update_telemetry();
    let t = &state.telemetry;

    println!("========================================================================");
    println!(concat!("  LenovoThinkFlow v", env!("CARGO_PKG_VERSION"), " — Lenovo EC Thermal & Fan Scheduler for macOS"));
    println!("========================================================================");
    println!("  Host Name            : {}", state.hostname);
    println!("  CPU Model            : {}", state.cpu_brand);
    println!(
        "  macOS Low Power Mode : {} (sysctl machdep.xcpm.lpm_enabled = {})",
        if t.is_lpm_active {
            "ON  [低电量模式已开启 -> 联动 EC 清凉模式]"
        } else {
            "OFF [正常模式 -> 均衡/满载野兽档自动切换]"
        },
        if t.is_lpm_active { 1 } else { 0 }
    );
    println!(
        "  ACPI Hardware Nodes  : VPC0(VPC2004)={} | TZS0/TZS1={} | IETM={}",
        if t.vpc0_ok { "OK" } else { "Missing" },
        if t.tzs0_ok { "OK" } else { "Missing" },
        if t.ietm_ok { "OK" } else { "Missing" }
    );
    println!("------------------------------------------------------------------------");

    if t.loaded {
        println!("  [Kernel Driver Status: ACTIVE (LenovoThinkFlow.kext loaded)]");
        println!("  Active Policy        : {} (Mode {})", t.policy_name, t.policy_mode);
        println!("  Active EC Profile    : {} (PFMM={}, ITSM={})", t.profile_name, t.ec_pfmm, t.ec_itsm);
        println!(
            "  CPU Telemetry        : {} °C (TjMax {} °C) | Package Power: {:.2} W (PP0 {:.2} W)",
            t.cpu_temp_c, t.tjmax_c, t.pkg_power_w, t.core_power_w
        );
        println!(
            "  Active Core Freq     : P-Core = {:.2} GHz | E-Core = {:.2} GHz",
            t.p_freq_ghz, t.e_freq_ghz
        );
        println!(
            "  HWP Request          : Max Ratio = {}x | EPP = 0x{:02X} | Throttling = {}",
            t.hwp_max_ratio,
            t.hwp_epp,
            if t.is_throttling { "YES" } else { "No" }
        );
        println!(
            "  Lenovo EC Registers  : THS0={}°C, THS1={}°C | FANS=0x{:02X} (~{} RPM)",
            t.ec_temp0_c, t.ec_temp1_c, t.ec_fan_raw, t.ec_fan_rpm
        );
        println!(
            "  EC Standalone Flags  : LXOS=0x{:02X} | TPA1={} | SSDT-THINKFLOW={} | S0ix Handoff={}",
            t.ec_lxos,
            t.ec_tpa1,
            if t.ssdt_present { "Loaded" } else { "Direct EC Mode" },
            if t.s0ix_handoff { "TFSL Ready" } else { "Direct EC" }
        );
        println!(
            "  Engine Counters      : Total Ticks = {} | Switches = {} | Hysteresis = {}t | Sleep Handoffs = {}",
            t.total_ticks, t.mode_switch_count, t.hysteresis_counter, t.sleep_handoff_count
        );
    } else {
        println!("  [Kernel Driver Status: STANDBY (Direct EC / Emulated Fallback)]");
        println!(
            "  CPU Telemetry        : {} °C | Package Power: {:.2} W (PP0 {:.2} W)",
            t.cpu_temp_c, t.pkg_power_w, t.core_power_w
        );
        println!(
            "  Active Core Freq     : P-Core = {:.2} GHz | E-Core = {:.2} GHz",
            t.p_freq_ghz, t.e_freq_ghz
        );
        println!("  Simulated Profile    : {} (PFMM={}, ITSM={})", t.profile_name, t.ec_pfmm, t.ec_itsm);
    }
    println!("========================================================================");
}

fn handle_mode_command(mode_arg: &str) {
    let (policy, name) = match mode_arg.to_ascii_lowercase().as_str() {
        "auto" | "default" => (ThinkFlowPolicy::Auto, "Auto (Adaptive)"),
        "cool" | "breeze" | "icy" => (ThinkFlowPolicy::Cool, "Cool (Low Power + Adaptive Fan)"),
        "quiet" | "silent" => (ThinkFlowPolicy::Quiet, "Quiet (Fan Suppressed)"),
        "balance" | "normal" => (ThinkFlowPolicy::Balance, "Balance (Normal)"),
        "performance" | "perf" | "beast" | "turbo" => (ThinkFlowPolicy::Performance, "Performance (Beast)"),
        _ => {
            eprintln!("Invalid mode '{}'. Expected one of: auto | cool | quiet | balance | performance", mode_arg);
            std::process::exit(1);
        }
    };

    println!("Switching LenovoThinkFlow Policy -> {}", name);
    match set_kext_policy(policy) {
        Ok(()) => println!("Successfully updated LenovoThinkFlow.kext kernel policy!"),
        Err(msg) => println!("Note: {}", msg),
    }
}

fn print_help() {
    println!(concat!("LenovoThinkFlow v", env!("CARGO_PKG_VERSION"), " — Lenovo EC Thermal & Fan Control CLI & TUI"));
    println!();
    println!("USAGE:");
    println!("  thinkflow                               Launch the full interactive TUI monitor");
    println!("  thinkflow status                        Print current CPU & EC status once");
    println!("  thinkflow mode <auto|cool|quiet|balance|perf> Switch ThinkFlow thermal/fan policy");
    println!("  thinkflow help                          Show this help message");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");

    match cmd {
        "" | "tui" | "monitor" | "watch" => {
            if let Err(e) = run_tui() {
                eprintln!("Error running TUI: {}", e);
            }
        }
        "status" | "info" => print_status(),
        "mode" | "set" => {
            if let Some(m) = args.get(2) {
                handle_mode_command(m);
            } else {
                eprintln!("Usage: thinkflow mode <auto|cool|quiet|balance|performance>");
                std::process::exit(1);
            }
        }
        "-h" | "--help" | "help" => print_help(),
        _ => {
            print_help();
            std::process::exit(1);
        }
    }
}
