#![no_std]

#[cfg(test)]
extern crate std;

// ============================================================================
// Lenovo VPC0.DYTC Hardware Command Constants (Verified from DSDT @ 0x9df70)
// ============================================================================
//
// DYTC command bitfield layout (Arg0):
//   Bits [8:0]   : Command (0x001 = CMD_SET, 0x002 = CMD_GET, 0x007 = CMD_MMC_GET)
//   Bits [15:12] : ICFunc  (0xB = MMC, 0x5 = APM, 0x6 = AQM, 0x4 = STP)
//   Bits [19:16] : ICMode  (0x2 = Performance, 0x3 = Quiet, 0xF = Auto/All)
//   Bit  [20]    : ValidF  (0 = Disable/Reset function, 1 = Enable function)

/// Query DYTC global status (`DYTC_CMD_GET`)
pub const DYTC_CMD_GET: u32 = 0x0000_0002;
/// Query current MMC mode (`DYTC_CMD_MMC_GET`)
pub const DYTC_CMD_MMC_GET: u32 = 0x0000_0007;

/// Clear APM (Active Performance Mode): cmd=1, ICFunc=0x5, ICMode=0xF, ValidF=0
pub const DYTC_SET_APM_OFF: u32 = 0x000F_5001;
/// Enable APM (Active Performance Mode): cmd=1, ICFunc=0x5, ICMode=0xF, ValidF=1 -> ITSM=4
pub const DYTC_SET_APM_ON: u32 = 0x001F_5001;

/// Clear AQM (Active Quiet Mode): cmd=1, ICFunc=0x6, ICMode=0xF, ValidF=0
pub const DYTC_SET_AQM_OFF: u32 = 0x000F_6001;
/// Enable AQM (Active Quiet Mode): cmd=1, ICFunc=0x6, ICMode=0xF, ValidF=1 -> ITSM=1
pub const DYTC_SET_AQM_ON: u32 = 0x001F_6001;

/// Set MMC Auto / Balance: cmd=1, ICFunc=0xB, ICMode=0xF, ValidF=0 -> PFMM=2, ITSM=2
pub const DYTC_SET_MMC_AUTO: u32 = 0x000F_B001;
/// Set MMC Performance: cmd=1, ICFunc=0xB, ICMode=0x2, ValidF=1 -> PFMM=1
pub const DYTC_SET_MMC_PERF: u32 = 0x0012_B001;
/// Set MMC Quiet: cmd=1, ICFunc=0xB, ICMode=0x3, ValidF=1 -> PFMM=0
pub const DYTC_SET_MMC_QUIET: u32 = 0x0013_B001;

/// EC0 `LXOS` (offset 0x58) value when `_OSI("Linux")` is true: enables standalone EC thermal loop
pub const EC_LXOS_STANDALONE_VALUE: u8 = 0x40;
/// EC0 `TPA1` (offset 0xCF) thermal participant active value
pub const EC_TPA1_ACTIVE_VALUE: u8 = 0x01;

// ============================================================================
// Policy & Profile Enums (C-ABI compatible)
// ============================================================================

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ThinkFlowPolicy {
    /// Follow macOS Low Power Mode + automatic CPU load/temperature escalation
    Auto = 0,
    /// Force Lenovo EC Quiet / Cool mode (`PFMM=0, ITSM=1`)
    Quiet = 1,
    /// Force Lenovo EC Balance / Auto mode (`PFMM=2, ITSM=2`)
    Balance = 2,
    /// Force Lenovo EC Performance / Beast mode (`PFMM=1, ITSM=4`)
    Performance = 3,
}

