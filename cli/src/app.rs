use std::collections::VecDeque;
use std::time::Instant;

use thinkflow_core::{ThinkFlowEngine, ThinkFlowPolicy, ThinkFlowTelemetryInput};

use crate::cpu::{
    detect_cpu_topology, CoreKind, CoreLiveMetric, CpuFreqMetrics, CpuLoadSampler,
    PhysicalCoreSpec, PmTelemetrySampler,
};
use crate::ffi::{check_acpi_node_exists, read_sysctl_i32, read_sysctl_string};
use crate::telemetry::{read_kext_telemetry, set_kext_policy, LiveTelemetry};

// ============================================================================
// Interactive Application State & Telemetry Aggregation
// ============================================================================

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoreViewMode {
    Auto,
    Expanded,
    TrendOnly,
}

pub(crate) struct AppState {
    pub hostname: String,
    pub cpu_brand: String,
    pub cpu_topo_summary: String,
    pub core_specs: Vec<PhysicalCoreSpec>,
    pub core_metrics: Vec<CoreLiveMetric>,
    pub core_view_mode: CoreViewMode,
    pub telemetry: LiveTelemetry,
    pub temp_history: VecDeque<u64>,
    pub status_message: String,
    pub status_time: Instant,
    pub engine_fallback: ThinkFlowEngine,
    pub sampler: Option<PmTelemetrySampler>,
    pub load_sampler: CpuLoadSampler,
}

impl AppState {
    pub fn new() -> Self {
        let hostname = read_sysctl_string("kern.hostname")
            .unwrap_or_else(|| "MacBook-Pro".into())
            .trim_end_matches(".local")
            .to_string();
        let cpu_brand = read_sysctl_string("machdep.cpu.brand_string")
            .unwrap_or_else(|| "13th Gen Intel Core i7-13700H".into());
        let logical_cpus = read_sysctl_i32("hw.logicalcpu").unwrap_or(20).max(1) as usize;
        let core_specs = detect_cpu_topology(logical_cpus);

        let p_cores = core_specs
            .iter()
            .filter(|c| c.kind == CoreKind::Performance)
            .count();
        let e_cores = core_specs
            .iter()
            .filter(|c| c.kind == CoreKind::Efficiency)
            .count();
        let short_model = cpu_brand
            .split_whitespace()
            .find(|tok| {
                tok.starts_with("i3-")
                    || tok.starts_with("i5-")
                    || tok.starts_with("i7-")
                    || tok.starts_with("i9-")
                    || tok.starts_with("M1")
                    || tok.starts_with("M2")
                    || tok.starts_with("M3")
                    || tok.starts_with("M4")
            })
            .unwrap_or(&cpu_brand);
        let cpu_topo_summary = if e_cores > 0 {
            format!(
                "{} ({}P+{}E / {}T)",
                short_model, p_cores, e_cores, logical_cpus
            )
        } else {
            format!("{} ({}C/{}T)", short_model, p_cores, logical_cpus)
        };

        let sampler = PmTelemetrySampler::new();
        let load_sampler = CpuLoadSampler::new();

        let mut app = Self {
            hostname,
            cpu_brand,
            cpu_topo_summary,
            core_specs,
            core_metrics: Vec::new(),
            core_view_mode: CoreViewMode::Auto,
            telemetry: LiveTelemetry::default(),
            temp_history: VecDeque::with_capacity(60),
            status_message: "NOMINAL".into(),
            status_time: Instant::now(),
            engine_fallback: ThinkFlowEngine::new(),
            sampler,
            load_sampler,
        };
        app.update_telemetry();
        app
    }

    pub fn set_status(&mut self, msg: &str) {
        self.status_message = msg.to_string();
        self.status_time = Instant::now();
    }

    pub fn cycle_core_view(&mut self) {
        self.core_view_mode = match self.core_view_mode {
            CoreViewMode::Auto => CoreViewMode::Expanded,
            CoreViewMode::Expanded => CoreViewMode::TrendOnly,
            CoreViewMode::TrendOnly => CoreViewMode::Auto,
        };
        let label = match self.core_view_mode {
            CoreViewMode::Auto => "View: Auto (P/E Cores + Trend)",
            CoreViewMode::Expanded => "View: Expanded Per-Core Detail",
            CoreViewMode::TrendOnly => "View: Classic Temperature Trend",
        };
        self.set_status(label);
    }

    pub fn update_telemetry(&mut self) {
        let freq_m = if let Some(ref mut s) = self.sampler {
            s.sample_delta(&self.core_specs)
        } else {
            CpuFreqMetrics::default()
        };
        let thread_usages = self.load_sampler.sample_thread_usages();

        let mut metrics = Vec::with_capacity(self.core_specs.len());
        for (idx, spec) in self.core_specs.iter().enumerate() {
            let mut t_usages = Vec::with_capacity(spec.threads.len());
            let mut sum = 0.0;
            for &tid in &spec.threads {
                let u = thread_usages.get(tid).copied().unwrap_or(0.0);
                t_usages.push(u);
                sum += u;
            }
            let usage_pct = if !t_usages.is_empty() {
                sum / (t_usages.len() as f64)
            } else {
                0.0
            };
            let freq_ghz = freq_m.core_freqs_ghz.get(idx).copied().unwrap_or(0.0);
            metrics.push(CoreLiveMetric {
                kind: spec.kind,
                label: spec.label.clone(),
                threads: spec.threads.clone(),
                usage_pct,
                thread_usages: t_usages,
                freq_ghz,
            });
        }
        self.core_metrics = metrics;

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
                target_pl1_w: out.target_pl1_mw as f64 / 1000.0,
                effective_pl1_w: out.target_pl1_mw as f64 / 1000.0,
                core_power_w: freq_m.core_power_w,
                p_freq_ghz: freq_m.p_freq_ghz,
                e_freq_ghz: freq_m.e_freq_ghz,
                ec_fan_target: out.ec_fan_target as u32,
                ec_temp0_c: est_temp.saturating_sub(2),
                ec_temp1_c: est_temp.saturating_sub(8),
                ec_fan_raw: if out.ec_fan_target == 3 {
                    79
                } else if out.ec_fan_target == 2 {
                    45
                } else {
                    0
                },
                ec_fan_rpm: if out.ec_fan_target == 3 {
                    3800
                } else if out.ec_fan_target == 2 {
                    2200
                } else {
                    0
                },
                ec_pfmm: if out.ec_fan_target == 3 {
                    1
                } else if out.ec_fan_target == 2 {
                    2
                } else {
                    0
                },
                ec_itsm: if out.ec_fan_target == 3 {
                    4
                } else if out.ec_fan_target == 2 {
                    2
                } else {
                    1
                },
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
