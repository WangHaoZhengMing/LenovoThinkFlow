use std::time::Duration;

use thinkflow_core::ThinkFlowPolicy;

use crate::app::AppState;
use crate::cpu::{CoreKind, CoreLiveMetric};
use crate::telemetry::set_kext_policy;

// ============================================================================
// CLI Subcommands (Plain Status, Per-Core P/E Monitor & Mode Control)
// ============================================================================

pub(crate) fn print_per_core_table(state: &AppState) {
    let p_cores: Vec<&CoreLiveMetric> = state
        .core_metrics
        .iter()
        .filter(|c| c.kind == CoreKind::Performance)
        .collect();
    let e_cores: Vec<&CoreLiveMetric> = state
        .core_metrics
        .iter()
        .filter(|c| c.kind == CoreKind::Efficiency)
        .collect();

    println!("------------------------------------------------------------------------");
    println!(
        "  CPU Topology         : {} (P = Performance 性能核, E = Efficiency 能效核)",
        state.cpu_topo_summary
    );

    if !p_cores.is_empty() {
        let p_threads: usize = p_cores.iter().map(|c| c.threads.len()).sum();
        println!(
            "  [P-Cores · 性能大核] : {} Physical Cores / {} Threads (Avg {:.2} GHz)",
            p_cores.len(),
            p_threads,
            state.telemetry.p_freq_ghz
        );
        for c in &p_cores {
            let filled = ((c.usage_pct.clamp(0.0, 100.0) / 100.0) * 14.0).round() as usize;
            let bar = format!(
                "{}{}",
                "█".repeat(filled),
                "░".repeat(14usize.saturating_sub(filled))
            );
            let threads_label = if c.threads.len() == 2 {
                format!("CPU{:>2},{:>2}", c.threads[0], c.threads[1])
            } else if let Some(&t0) = c.threads.first() {
                format!("CPU{:>2}   ", t0)
            } else {
                "CPU--   ".to_string()
            };
            let smt_detail = if c.thread_usages.len() >= 2 {
                format!(
                    " (HT: {:>4.1}% / {:>4.1}%)",
                    c.thread_usages[0], c.thread_usages[1]
                )
            } else {
                String::new()
            };
            println!(
                "    [{:<2} | P-Core] ({}) [{}] {:>5.1}% | {:>4.2} GHz{}",
                c.label, threads_label, bar, c.usage_pct, c.freq_ghz, smt_detail
            );
        }
    }

    if !e_cores.is_empty() {
        let e_threads: usize = e_cores.iter().map(|c| c.threads.len()).sum();
        println!(
            "  [E-Cores · 能效小核] : {} Physical Cores / {} Threads (Avg {:.2} GHz)",
            e_cores.len(),
            e_threads,
            state.telemetry.e_freq_ghz
        );
        for c in &e_cores {
            let filled = ((c.usage_pct.clamp(0.0, 100.0) / 100.0) * 14.0).round() as usize;
            let bar = format!(
                "{}{}",
                "█".repeat(filled),
                "░".repeat(14usize.saturating_sub(filled))
            );
            let threads_label = if let Some(&t0) = c.threads.first() {
                format!("CPU{:>2}   ", t0)
            } else {
                "CPU--   ".to_string()
            };
            println!(
                "    [{:<2} | E-Core] ({}) [{}] {:>5.1}% | {:>4.2} GHz",
                c.label, threads_label, bar, c.usage_pct, c.freq_ghz
            );
        }
    }
}

