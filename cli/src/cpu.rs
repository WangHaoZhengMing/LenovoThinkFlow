use std::collections::HashMap;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;
use std::time::Instant;

use crate::ffi::{
    cfstr, cfstr_to_string, host_processor_info, mach_host_self, mach_port_deallocate,
    mach_task_self_, read_sysctl_i32, read_sysctl_string, read_sysctl_u64, vm_deallocate,
    CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef, CFDataGetBytePtr, CFDataGetLength,
    CFDataRef, CFDictionaryGetValue, CFDictionaryRef, CFMutableDictionaryRef, CFRelease,
    CFStringRef, CFTypeRef, IOIteratorNext, IOObjectRelease, IORegistryEntryFromPath,
    IORegistryEntryGetChildIterator, IORegistryEntryGetLocationInPlane,
    IOReportCopyChannelsInGroup, IOReportCreateSamples, IOReportCreateSubscription,
    IOReportSubscriptionRef, IoObjectT, MachMsgTypeNumberT, NaturalT, ProcessorInfoArrayT,
    CPU_STATE_IDLE, CPU_STATE_MAX, CPU_STATE_NICE, CPU_STATE_SYSTEM, CPU_STATE_USER,
    PROCESSOR_CPU_LOAD_INFO,
};

// ============================================================================
// Hybrid CPU Topology (P-Core / E-Core), Mach Load & `pmtelemetry_cpu` Samplers
// ============================================================================

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoreKind {
    Performance, // P-Core (性能核)
    Efficiency,  // E-Core (能效核)
}

#[derive(Clone, Debug)]
pub struct PhysicalCoreSpec {
    pub kind: CoreKind,
    pub label: String,       // e.g. "P0".."P5", "E0".."E7"
    pub threads: Vec<usize>, // Logical CPU thread indices (e.g. [0, 1] for P0, [12] for E0)
}

#[derive(Clone, Debug)]
pub struct CoreLiveMetric {
    pub kind: CoreKind,
    pub label: String,
    pub threads: Vec<usize>,
    pub usage_pct: f64,
    pub thread_usages: Vec<f64>,
    pub freq_ghz: f64,
}

fn read_cpus_apic_ids() -> Vec<u32> {
    let mut apics = Vec::new();
    let path = CString::new("IODeviceTree:/cpus").unwrap();
    let plane = CString::new("IODeviceTree").unwrap();
    let cpus_entry = unsafe { IORegistryEntryFromPath(0, path.as_ptr()) };
    if cpus_entry == 0 {
        return apics;
    }

    let mut iter: IoObjectT = 0;
    let kr = unsafe { IORegistryEntryGetChildIterator(cpus_entry, plane.as_ptr(), &mut iter) };
    if kr == 0 && iter != 0 {
        loop {
            let child = unsafe { IOIteratorNext(iter) };
            if child == 0 {
                break;
            }
            let mut loc_buf = [0 as c_char; 128];
            let lkr = unsafe {
                IORegistryEntryGetLocationInPlane(child, plane.as_ptr(), loc_buf.as_mut_ptr())
            };
            if lkr == 0 && loc_buf[0] != 0 {
                let loc_str = unsafe { CStr::from_ptr(loc_buf.as_ptr()) }.to_string_lossy();
                if let Ok(apic_id) = u32::from_str_radix(loc_str.trim(), 16) {
                    apics.push(apic_id);
                }
            }
            unsafe { IOObjectRelease(child) };
        }
        unsafe { IOObjectRelease(iter) };
    }
    unsafe { IOObjectRelease(cpus_entry) };
    apics
}