impl ThinkFlowPolicy {
    pub const fn from_u32(val: u32) -> Self {
        match val {
            1 => Self::Quiet,
            2 => Self::Balance,
            3 => Self::Performance,
            _ => Self::Auto,
        }
    }
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ThinkFlowProfile {
    Unknown = 0,
    /// Quiet / Cool mode (`PFMM=0, ITSM=1`)
    Quiet = 1,
    /// Balance / Auto mode (`PFMM=2, ITSM=2`)
    Balance = 2,
    /// Performance / Beast mode (`PFMM=1, ITSM=4`)
    Performance = 3,
}

// ============================================================================
// Telemetry Input & Action Output Structs (C-ABI compatible)
// ============================================================================

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct ThinkFlowTelemetryInput {
    /// `MSR_TEMPERATURE_TARGET` (0x1A2)
    pub msr_temp_target: u64,
    /// `MSR_IA32_PACKAGE_THERM_STATUS` (0x1B1)
    pub msr_pkg_therm: u64,
    /// `MSR_IA32_THERM_STATUS` (0x19C)
    pub msr_core_therm: u64,
    /// `MSR_IA32_HWP_REQUEST` (0x774)
    pub msr_hwp_req: u64,
    /// `MSR_PKG_ENERGY_STATUS` (0x611)
    pub msr_pkg_energy: u64,
    /// `MSR_RAPL_POWER_UNIT` (0x606)
    pub msr_rapl_unit: u64,
    /// `sysctl machdep.xcpm.lpm_enabled` (0 = Off, 1 = On, -1 = Unavailable)
    pub lpm_sysctl: i32,
    /// `ECMB.THS0` (0xFE802020) thermal sensor 0 in °C
    pub ec_temp0_c: u8,
    /// `ECMB.THS1` (0xFE802021) thermal sensor 1 in °C
    pub ec_temp1_c: u8,
    /// `EC0.FANS` (0x95) raw fan speed/level register
    pub ec_fan_raw: u8,
    /// `EC0.PFMM` (0xC5 bits [2:1]): 0=Quiet, 1=Performance, 2=Auto
    pub ec_pfmm: u8,
    /// `EC0.ITSM` (0xC6 bits [5:3]): 1=Quiet, 2=Balance, 4=Performance
    pub ec_itsm: u8,
    /// `EC0.LXOS` (0x58): 0x40 = standalone EC thermal mode enabled
    pub ec_lxos: u8,
    /// `EC0.TPA1` (0xCF): 1 = thermal participant active
    pub ec_tpa1: u8,
    /// Reserved alignment byte
    pub _pad0: u8,
    /// Elapsed milliseconds since last poll tick (typically 2000 ms)
    pub elapsed_ms: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct ThinkFlowActionOutput {
    /// Selected target `ThinkFlowProfile` (1=Quiet, 2=Balance, 3=Performance)
    pub target_profile: u32,
    /// 1 if the kext should evaluate `dytc_cmds[0..dytc_cmd_count]` on `VPC0.DYTC`
    pub apply_dytc: u8,
    /// 1 if the kext should write `LXOS=0x40` and `TPA1=1` to `EC0`
    pub fix_ec_init: u8,
    /// 1 if the kext should evaluate `TZS0._TMP` and `TZS1._TMP` to keep EC sensors fresh
    pub poll_thermal_zones: u8,
    /// Number of valid commands in `dytc_cmds` (0..=4)
    pub dytc_cmd_count: u8,
    /// Sequence of 32-bit command integers to pass to `VPC0.DYTC`
    pub dytc_cmds: [u32; 4],
    /// Decoded CPU TjMax in °C (typically 100°C for i7-13700H)
    pub computed_tjmax_c: u32,
    /// Decoded CPU package/core temperature in °C
    pub computed_cpu_temp_c: u32,
    /// Computed CPU package power in milliwatts (e.g. 47200 = 47.2 W)
    pub computed_pkg_power_mw: u32,
    /// Current HWP `Maximum_Performance` ratio from `MSR_IA32_HWP_REQUEST` bits [15:8]
    pub computed_hwp_max_ratio: u32,
    /// Current HWP `Energy_Performance_Preference` (EPP) from `MSR_IA32_HWP_REQUEST` bits [31:24]
    pub computed_hwp_epp: u32,
    /// 1 if macOS Low Power Mode is detected as active
    pub is_lpm_active: u8,
    /// 1 if CPU thermal throttling bit (`MSR_IA32_PACKAGE_THERM_STATUS[0]`) is active
    pub is_throttling: u8,
    /// Reserved alignment bytes
    pub _pad1: [u8; 2],
    /// Remaining hysteresis ticks before de-escalating from Performance -> Balance
    pub hysteresis_counter: u32,
}

// ============================================================================
// ThinkFlow Core State Machine
// ============================================================================

/// Thresholds for Auto policy escalation/de-escalation
pub const AUTO_ESCALATE_TEMP_C: u32 = 72;
pub const AUTO_DEESCALATE_TEMP_C: u32 = 64;
pub const AUTO_ESCALATE_POWER_MW: u32 = 26_000; // 26.0 W
pub const AUTO_DEESCALATE_POWER_MW: u32 = 18_000; // 18.0 W
pub const AUTO_HOLD_TICKS_INITIAL: u32 = 6; // 12 seconds at 2s interval
pub const AUTO_HOLD_TICKS_SUSTAIN: u32 = 4; // 8 seconds sustain
pub const PERIODIC_DYTC_REFRESH_TICKS: u32 = 30; // Re-verify DYTC every 60s

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ThinkFlowEngine {
    pub policy: ThinkFlowPolicy,
    pub current_profile: ThinkFlowProfile,
    pub prev_energy_raw: u32,
    pub has_prev_energy: u8,
    pub perf_hold_ticks: u32,
    pub ticks_since_dytc_sync: u32,
    pub total_ticks: u64,
    pub mode_switch_count: u32,
}

impl Default for ThinkFlowEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ThinkFlowEngine {
    pub const fn new() -> Self {
        Self {
            policy: ThinkFlowPolicy::Auto,
            current_profile: ThinkFlowProfile::Unknown,
            prev_energy_raw: 0,
            has_prev_energy: 0,
            perf_hold_ticks: 0,
            ticks_since_dytc_sync: 0,
            total_ticks: 0,
            mode_switch_count: 0,
        }
    }

    pub fn reset_after_wake(&mut self) {
        self.current_profile = ThinkFlowProfile::Unknown;
        self.has_prev_energy = 0;
        self.ticks_since_dytc_sync = PERIODIC_DYTC_REFRESH_TICKS;
    }

    pub fn set_policy(&mut self, policy: ThinkFlowPolicy) {
        if self.policy != policy {
            self.policy = policy;
            self.perf_hold_ticks = 0;
            // Force immediate re-evaluation and DYTC sync on next tick
            self.current_profile = ThinkFlowProfile::Unknown;
        }
    }

    /// Decode `TjMax` and digital thermal sensor readout from Intel MSRs
    fn decode_temperature(input: &ThinkFlowTelemetryInput) -> (u32, u32, bool) {
        let raw_tjmax = ((input.msr_temp_target >> 16) & 0xFF) as u32;
        let tjmax = if raw_tjmax >= 70 && raw_tjmax <= 115 {
            raw_tjmax
        } else {
            100
        };

        let pkg_valid = (input.msr_pkg_therm & (1 << 31)) != 0;
        let core_valid = (input.msr_core_therm & (1 << 31)) != 0;

        let pkg_temp = if pkg_valid {
            let offset = ((input.msr_pkg_therm >> 16) & 0x7F) as u32;
            tjmax.saturating_sub(offset)
        } else {
            0
        };

        let core_temp = if core_valid {
            let offset = ((input.msr_core_therm >> 16) & 0x7F) as u32;
            tjmax.saturating_sub(offset)
        } else {
            0
        };

        let msr_temp = if pkg_temp >= core_temp {
            pkg_temp
        } else {
            core_temp
        };

        let cpu_temp = if msr_temp > 0 {
            msr_temp
        } else if input.ec_temp0_c > 0 && input.ec_temp0_c < 120 {
            input.ec_temp0_c as u32
        } else {
            45
        };

        let is_throttling = ((input.msr_pkg_therm & 0x1) != 0) || ((input.msr_core_therm & 0x1) != 0);
        (tjmax, cpu_temp, is_throttling)
    }

    /// Compute CPU package power in milliwatts using pure 64-bit integer arithmetic
    fn compute_package_power_mw(&mut self, input: &ThinkFlowTelemetryInput) -> u32 {
        let raw_energy = (input.msr_pkg_energy & 0xFFFF_FFFF) as u32;
        if self.has_prev_energy == 0 || input.elapsed_ms == 0 {
            self.prev_energy_raw = raw_energy;
            self.has_prev_energy = 1;
            return 0;
        }

        let delta_ticks = raw_energy.wrapping_sub(self.prev_energy_raw) as u64;
        self.prev_energy_raw = raw_energy;

        let raw_esu = ((input.msr_rapl_unit >> 8) & 0x1F) as u32;
        let esu = if raw_esu >= 8 && raw_esu <= 20 {
            raw_esu
        } else {
            14 // 1 / 16384 Joule per tick on Intel Client CPUs
        };

        // Power (mW) = (delta_ticks * 1000 mJ/J * 1000 ms/s) / (2^esu * elapsed_ms)
        let numerator = delta_ticks.saturating_mul(1_000_000);
        let energy_mj_x1000 = numerator >> esu;
        let power_mw = energy_mj_x1000 / (input.elapsed_ms as u64);

        if power_mw > 250_000 {
            250_000
        } else {
            power_mw as u32
        }
    }

    /// Check whether hardware EC registers (`PFMM` and `ITSM`) match the target profile
    fn ec_matches_profile(input: &ThinkFlowTelemetryInput, profile: ThinkFlowProfile) -> bool {
        match profile {
            ThinkFlowProfile::Quiet => input.ec_pfmm == 0 && input.ec_itsm == 1,
            ThinkFlowProfile::Balance => input.ec_pfmm == 2 && input.ec_itsm == 2,
            ThinkFlowProfile::Performance => input.ec_pfmm == 1 && input.ec_itsm == 4,
            ThinkFlowProfile::Unknown => false,
        }
    }

    /// Populate the exact sequence of `VPC0.DYTC` commands for a target profile
    fn fill_dytc_commands(profile: ThinkFlowProfile, out: &mut ThinkFlowActionOutput) {
        match profile {
            ThinkFlowProfile::Quiet => {
                out.dytc_cmd_count = 3;
                out.dytc_cmds[0] = DYTC_SET_APM_OFF;
                out.dytc_cmds[1] = DYTC_SET_MMC_QUIET;
                out.dytc_cmds[2] = DYTC_SET_AQM_ON;
                out.dytc_cmds[3] = 0;
            }
            ThinkFlowProfile::Balance => {
                out.dytc_cmd_count = 3;
                out.dytc_cmds[0] = DYTC_SET_APM_OFF;
                out.dytc_cmds[1] = DYTC_SET_AQM_OFF;
                out.dytc_cmds[2] = DYTC_SET_MMC_AUTO;
                out.dytc_cmds[3] = 0;
            }
            ThinkFlowProfile::Performance => {
                out.dytc_cmd_count = 3;
                out.dytc_cmds[0] = DYTC_SET_AQM_OFF;
                out.dytc_cmds[1] = DYTC_SET_MMC_PERF;
                out.dytc_cmds[2] = DYTC_SET_APM_ON;
                out.dytc_cmds[3] = 0;
            }
            ThinkFlowProfile::Unknown => {
                out.dytc_cmd_count = 0;
                out.dytc_cmds = [0; 4];
            }
        }
    }

    /// Main control loop step called every timer tick (e.g. 2000 ms)
    pub fn step(&mut self, input: &ThinkFlowTelemetryInput) -> ThinkFlowActionOutput {
        self.total_ticks = self.total_ticks.wrapping_add(1);
        self.ticks_since_dytc_sync = self.ticks_since_dytc_sync.saturating_add(1);

        let (tjmax_c, cpu_temp_c, is_throttling) = Self::decode_temperature(input);
        let pkg_power_mw = self.compute_package_power_mw(input);

        let hwp_max_ratio = ((input.msr_hwp_req >> 8) & 0xFF) as u32;
        let hwp_epp = ((input.msr_hwp_req >> 24) & 0xFF) as u32;

        // Detect macOS Low Power Mode:
        // 1. Primary: `machdep.xcpm.lpm_enabled` sysctl == 1
        // 2. Fallback: HWP Maximum_Performance capped to <= 28 (macOS LPM sets 0x18 = 24)
        let is_lpm = if input.lpm_sysctl >= 0 {
            input.lpm_sysctl > 0
        } else {
            hwp_max_ratio > 0 && hwp_max_ratio <= 28
        };

        // Effective temperature combines CPU digital sensor and EC sensors
        let ec0 = if input.ec_temp0_c < 120 { input.ec_temp0_c as u32 } else { 0 };
        let ec1 = if input.ec_temp1_c < 120 { input.ec_temp1_c as u32 } else { 0 };
        let eff_temp_c = cpu_temp_c.max(ec0).max(ec1);

        // Select target profile according to active policy
        let target_profile = match self.policy {
            ThinkFlowPolicy::Quiet => {
                self.perf_hold_ticks = 0;
                ThinkFlowProfile::Quiet
            }
            ThinkFlowPolicy::Balance => {
                self.perf_hold_ticks = 0;
                ThinkFlowProfile::Balance
            }
            ThinkFlowPolicy::Performance => {
                self.perf_hold_ticks = 0;
                ThinkFlowProfile::Performance
            }
            ThinkFlowPolicy::Auto => {
                if is_lpm {
                    // When macOS Low Power Mode is enabled in System Settings -> Battery,
                    // immediately place EC into Quiet / Cool profile.
                    self.perf_hold_ticks = 0;
                    ThinkFlowProfile::Quiet
                } else {
                    let high_load = eff_temp_c >= AUTO_ESCALATE_TEMP_C
                        || pkg_power_mw >= AUTO_ESCALATE_POWER_MW
                        || is_throttling;

                    let moderate_load = eff_temp_c >= AUTO_DEESCALATE_TEMP_C
                        || pkg_power_mw >= AUTO_DEESCALATE_POWER_MW;

                    if high_load {
                        self.perf_hold_ticks = AUTO_HOLD_TICKS_INITIAL;
                        ThinkFlowProfile::Performance
                    } else if self.perf_hold_ticks > 0 {
                        if moderate_load {
                            if self.perf_hold_ticks < AUTO_HOLD_TICKS_SUSTAIN {
                                self.perf_hold_ticks = AUTO_HOLD_TICKS_SUSTAIN;
                            }
                            ThinkFlowProfile::Performance
                        } else {
                            self.perf_hold_ticks -= 1;
                            if self.perf_hold_ticks > 0 {
                                ThinkFlowProfile::Performance
                            } else {
                                ThinkFlowProfile::Balance
                            }
                        }
                    } else {
                        ThinkFlowProfile::Balance
                    }
                }
            }
        };

        let profile_changed = self.current_profile != target_profile;
        let hw_mismatch = !Self::ec_matches_profile(input, target_profile);
        let periodic_sync = self.ticks_since_dytc_sync >= PERIODIC_DYTC_REFRESH_TICKS;

        let apply_dytc = profile_changed || hw_mismatch || periodic_sync;

        if profile_changed {
            self.current_profile = target_profile;
            self.mode_switch_count = self.mode_switch_count.wrapping_add(1);
        }
        if apply_dytc {
            self.ticks_since_dytc_sync = 0;
        }

        // Check if EC standalone thermal flags (`LXOS = 0x40`, `TPA1 = 1`) need assertion
        let fix_ec_init = (input.ec_lxos != EC_LXOS_STANDALONE_VALUE)
            || (input.ec_tpa1 != EC_TPA1_ACTIVE_VALUE);

        let mut out = ThinkFlowActionOutput {
            target_profile: target_profile as u32,
            apply_dytc: if apply_dytc { 1 } else { 0 },
            fix_ec_init: if fix_ec_init { 1 } else { 0 },
            poll_thermal_zones: 1,
            dytc_cmd_count: 0,
            dytc_cmds: [0; 4],
            computed_tjmax_c: tjmax_c,
            computed_cpu_temp_c: cpu_temp_c,
            computed_pkg_power_mw: pkg_power_mw,
            computed_hwp_max_ratio: hwp_max_ratio,
            computed_hwp_epp: hwp_epp,
            is_lpm_active: if is_lpm { 1 } else { 0 },
            is_throttling: if is_throttling { 1 } else { 0 },
            _pad1: [0; 2],
            hysteresis_counter: self.perf_hold_ticks,
        };

        if apply_dytc {
            Self::fill_dytc_commands(target_profile, &mut out);
        }

        out
    }
}

// ============================================================================
// C-ABI Exports for XNU Kernel Extension (`ThinkFlow.kext`)
// ============================================================================

#[no_mangle]
pub unsafe extern "C" fn thinkflow_engine_init(engine: *mut ThinkFlowEngine) {
    if !engine.is_null() {
        *engine = ThinkFlowEngine::new();
    }
}

#[no_mangle]
pub unsafe extern "C" fn thinkflow_engine_reset_wake(engine: *mut ThinkFlowEngine) {
    if let Some(eng) = engine.as_mut() {
        eng.reset_after_wake();
    }
}

#[no_mangle]
pub unsafe extern "C" fn thinkflow_engine_set_policy(engine: *mut ThinkFlowEngine, policy: u32) {
    if let Some(eng) = engine.as_mut() {
        eng.set_policy(ThinkFlowPolicy::from_u32(policy));
    }
}

#[no_mangle]
pub unsafe extern "C" fn thinkflow_engine_get_policy(engine: *const ThinkFlowEngine) -> u32 {
    if let Some(eng) = engine.as_ref() {
        eng.policy as u32
    } else {
        ThinkFlowPolicy::Auto as u32
    }
}

#[no_mangle]
pub unsafe extern "C" fn thinkflow_engine_step(
    engine: *mut ThinkFlowEngine,
    input: *const ThinkFlowTelemetryInput,
    output: *mut ThinkFlowActionOutput,
) {
    if let (Some(eng), Some(inp), Some(out)) = (engine.as_mut(), input.as_ref(), output.as_mut()) {
        *out = eng.step(inp);
    }
}

// Panic handler required for `#![no_std]` staticlib builds
#[cfg(all(not(test), feature = "kernel"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> ThinkFlowTelemetryInput {
        ThinkFlowTelemetryInput {
            msr_temp_target: 100 << 16,               // TjMax = 100°C
            msr_pkg_therm: (1 << 31) | (50 << 16),    // 100 - 50 = 50°C
            msr_core_therm: (1 << 31) | (52 << 16),   // 100 - 52 = 48°C
            msr_hwp_req: (0x80 << 24) | (0x32 << 8),  // EPP=0x80, Max=50
            msr_pkg_energy: 0,
            msr_rapl_unit: 14 << 8,                   // 1/16384 J
            lpm_sysctl: 0,
            ec_temp0_c: 48,
            ec_temp1_c: 45,
            ec_fan_raw: 0,
            ec_pfmm: 2,
            ec_itsm: 2,
            ec_lxos: 0x40,
            ec_tpa1: 1,
            _pad0: 0,
            elapsed_ms: 2000,
        }
    }

