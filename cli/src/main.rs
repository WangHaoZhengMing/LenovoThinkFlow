use std::collections::HashMap;
use std::env;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use thinkflow_core::{
    ThinkFlowEngine, ThinkFlowPolicy, ThinkFlowProfile, ThinkFlowTelemetryInput, DYTC_SET_APM_OFF,
    DYTC_SET_APM_ON, DYTC_SET_AQM_OFF, DYTC_SET_AQM_ON, DYTC_SET_MMC_AUTO, DYTC_SET_MMC_PERF,
    DYTC_SET_MMC_QUIET,
};

// ============================================================================
// CoreFoundation & IOKit / libIOReport FFI Bindings (Zero external crates!)
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
    fn CFNumberGetValue(number: CFNumberRef, the_type: CFIndex, value_ptr: *mut c_void) -> bool;
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
    fn IOServiceGetMatchingService(main_port: MachPortT, matching: CFDictionaryRef) -> IoServiceT;
    fn IORegistryEntryFromPath(main_port: MachPortT, path: *const c_char) -> IoRegistryEntryT;
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
// Helper Wrappers for CFString / sysctl / IORegistry
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

fn read_sysctl_u64(name: &str) -> Option<u64> {
    let cname = CString::new(name).ok()?;
    let mut val: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let ret = unsafe {
        sysctlbyname(
            cname.as_ptr(),
            &mut val as *mut u64 as *mut c_void,
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

fn detect_tsc_ghz() -> f64 {
    if let Some(hz) = read_sysctl_u64("machdep.tsc.frequency").filter(|&v| v > 100_000_000) {
        return hz as f64 / 1_000_000_000.0;
    }
    if let Some(hz) = read_sysctl_u64("hw.cpufrequency").filter(|&v| v > 100_000_000) {
        return hz as f64 / 1_000_000_000.0;
    }
    2.9184
}

/// Detect (p_core_threads, total_logical_threads) dynamically across hybrid & non-hybrid Intel CPUs
fn detect_cpu_thread_split() -> (usize, usize) {
    let total = read_sysctl_i32("hw.logicalcpu")
        .filter(|&v| v > 0)
        .map(|v| v as usize)
        .unwrap_or(20);
    let p_threads = read_sysctl_i32("hw.perflevel0.logicalcpu")
        .filter(|&v| v > 0 && (v as usize) <= total)
        .map(|v| v as usize)
        .unwrap_or_else(|| if total == 20 { 12 } else { total });
    (p_threads, total)
}

#[derive(Debug, Default, Clone)]
struct KextTelemetry {
    loaded: bool,
    policy_mode: u32,
    policy_name: String,
    active_profile: u32,
    cpu_temp_c: u32,
    tjmax_c: u32,
    pkg_power_mw: u32,
    ec_temp0_c: u32,
    ec_temp1_c: u32,
    ec_fan_raw: u32,
    ec_pfmm: u32,
    ec_itsm: u32,
    ec_lxos: u32,
    ec_tpa1: u32,
    hwp_max_ratio: u32,
    hwp_epp: u32,
    is_throttling: bool,
    hysteresis_counter: u32,
    mode_switch_count: u32,
    total_ticks: u64,
    ssdt_present: bool,
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
        return Some(if unsafe { CFBooleanGetValue(val_ptr) } {
            1
        } else {
            0
        });
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

fn read_kext_telemetry() -> KextTelemetry {
    let mut t = KextTelemetry::default();
    let cname = CString::new("ThinkFlow").unwrap();
    let service = unsafe { IOServiceGetMatchingService(0, IOServiceMatching(cname.as_ptr())) };
    if service == 0 {
        return t;
    }

    let mut props: CFMutableDictionaryRef = ptr::null_mut();
    let kr =
        unsafe { IORegistryEntryCreateCFProperties(service, &mut props, kCFAllocatorDefault, 0) };
    if kr == 0 && !props.is_null() {
        let dict = props as CFDictionaryRef;
        t.loaded = true;
        t.policy_mode = dict_get_u64(dict, "PolicyMode").unwrap_or(0) as u32;
        t.policy_name = dict_get_string(dict, "PolicyName").unwrap_or_else(|| "Auto".into());
        t.active_profile = dict_get_u64(dict, "ActiveProfile").unwrap_or(0) as u32;
        t.cpu_temp_c = dict_get_u64(dict, "CPUTempC").unwrap_or(0) as u32;
        t.tjmax_c = dict_get_u64(dict, "CPUTjMaxC").unwrap_or(100) as u32;
        t.pkg_power_mw = dict_get_u64(dict, "PackagePowerMW").unwrap_or(0) as u32;
        t.ec_temp0_c = dict_get_u64(dict, "ECTemp0C").unwrap_or(0) as u32;
        t.ec_temp1_c = dict_get_u64(dict, "ECTemp1C").unwrap_or(0) as u32;
        t.ec_fan_raw = dict_get_u64(dict, "ECFanRaw").unwrap_or(0) as u32;
        t.ec_pfmm = dict_get_u64(dict, "ECPFMM").unwrap_or(0) as u32;
        t.ec_itsm = dict_get_u64(dict, "ECITSM").unwrap_or(0) as u32;
        t.ec_lxos = dict_get_u64(dict, "ECLXOS").unwrap_or(0) as u32;
        t.ec_tpa1 = dict_get_u64(dict, "ECTPA1").unwrap_or(0) as u32;
        t.hwp_max_ratio = dict_get_u64(dict, "HWPMaxRatio").unwrap_or(0) as u32;
        t.hwp_epp = dict_get_u64(dict, "HWPEPP").unwrap_or(0) as u32;
        t.is_throttling = dict_get_u64(dict, "ThermalThrottling").unwrap_or(0) != 0;
        t.hysteresis_counter = dict_get_u64(dict, "HysteresisTicks").unwrap_or(0) as u32;
        t.mode_switch_count = dict_get_u64(dict, "ModeSwitchCount").unwrap_or(0) as u32;
        t.total_ticks = dict_get_u64(dict, "TotalTicks").unwrap_or(0);
        t.ssdt_present = dict_get_u64(dict, "SSDTHelperPresent").unwrap_or(0) != 0;
        unsafe { CFRelease(props as CFTypeRef) };
    }
    unsafe { IOObjectRelease(service) };
    t
}

fn check_any_acpi_node_exists(paths: &[&str]) -> bool {
    for path in paths {
        if let Ok(cpath) = CString::new(*path) {
            let entry = unsafe { IORegistryEntryFromPath(0, cpath.as_ptr()) };
            if entry != 0 {
                unsafe { IOObjectRelease(entry) };
                return true;
            }
        }
    }
    false
}

fn set_kext_policy(policy: ThinkFlowPolicy) -> Result<(), String> {
    let cname = CString::new("ThinkFlow").unwrap();
    let service = unsafe { IOServiceGetMatchingService(0, IOServiceMatching(cname.as_ptr())) };
    if service == 0 {
        return Err(
            "ThinkFlow.kext service is not currently loaded in the running kernel.\n\
             (Built kext and SSDT are ready in `build/`; load via kextload or OpenCore to apply to hardware.)"
                .into(),
        );
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
        Err(format!(
            "IORegistryEntrySetCFProperty returned error 0x{:08x}",
            kr
        ))
    }
}

// ============================================================================
// Direct CFArray / CFData `pmtelemetry_cpu` Sampler
// ============================================================================

struct PmTelemetrySampler {
    sub: IOReportSubscriptionRef,
    subbed: CFMutableDictionaryRef,
    prev_map: HashMap<String, u64>,
    prev_time: Instant,
    tsc_ghz: f64,
    p_threads: usize,
    total_threads: usize,
}

#[derive(Debug, Default, Clone)]
struct CpuLiveMetrics {
    pkg_power_w: f64,
    core_power_w: f64,
    p_freq_ghz: f64,
    e_freq_ghz: f64,
    therm_limit_delta: u64,
    pl1_limit_delta: u64,
    raw_pkg_energy: u64,
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
        let sub =
            unsafe { IOReportCreateSubscription(ptr::null(), ch, &mut subbed, 0, ptr::null()) };
        unsafe { CFRelease(ch as CFTypeRef) };
        if sub.is_null() || subbed.is_null() {
            return None;
        }
        let prev_map = Self::take_snapshot(sub, subbed);
        let tsc_ghz = detect_tsc_ghz();
        let (p_threads, total_threads) = detect_cpu_thread_split();
        Some(Self {
            sub,
            subbed,
            prev_map,
            prev_time: Instant::now(),
            tsc_ghz,
            p_threads,
            total_threads,
        })
    }

    fn take_snapshot(
        sub: IOReportSubscriptionRef,
        subbed: CFMutableDictionaryRef,
    ) -> HashMap<String, u64> {
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
                if unsafe { CFArrayGetCount(legend) } >= 3
                    && unsafe { CFDataGetLength(raw_data) } >= 40
                {
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

    fn sample_delta(&mut self) -> CpuLiveMetrics {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_time).as_secs_f64().max(0.05);
        let curr_map = Self::take_snapshot(self.sub, self.subbed);

        let pkg0 = *self
            .prev_map
            .get("MSR_IA32_PKG_ENERGY_STATUS")
            .unwrap_or(&0) as u32;
        let pkg1 = *curr_map.get("MSR_IA32_PKG_ENERGY_STATUS").unwrap_or(&0) as u32;
        let pp0_0 = *self
            .prev_map
            .get("MSR_IA32_PP0_ENERGY_STATUS")
            .unwrap_or(&0) as u32;
        let pp0_1 = *curr_map.get("MSR_IA32_PP0_ENERGY_STATUS").unwrap_or(&0) as u32;

        let pkg_j = pkg1.wrapping_sub(pkg0) as f64 / 16384.0;
        let pp0_j = pp0_1.wrapping_sub(pp0_0) as f64 / 16384.0;

        let mut p_sum = 0.0;
        let mut p_cnt = 0;
        let p_step = if self.p_threads >= 4 { 2 } else { 1 };
        for i in (0..self.p_threads).step_by(p_step) {
            let da = curr_map
                .get(&format!("MSR_IA32_ACNT {}", i))
                .unwrap_or(&0)
                .wrapping_sub(
                    *self
                        .prev_map
                        .get(&format!("MSR_IA32_ACNT {}", i))
                        .unwrap_or(&0),
                );
            let dm = curr_map
                .get(&format!("MSR_IA32_MCNT {}", i))
                .unwrap_or(&0)
                .wrapping_sub(
                    *self
                        .prev_map
                        .get(&format!("MSR_IA32_MCNT {}", i))
                        .unwrap_or(&0),
                );
            if dm > 0 {
                p_sum += self.tsc_ghz * (da as f64) / (dm as f64);
                p_cnt += 1;
            }
        }

        let mut e_sum = 0.0;
        let mut e_cnt = 0;
        for i in self.p_threads..self.total_threads {
            let da = curr_map
                .get(&format!("MSR_IA32_ACNT {}", i))
                .unwrap_or(&0)
                .wrapping_sub(
                    *self
                        .prev_map
                        .get(&format!("MSR_IA32_ACNT {}", i))
                        .unwrap_or(&0),
                );
            let dm = curr_map
                .get(&format!("MSR_IA32_MCNT {}", i))
                .unwrap_or(&0)
                .wrapping_sub(
                    *self
                        .prev_map
                        .get(&format!("MSR_IA32_MCNT {}", i))
                        .unwrap_or(&0),
                );
            if dm > 0 {
                e_sum += self.tsc_ghz * (da as f64) / (dm as f64);
                e_cnt += 1;
            }
        }

        let therm_delta = curr_map
            .get("CPU LIMIT THERMAL")
            .unwrap_or(&0)
            .saturating_sub(*self.prev_map.get("CPU LIMIT THERMAL").unwrap_or(&0));
        let pl1_delta = curr_map
            .get("CPU LIMIT PL1")
            .unwrap_or(&0)
            .saturating_sub(*self.prev_map.get("CPU LIMIT PL1").unwrap_or(&0));

        self.prev_map = curr_map;
        self.prev_time = now;

        CpuLiveMetrics {
            pkg_power_w: pkg_j / dt,
            core_power_w: pp0_j / dt,
            p_freq_ghz: if p_cnt > 0 {
                p_sum / (p_cnt as f64)
            } else {
                0.0
            },
            e_freq_ghz: if e_cnt > 0 {
                e_sum / (e_cnt as f64)
            } else {
                0.0
            },
            therm_limit_delta: therm_delta,
            pl1_limit_delta: pl1_delta,
            raw_pkg_energy: pkg1 as u64,
        }
    }
}

// ============================================================================
// Formatting Helpers & Commands
// ============================================================================

fn profile_label(p: u32) -> &'static str {
    match p {
        1 => "Quiet / Cool (静音省电: PFMM=0, ITSM=1)",
        2 => "Balance / Auto (智能均衡: PFMM=2, ITSM=2)",
        3 => "Performance / Beast (野兽满血: PFMM=1, ITSM=4)",
        _ => "Unknown",
    }
}

fn policy_label(p: u32) -> &'static str {
    match p {
        1 => "Quiet (强制静音)",
        2 => "Balance (强制均衡)",
        3 => "Performance (强制高性能)",
        _ => "Auto (跟随 macOS 低电量模式 + 负载自适应)",
    }
}

fn print_status() {
    let cpu_brand =
        read_sysctl_string("machdep.cpu.brand_string").unwrap_or_else(|| "Unknown CPU".into());
    let lpm = read_sysctl_i32("machdep.xcpm.lpm_enabled").unwrap_or(0);
    let kext = read_kext_telemetry();

    let vpc0_ok = check_any_acpi_node_exists(&[
        "IOACPIPlane:/_SB/PC00/LPCB/EC0/VPC0",
        "IOACPIPlane:/_SB/PCI0/LPCB/EC0/VPC0",
        "IOACPIPlane:/_SB/PC00/LPC0/EC0/VPC0",
        "IOACPIPlane:/_SB/PCI0/LPC0/EC0/VPC0",
        "IOACPIPlane:/_SB/PCI0/SBRG/EC0/VPC0",
        "IOACPIPlane:/_SB/PC00/LPCB/EC/VPC0",
        "IOACPIPlane:/_SB/PCI0/LPCB/EC/VPC0",
        "IOACPIPlane:/_SB/PCI0/LPCB/H_EC/VPC0",
    ]);
    let tzs0_ok = check_any_acpi_node_exists(&[
        "IOACPIPlane:/TZS0",
        "IOACPIPlane:/_TZ/TZS0",
        "IOACPIPlane:/TZ00",
        "IOACPIPlane:/_TZ/TZ00",
    ]);
    let ietm_ok = check_any_acpi_node_exists(&[
        "IOACPIPlane:/_SB/IETM",
        "IOACPIPlane:/_SB/PC00/IETM",
        "IOACPIPlane:/_SB/PCI0/IETM",
    ]);

    println!("========================================================================");
    println!("  ThinkFlow v1.0.0 — Lenovo EC Thermal & Fan Scheduler for macOS");
    println!("========================================================================");
    println!("  CPU Model            : {}", cpu_brand);
    println!(
        "  macOS Low Power Mode : {} (sysctl machdep.xcpm.lpm_enabled = {})",
        if lpm != 0 {
            "ON  [省电模式已开启 -> 联动 EC 静音档]"
        } else {
            "OFF [正常模式 -> 均衡/满载野兽档自动切换]"
        },
        lpm
    );
    println!(
        "  ACPI Hardware Nodes  : VPC0(VPC2004)={} | TZS0/TZS1={} | IETM={}",
        if vpc0_ok { "OK" } else { "Missing" },
        if tzs0_ok { "OK" } else { "Missing" },
        if ietm_ok { "OK" } else { "Missing" }
    );
    println!("------------------------------------------------------------------------");

    if kext.loaded {
        println!("  [Kernel Driver Status: ACTIVE (ThinkFlow.kext loaded)]");
        println!(
            "  Active Policy        : {} ({})",
            kext.policy_name,
            policy_label(kext.policy_mode)
        );
        println!(
            "  Active EC Profile    : {}",
            profile_label(kext.active_profile)
        );
        println!(
            "  CPU Telemetry        : {} °C (TjMax {} °C) | Package Power: {:.2} W",
            kext.cpu_temp_c,
            kext.tjmax_c,
            kext.pkg_power_mw as f64 / 1000.0
        );
        println!(
            "  HWP Request          : Max Ratio = {}x | EPP = 0x{:02X} | Throttling = {}",
            kext.hwp_max_ratio,
            kext.hwp_epp,
            if kext.is_throttling { "YES" } else { "No" }
        );
        println!(
            "  Lenovo EC Registers  : THS0={}°C, THS1={}°C | FANS=0x{:02X} | PFMM={} | ITSM={}",
            kext.ec_temp0_c, kext.ec_temp1_c, kext.ec_fan_raw, kext.ec_pfmm, kext.ec_itsm
        );
        println!(
            "  EC Standalone Flags  : LXOS=0x{:02X} | TPA1={} | SSDT-THINKFLOW={}",
            kext.ec_lxos,
            kext.ec_tpa1,
            if kext.ssdt_present {
                "Loaded"
            } else {
                "Direct EC Mode"
            }
        );
        println!(
            "  Engine Counters      : Total Ticks = {} | Mode Switches = {} | Hysteresis = {} ticks",
            kext.total_ticks, kext.mode_switch_count, kext.hysteresis_counter
        );
    } else {
        println!("  [Kernel Driver Status: STANDBY (ThinkFlow.kext not yet loaded in EFI)]");
        println!("  Sampling live hardware telemetry via libIOReport (500ms window)...");
        if let Some(mut sampler) = PmTelemetrySampler::new() {
            let init_pkg = *sampler
                .prev_map
                .get("MSR_IA32_PKG_ENERGY_STATUS")
                .unwrap_or(&0);
            thread::sleep(Duration::from_millis(500));
            let m = sampler.sample_delta();

            let mut engine = ThinkFlowEngine::new();
            let mut inp = ThinkFlowTelemetryInput::default();
            inp.msr_temp_target = 100 << 16;
            inp.msr_pkg_energy = init_pkg;
            inp.msr_rapl_unit = 14 << 8;
            inp.lpm_sysctl = lpm;
            inp.elapsed_ms = 500;
            let _ = engine.step(&inp);

            inp.msr_pkg_energy = m.raw_pkg_energy;
            let est_temp = if m.therm_limit_delta > 0 {
                98
            } else {
                (42.0 + m.pkg_power_w * 0.95).min(97.0) as u32
            };
            inp.msr_pkg_therm = (1 << 31)
                | ((100u32.saturating_sub(est_temp) as u64) << 16)
                | (if m.therm_limit_delta > 0 { 1 } else { 0 });
            let out = engine.step(&inp);

            println!(
                "  Live CPU Power       : Package = {:.2} W | PP0 Cores = {:.2} W",
                m.pkg_power_w, m.core_power_w
            );
            println!(
                "  Active Core Freq     : P-Core = {:.2} GHz | E-Core = {:.2} GHz | Thermal/PL1 Limit = +{}/+{}",
                m.p_freq_ghz, m.e_freq_ghz, m.therm_limit_delta, m.pl1_limit_delta
            );
            println!(
                "  ThinkFlow Engine     : Target Profile -> {}",
                profile_label(out.target_profile)
            );
            print!("  DYTC Command Queue   : ");
            for i in 0..(out.dytc_cmd_count as usize) {
                print!("0x{:08X} ", out.dytc_cmds[i]);
            }
            println!();
        }
    }
    println!("========================================================================");
}

fn run_monitor(interval_sec: f64, stress_threads: usize, max_seconds: Option<u64>) {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();

    if stress_threads > 0 {
        println!(
            "Starting {} CPU stress threads + ThinkFlow real-time telemetry monitor...",
            stress_threads
        );
        for _ in 0..stress_threads {
            let flag = Arc::clone(&stop_flag);
            workers.push(thread::spawn(move || {
                let mut x: f64 = 1.0000001;
                let mut y: f64 = 1.0000002;
                while !flag.load(Ordering::Relaxed) {
                    for _ in 0..200_000 {
                        x = x.mul_add(1.000000001, 0.000000001);
                        y = y.mul_add(0.999999999, 0.000000002);
                    }
                    std::hint::black_box((x, y));
                }
            }));
        }
    }

    let mut sampler = PmTelemetrySampler::new().expect("Failed to subscribe to pmtelemetry_cpu");
    let mut engine = ThinkFlowEngine::new();

    // Prime baseline energy in engine
    let mut init_inp = ThinkFlowTelemetryInput::default();
    init_inp.msr_pkg_energy = *sampler
        .prev_map
        .get("MSR_IA32_PKG_ENERGY_STATUS")
        .unwrap_or(&0);
    init_inp.msr_rapl_unit = 14 << 8;
    init_inp.elapsed_ms = (interval_sec * 1000.0) as u32;
    let _ = engine.step(&init_inp);

    println!(
        "{:<4} | {:<8} | {:<8} | {:<13} | {:<7} | {:<4} | {:<28} | {:<12}",
        "SEC",
        "PKG(W)",
        "CORE(W)",
        "P / E (GHz)",
        "THERM",
        "LPM",
        "THINKFLOW TARGET PROFILE",
        "HOLD/TEMP"
    );
    println!("{}", "-".repeat(100));

    let start = Instant::now();
    let mut tick: u64 = 0;
    loop {
        thread::sleep(Duration::from_secs_f64(interval_sec));
        tick += 1;

        let m = sampler.sample_delta();
        let lpm = read_sysctl_i32("machdep.xcpm.lpm_enabled").unwrap_or(0);
        let kext = read_kext_telemetry();

        let mut inp = ThinkFlowTelemetryInput::default();
        inp.msr_temp_target = 100 << 16;
        let est_temp = if kext.loaded && kext.cpu_temp_c > 0 {
            kext.cpu_temp_c
        } else if m.therm_limit_delta > 0 {
            98
        } else {
            (42.0 + m.pkg_power_w * 0.95).min(97.0) as u32
        };
        inp.msr_pkg_therm = (1 << 31)
            | ((100u32.saturating_sub(est_temp) as u64) << 16)
            | (if m.therm_limit_delta > 0 { 1 } else { 0 });
        inp.msr_pkg_energy = m.raw_pkg_energy;
        inp.msr_rapl_unit = 14 << 8;
        inp.lpm_sysctl = lpm;
        inp.elapsed_ms = (interval_sec * 1000.0) as u32;

        let out = engine.step(&inp);

        let profile_short = match if kext.loaded {
            kext.active_profile
        } else {
            out.target_profile
        } {
            1 => "Quiet (PFMM=0,ITSM=1)",
            2 => "Balance (PFMM=2,ITSM=2)",
            3 => "Performance (PFMM=1,ITSM=4)",
            _ => "Unknown",
        };

        let status_str = if kext.loaded {
            format!("0x{:02X} / {}°C", kext.ec_fan_raw, kext.ec_temp0_c)
        } else {
            format!("hold={} ~{}°C", out.hysteresis_counter, est_temp)
        };

        println!(
            "{:<4} | {:>6.2} W | {:>6.2} W | {:>4.2}/{:>4.2} GHz | {:>+7} | {:<4} | {:<28} | {:<12}",
            tick,
            m.pkg_power_w,
            m.core_power_w,
            m.p_freq_ghz,
            m.e_freq_ghz,
            m.therm_limit_delta,
            if lpm != 0 { "ON" } else { "OFF" },
            profile_short,
            status_str
        );

        if let Some(max_s) = max_seconds {
            if start.elapsed().as_secs() >= max_s {
                break;
            }
        }
    }

    stop_flag.store(true, Ordering::Relaxed);
    for w in workers {
        let _ = w.join();
    }
}

fn handle_mode_command(mode_arg: &str) {
    let (policy, profile, cmds) = match mode_arg.to_ascii_lowercase().as_str() {
        "auto" => (
            ThinkFlowPolicy::Auto,
            ThinkFlowProfile::Balance,
            [DYTC_SET_APM_OFF, DYTC_SET_AQM_OFF, DYTC_SET_MMC_AUTO],
        ),
        "quiet" | "cool" | "lpm" => (
            ThinkFlowPolicy::Quiet,
            ThinkFlowProfile::Quiet,
            [DYTC_SET_APM_OFF, DYTC_SET_MMC_QUIET, DYTC_SET_AQM_ON],
        ),
        "balance" | "normal" => (
            ThinkFlowPolicy::Balance,
            ThinkFlowProfile::Balance,
            [DYTC_SET_APM_OFF, DYTC_SET_AQM_OFF, DYTC_SET_MMC_AUTO],
        ),
        "performance" | "perf" | "beast" | "turbo" => (
            ThinkFlowPolicy::Performance,
            ThinkFlowProfile::Performance,
            [DYTC_SET_AQM_OFF, DYTC_SET_MMC_PERF, DYTC_SET_APM_ON],
        ),
        _ => {
            eprintln!(
                "Invalid mode '{}'. Expected one of: auto | quiet | balance | performance",
                mode_arg
            );
            std::process::exit(1);
        }
    };

    println!("Target Policy      : {}", policy_label(policy as u32));
    println!("Target EC Profile  : {}", profile_label(profile as u32));
    println!(
        "VPC0.DYTC Sequence : 0x{:08X} -> 0x{:08X} -> 0x{:08X}",
        cmds[0], cmds[1], cmds[2]
    );

    match set_kext_policy(policy) {
        Ok(()) => {
            println!(
                "Successfully updated ThinkFlow.kext kernel policy to {:?}!",
                policy
            );
        }
        Err(msg) => {
            println!("Note: {}", msg);
        }
    }
}

fn print_help() {
    println!("ThinkFlow v1.0.0 — Lenovo EC Thermal & Fan Control CLI");
    println!();
    println!("USAGE:");
    println!("  thinkflow status                        Show current thermal, LPM & EC status");
    println!("  thinkflow monitor [interval_sec]        Stream real-time power, freq & EC profile");
    println!("  thinkflow mode <auto|quiet|balance|perf> Switch ThinkFlow thermal/fan policy");
    println!("  thinkflow bench [duration_sec]          Run multi-core stress test with telemetry");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    match cmd {
        "status" | "info" => print_status(),
        "monitor" | "watch" => {
            let interval = args
                .get(2)
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(1.0);
            let max_s = args.get(3).and_then(|s| s.parse::<u64>().ok());
            run_monitor(interval, 0, max_s);
        }
        "mode" | "set" => {
            if let Some(m) = args.get(2) {
                handle_mode_command(m);
            } else {
                eprintln!("Usage: thinkflow mode <auto|quiet|balance|performance>");
                std::process::exit(1);
            }
        }
        "bench" | "stress" => {
            let secs = args.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(6);
            let threads = thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(20);
            run_monitor(1.0, threads, Some(secs));
        }
        "-h" | "--help" | "help" => print_help(),
        _ => {
            print_help();
            std::process::exit(1);
        }
    }
}