pub(crate) fn detect_cpu_topology(logical_count: usize) -> Vec<PhysicalCoreSpec> {
    let apics = read_cpus_apic_ids();
    if apics.len() == logical_count && logical_count > 0 {
        let mut specs = Vec::new();
        let mut p_idx = 0usize;
        let mut e_idx = 0usize;
        let mut i = 0usize;
        let mut has_paired = false;
        let mut has_single = false;

        // Pass 1: check whether mixed SMT-paired (P-Core) & single (E-Core) APIC IDs exist
        while i < apics.len() {
            let a = apics[i];
            if (a & 1) == 0 && i + 1 < apics.len() && apics[i + 1] == a + 1 {
                has_paired = true;
                i += 2;
            } else {
                has_single = true;
                i += 1;
            }
        }

        if has_paired && has_single {
            // Intel Hybrid (12th/13th/14th Gen Alder/Raptor Lake):
            // Consecutive (even, even+1) APIC pairs are SMT P-Cores; standalone APICs are E-Cores
            let mut idx = 0usize;
            while idx < apics.len() {
                let a = apics[idx];
                if (a & 1) == 0 && idx + 1 < apics.len() && apics[idx + 1] == a + 1 {
                    specs.push(PhysicalCoreSpec {
                        kind: CoreKind::Performance,
                        label: format!("P{}", p_idx),
                        threads: vec![idx, idx + 1],
                    });
                    p_idx += 1;
                    idx += 2;
                } else {
                    specs.push(PhysicalCoreSpec {
                        kind: CoreKind::Efficiency,
                        label: format!("E{}", e_idx),
                        threads: vec![idx],
                    });
                    e_idx += 1;
                    idx += 1;
                }
            }
            return specs;
        } else if has_paired && !has_single {
            // Uniform SMT P-Core CPU (e.g. 6C/12T or 8C/16T non-hybrid Intel)
            for (k, pair) in (0..logical_count).collect::<Vec<_>>().chunks(2).enumerate() {
                specs.push(PhysicalCoreSpec {
                    kind: CoreKind::Performance,
                    label: format!("P{}", k),
                    threads: pair.to_vec(),
                });
            }
            return specs;
        }
    }

    // Apple Silicon / perflevel sysctl fallback
    let pl0_cpus = read_sysctl_i32("hw.perflevel0.logicalcpu").unwrap_or(0) as usize;
    let pl1_cpus = read_sysctl_i32("hw.perflevel1.logicalcpu").unwrap_or(0) as usize;
    if pl0_cpus > 0 && pl1_cpus > 0 && pl0_cpus + pl1_cpus == logical_count {
        let pl0_name = read_sysctl_string("hw.perflevel0.name").unwrap_or_default();
        let pl0_is_perf = pl0_name.to_ascii_lowercase().contains("perf");
        let mut specs = Vec::with_capacity(logical_count);
        let mut p_idx = 0usize;
        let mut e_idx = 0usize;
        for t in 0..logical_count {
            let is_p = if t < pl0_cpus { pl0_is_perf } else { !pl0_is_perf };
            if is_p {
                specs.push(PhysicalCoreSpec {
                    kind: CoreKind::Performance,
                    label: format!("P{}", p_idx),
                    threads: vec![t],
                });
                p_idx += 1;
            } else {
                specs.push(PhysicalCoreSpec {
                    kind: CoreKind::Efficiency,
                    label: format!("E{}", e_idx),
                    threads: vec![t],
                });
                e_idx += 1;
            }
        }
        return specs;
    }

    // Generic fallback
    let phys = read_sysctl_i32("hw.physicalcpu")
        .unwrap_or(logical_count as i32)
        .max(1) as usize;
    let mut specs = Vec::new();
    if logical_count == phys * 2 {
        for k in 0..phys {
            specs.push(PhysicalCoreSpec {
                kind: CoreKind::Performance,
                label: format!("P{}", k),
                threads: vec![k * 2, k * 2 + 1],
            });
        }
    } else {
        for t in 0..logical_count {
            specs.push(PhysicalCoreSpec {
                kind: CoreKind::Performance,
                label: format!("P{}", t),
                threads: vec![t],
            });
        }
    }
    specs
}

pub(crate) struct CpuLoadSampler {
    prev_ticks: Vec<[u64; CPU_STATE_MAX]>,
}

impl CpuLoadSampler {
    pub(crate) fn new() -> Self {
        let prev_ticks = Self::read_raw_ticks().unwrap_or_default();
        Self { prev_ticks }
    }

    fn read_raw_ticks() -> Option<Vec<[u64; CPU_STATE_MAX]>> {
        let host = unsafe { mach_host_self() };
        if host == 0 {
            return None;
        }
        let mut cpu_count: NaturalT = 0;
        let mut info_ptr: ProcessorInfoArrayT = ptr::null_mut();
        let mut info_cnt: MachMsgTypeNumberT = 0;

        let kr = unsafe {
            host_processor_info(
                host,
                PROCESSOR_CPU_LOAD_INFO,
                &mut cpu_count,
                &mut info_ptr,
                &mut info_cnt,
            )
        };
        unsafe {
            let _ = mach_port_deallocate(mach_task_self_, host);
        }

        if kr != 0 || info_ptr.is_null() || cpu_count == 0 {
            return None;
        }

        let n = cpu_count as usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let base = i * CPU_STATE_MAX;
            let ticks = unsafe {
                [
                    *info_ptr.add(base + CPU_STATE_USER) as u32 as u64,
                    *info_ptr.add(base + CPU_STATE_SYSTEM) as u32 as u64,
                    *info_ptr.add(base + CPU_STATE_IDLE) as u32 as u64,
                    *info_ptr.add(base + CPU_STATE_NICE) as u32 as u64,
                ]
            };
            out.push(ticks);
        }

        unsafe {
            let _ = vm_deallocate(
                mach_task_self_,
                info_ptr as usize,
                (info_cnt as usize) * std::mem::size_of::<c_int>(),
            );
        }
        Some(out)
    }

    pub(crate) fn sample_thread_usages(&mut self) -> Vec<f64> {
        let Some(curr) = Self::read_raw_ticks() else {
            return Vec::new();
        };
        if self.prev_ticks.len() != curr.len() {
            self.prev_ticks = curr.clone();
            return vec![0.0; curr.len()];
        }

        let mut usages = Vec::with_capacity(curr.len());
        for (p, c) in self.prev_ticks.iter().zip(curr.iter()) {
            let du = c[CPU_STATE_USER].wrapping_sub(p[CPU_STATE_USER]);
            let ds = c[CPU_STATE_SYSTEM].wrapping_sub(p[CPU_STATE_SYSTEM]);
            let di = c[CPU_STATE_IDLE].wrapping_sub(p[CPU_STATE_IDLE]);
            let dn = c[CPU_STATE_NICE].wrapping_sub(p[CPU_STATE_NICE]);
            let active = du + ds + dn;
            let total = active + di;
            let pct = if total > 0 {
                ((active as f64 / total as f64) * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };
            usages.push(pct);
        }
        self.prev_ticks = curr;
        usages
    }
}