    #[test]
    fn test_lpm_switches_to_quiet() {
        let mut engine = ThinkFlowEngine::new();
        let mut inp = base_input();
        inp.lpm_sysctl = 1; // macOS Low Power Mode ON

        let out = engine.step(&inp);
        assert_eq!(out.target_profile, ThinkFlowProfile::Quiet as u32);
        assert_eq!(out.is_lpm_active, 1);
        assert_eq!(out.apply_dytc, 1);
        assert_eq!(out.dytc_cmd_count, 3);
        assert_eq!(out.dytc_cmds[0], DYTC_SET_APM_OFF);
        assert_eq!(out.dytc_cmds[1], DYTC_SET_MMC_QUIET);
        assert_eq!(out.dytc_cmds[2], DYTC_SET_AQM_ON);
    }

    #[test]
    fn test_normal_idle_selects_balance() {
        let mut engine = ThinkFlowEngine::new();
        let inp = base_input();

        let out = engine.step(&inp);
        assert_eq!(out.target_profile, ThinkFlowProfile::Balance as u32);
        assert_eq!(out.computed_cpu_temp_c, 50);
        assert_eq!(out.apply_dytc, 1);
        assert_eq!(out.dytc_cmds[2], DYTC_SET_MMC_AUTO);
    }

    #[test]
    fn test_high_load_escalates_to_performance_with_hysteresis() {
        let mut engine = ThinkFlowEngine::new();
        let mut inp = base_input();
        let _ = engine.step(&inp); // Initial tick sets baseline energy

        // Simulate 40W load over 2 seconds: 80 J = 80 * 16384 = 1310720 ticks
        inp.msr_pkg_energy = 1_310_720;
        inp.msr_pkg_therm = (1 << 31) | (20 << 16); // 80°C
        inp.ec_pfmm = 1;
        inp.ec_itsm = 4;

        let out = engine.step(&inp);
        assert_eq!(out.target_profile, ThinkFlowProfile::Performance as u32);
        assert_eq!(out.computed_cpu_temp_c, 80);
        assert_eq!(out.computed_pkg_power_mw, 40_000);
        assert_eq!(out.hysteresis_counter, AUTO_HOLD_TICKS_INITIAL);
        assert_eq!(out.dytc_cmds[1], DYTC_SET_MMC_PERF);
        assert_eq!(out.dytc_cmds[2], DYTC_SET_APM_ON);

        // Now drop load to 10W and 50°C; hysteresis should hold Performance for AUTO_HOLD_TICKS_INITIAL - 1 ticks
        for remaining in (1..AUTO_HOLD_TICKS_INITIAL).rev() {
            inp.msr_pkg_energy += 10 * 2 * 16384; // 10W over 2s
            inp.msr_pkg_therm = (1 << 31) | (50 << 16); // 50°C
            let hold_out = engine.step(&inp);
            assert_eq!(hold_out.target_profile, ThinkFlowProfile::Performance as u32);
            assert_eq!(hold_out.hysteresis_counter, remaining);
        }

        // Next low-load tick transitions back to Balance
        inp.msr_pkg_energy += 10 * 2 * 16384;
        let final_out = engine.step(&inp);
        assert_eq!(final_out.target_profile, ThinkFlowProfile::Balance as u32);
        assert_eq!(final_out.hysteresis_counter, 0);
    }

    #[test]
    fn test_ec_init_fix_detection() {
        let mut engine = ThinkFlowEngine::new();
        let mut inp = base_input();
        inp.ec_lxos = 0x00; // Windows/broken mode
        let out = engine.step(&inp);
        assert_eq!(out.fix_ec_init, 1);

        inp.ec_lxos = 0x40;
        inp.ec_tpa1 = 1;
        let out2 = engine.step(&inp);
        assert_eq!(out2.fix_ec_init, 0);
    }
}