pub(crate) fn print_status() {
    let mut state = AppState::new();
    std::thread::sleep(Duration::from_millis(200));
    state.update_telemetry();
    let t = &state.telemetry;

    println!("========================================================================");
    println!(concat!(
        "  LenovoThinkFlow v",
        env!("CARGO_PKG_VERSION"),
        " — Lenovo EC Thermal & Fan Scheduler for macOS"
    ));
    println!("========================================================================");
    println!("  Host Name            : {}", state.hostname);
    println!(
        "  CPU Model            : {} [{}]",
        state.cpu_brand, state.cpu_topo_summary
    );
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
        println!(
            "  Active Policy        : {} (Mode {})",
            t.policy_name, t.policy_mode
        );
        println!(
            "  Active EC Profile    : {} (PFMM={}, ITSM={})",
            t.profile_name, t.ec_pfmm, t.ec_itsm
        );
        let pl1_note = if t.effective_pl1_w > 0.5 {
            format!(", PL1 {:.0} W", t.effective_pl1_w)
        } else if t.target_pl1_w > 0.5 {
            format!(", Cap {:.0} W", t.target_pl1_w)
        } else {
            String::new()
        };
        println!(
            "  CPU Telemetry        : {} °C (TjMax {} °C) | Package Power: {:.2} W (PP0 {:.2} W{})",
            t.cpu_temp_c, t.tjmax_c, t.pkg_power_w, t.core_power_w, pl1_note
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
            "  EC Standalone Flags  : LXOS=0x{:02X} | TPA1={} | SSDT-THINKFLOW={}",
            t.ec_lxos,
            t.ec_tpa1,
            if t.ssdt_present {
                "Loaded"
            } else {
                "Direct EC Mode"
            }
        );
        println!(
            "  Engine Counters      : Total Ticks = {} | Mode Switches = {} | Hysteresis = {} ticks",
            t.total_ticks, t.mode_switch_count, t.hysteresis_counter
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
        println!(
            "  Simulated Profile    : {} (PFMM={}, ITSM={})",
            t.profile_name, t.ec_pfmm, t.ec_itsm
        );
    }

    print_per_core_table(&state);
    println!("========================================================================");
}

pub(crate) fn print_cores() {
    let mut state = AppState::new();
    std::thread::sleep(Duration::from_millis(250));
    state.update_telemetry();
    println!("========================================================================");
    println!(concat!(
        "  LenovoThinkFlow v",
        env!("CARGO_PKG_VERSION"),
        " — Per-Core P/E Live Telemetry"
    ));
    print_per_core_table(&state);
    println!("========================================================================");
}

pub(crate) fn handle_mode_command(mode_arg: &str) {
    let (policy, name) = match mode_arg.to_ascii_lowercase().as_str() {
        "auto" | "default" => (ThinkFlowPolicy::Auto, "Auto (Adaptive)"),
        "cool" | "breeze" | "icy" => (ThinkFlowPolicy::Cool, "Cool (Low Power + Adaptive Fan)"),
        "quiet" | "silent" => (ThinkFlowPolicy::Quiet, "Quiet (Fan Suppressed)"),
        "balance" | "normal" => (ThinkFlowPolicy::Balance, "Balance (Normal)"),
        "performance" | "perf" | "beast" | "turbo" => {
            (ThinkFlowPolicy::Performance, "Performance (Beast)")
        }
        _ => {
            eprintln!(
                "Invalid mode '{}'. Expected one of: auto | cool | quiet | balance | performance",
                mode_arg
            );
            std::process::exit(1);
        }
    };

    println!("Switching LenovoThinkFlow Policy -> {}", name);
    match set_kext_policy(policy) {
        Ok(()) => println!("Successfully updated LenovoThinkFlow.kext kernel policy!"),
        Err(msg) => println!("Note: {}", msg),
    }
}

pub(crate) fn print_help() {
    println!(concat!(
        "LenovoThinkFlow v",
        env!("CARGO_PKG_VERSION"),
        " — Lenovo EC Thermal & Fan Control CLI & TUI"
    ));
    println!();
    println!("USAGE:");
    println!("  thinkflow                               Launch the full interactive TUI monitor");
    println!("  thinkflow status                        Print current CPU, EC & P/E per-core status");
    println!("  thinkflow cores                         Print P-Core / E-Core per-core load & frequency");
    println!("  thinkflow mode <auto|cool|quiet|balance|perf> Switch ThinkFlow thermal/fan policy");
    println!("  thinkflow help                          Show this help message");
}