pub(crate) struct PmTelemetrySampler {
    sub: IOReportSubscriptionRef,
    subbed: CFMutableDictionaryRef,
    prev_map: HashMap<String, u64>,
    prev_time: Instant,
    tsc_ghz: f64,
}

#[derive(Debug, Default, Clone)]
pub struct CpuFreqMetrics {
    pub pkg_power_w: f64,
    pub core_power_w: f64,
    pub p_freq_ghz: f64,
    pub e_freq_ghz: f64,
    pub core_freqs_ghz: Vec<f64>,
    pub therm_limit_delta: u64,
    pub raw_pkg_energy: u64,
}

impl PmTelemetrySampler {
    pub(crate) fn new() -> Option<Self> {
        let grp = cfstr("pmtelemetry_cpu");
        let ch = unsafe { IOReportCopyChannelsInGroup(grp, ptr::null(), 0, 0, 0) };
        unsafe { CFRelease(grp) };
        if ch.is_null() {
            return None;
        }
        let mut subbed: CFMutableDictionaryRef = ptr::null_mut();
        let sub = unsafe {
            IOReportCreateSubscription(ptr::null(), ch, &mut subbed, 0, ptr::null())
        };
        unsafe { CFRelease(ch as CFTypeRef) };
        if sub.is_null() || subbed.is_null() {
            return None;
        }
        let tsc_hz = read_sysctl_u64("machdep.tsc.frequency").unwrap_or(2_918_400_000);
        let tsc_ghz = if tsc_hz > 100_000_000 {
            tsc_hz as f64 / 1_000_000_000.0
        } else {
            2.9184
        };
        let prev_map = Self::take_snapshot(sub, subbed);
        Some(Self {
            sub,
            subbed,
            prev_map,
            prev_time: Instant::now(),
            tsc_ghz,
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

    pub(crate) fn sample_delta(&mut self, core_specs: &[PhysicalCoreSpec]) -> CpuFreqMetrics {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_time).as_secs_f64().max(0.05);
        let curr_map = Self::take_snapshot(self.sub, self.subbed);

        let pkg0 = *self
            .prev_map
            .get("MSR_IA32_PKG_ENERGY_STATUS")
            .unwrap_or(&0) as u32;
        let pkg1 = *curr_map
            .get("MSR_IA32_PKG_ENERGY_STATUS")
            .unwrap_or(&0) as u32;
        let pp0_0 = *self
            .prev_map
            .get("MSR_IA32_PP0_ENERGY_STATUS")
            .unwrap_or(&0) as u32;
        let pp0_1 = *curr_map
            .get("MSR_IA32_PP0_ENERGY_STATUS")
            .unwrap_or(&0) as u32;

        let pkg_j = pkg1.wrapping_sub(pkg0) as f64 / 16384.0;
        let pp0_j = pp0_1.wrapping_sub(pp0_0) as f64 / 16384.0;

        let mut p_sum = 0.0;
        let mut p_cnt = 0usize;
        let mut e_sum = 0.0;
        let mut e_cnt = 0usize;
        let mut core_freqs_ghz = Vec::with_capacity(core_specs.len());

        for spec in core_specs {
            let mut da_total = 0u64;
            let mut dm_total = 0u64;
            for &tid in &spec.threads {
                let ka = format!("MSR_IA32_ACNT {}", tid);
                let km = format!("MSR_IA32_MCNT {}", tid);
                let da = curr_map
                    .get(&ka)
                    .unwrap_or(&0)
                    .wrapping_sub(*self.prev_map.get(&ka).unwrap_or(&0));
                let dm = curr_map
                    .get(&km)
                    .unwrap_or(&0)
                    .wrapping_sub(*self.prev_map.get(&km).unwrap_or(&0));
                da_total = da_total.saturating_add(da);
                dm_total = dm_total.saturating_add(dm);
            }
            let freq = if dm_total > 0 {
                (self.tsc_ghz * (da_total as f64) / (dm_total as f64)).clamp(0.0, 6.5)
            } else {
                0.0
            };
            core_freqs_ghz.push(freq);
            if freq > 0.05 {
                match spec.kind {
                    CoreKind::Performance => {
                        p_sum += freq;
                        p_cnt += 1;
                    }
                    CoreKind::Efficiency => {
                        e_sum += freq;
                        e_cnt += 1;
                    }
                }
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
            core_freqs_ghz,
            therm_limit_delta: therm_delta,
            raw_pkg_energy: pkg1 as u64,
        }
    }
}
