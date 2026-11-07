# LenovoThinkFlow (`v1.0.2`)

[![CI](https://github.com/WangHaoZhengMing/LenovoThinkFlow/actions/workflows/ci.yml/badge.svg)](https://github.com/WangHaoZhengMing/LenovoThinkFlow/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/WangHaoZhengMing/LenovoThinkFlow)](https://github.com/WangHaoZhengMing/LenovoThinkFlow/releases)
[![License: BSD-3-Clause](https://img.shields.io/badge/License-BSD--3--Clause-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-macOS%2010.15+-lightgrey.svg)]()
[![Rust](https://img.shields.io/badge/Rust-2021%20edition-orange.svg)]()
[![OpenCore](https://img.shields.io/badge/OpenCore-0.8.0+-brightgreen.svg)]()

**Lenovo EC Thermal & Fan Scheduler (`VPC2004` / `DYTC`) + macOS Low Power Mode Bridge**  
Designed for Lenovo laptops featuring `VPC2004` (`VPC0`) / `DYTC` Intelligent Thermal Solution (IdeaPad, ThinkBook, Yoga, ZhaoYang, Legion) running macOS under OpenCore.

[English](#english) | [中文说明](#chinese) | [GitFlow & Release Guide](GITFLOW.md)

---

<a name="english"></a>
## English

### 1. Background & Root Cause

On modern Lenovo laptops running macOS under OpenCore, fans often fail to ramp up under heavy CPU load or get stuck in a fixed low-RPM state. This is caused by three interacting firmware/driver issues:

1. **EC OS-Mode Deadlock (`LXOS` vs `OSTP`)**:
   - In Lenovo's `DSDT.aml` (`EC0._INI`), the Embedded Controller checks `_OSI("Linux")` (via `XOSI`).
   - Standard OpenCore `SSDT-XOSI.aml` tables spoof `"Windows 2015"`. Consequently, the EC sets `OSTP = 5` (Windows 10+) while leaving `LXOS = 0`.
   - In Windows mode, the EC suppresses autonomous hardware fan curves and waits for the Windows Intel DPTF (`\_SB.IETM`) and Lenovo ITS (`IDEA2008` / `VPC2004`) drivers to push thermal tables and `DYTC` commands.
2. **Unscheduled `VPC0.DYTC` (`PFMM` / `ITSM`) & Unpolled Thermal Zones**:
   - Without active `VPC0.DYTC` evaluations, EC registers `PFMM` (`0xC5` bits `[2:1]`) and `ITSM` (`0xC6` bits `[5:3]`) never transition to **Performance / Beast Mode** (`PFMM=1, ITSM=4`) under load or **Cool Mode** (`PFMM=2, ITSM=2` under Low Power cap).
   - Furthermore, when `\_SB.IETM.DPTE` is non-zero, `DYTC` skips `PL1S()` MCHBAR MMIO power-limit updates.
3. **`SMCProcessor.kext` Missing on 13th Gen Raptor Lake-H (`0xB06A2`)**:
   - `Lilu.kext <= 1.7.2` includes desktop Raptor Lake (`0xB7`, `0xBF`) in its CPUID jump table, but mobile Raptor Lake-H (`0xBA` = 186, e.g. Core i7-13700H) defaults to `Unknown (0)` unless `lilucpu=17` is added to `boot-args`.

---

### 2. Architecture

```text
LenovoThinkFlow/
├── Cargo.toml                  # Rust workspace manifest
├── Makefile                    # Make entrypoint (make build / test / release / clean)
├── build.py                    # One-click compiler & kernel symbol auditor
├── rust-core/                  # #![no_std] Rust kernel state machine (libthinkflow_core.a)
│   ├── Cargo.toml
│   └── src/lib.rs
├── kext/                       # C++ IOKit driver matching IOACPIPlatformDevice "VPC2004"
│   ├── ThinkFlow.hpp
│   └── ThinkFlow.cpp
├── acpi/                       # SSDT-THINKFLOW.dsl (TFIN/TFPL) & Python AML compiler
│   ├── SSDT-THINKFLOW.dsl
│   └── gen_ssdt.py
├── cli/                        # Ratatui interactive TUI & CLI telemetry monitor (`thinkflow`)
│   ├── Cargo.toml
│   └── src/main.rs
└── build/                      # Built artifacts output directory
    ├── LenovoThinkFlow.kext    # (plus legacy-compatible ThinkFlow.kext)
    ├── SSDT-THINKFLOW.aml
    └── thinkflow
```

---

### 3. Thermal State Machine (`rust-core`)

| Policy / Trigger | Lenovo EC Profile | `VPC0.DYTC` Command Sequence | EC Registers (`0xC5` / `0xC6`) | Behavior |
| :--- | :--- | :--- | :--- | :--- |
| **macOS Low Power Mode ON** (`machdep.xcpm.lpm_enabled = 1`) or `thinkflow mode cool` | **Cool Mode** (`Policy 4`) | `0x000F5001` -> `0x000F6001` -> `0x000FB001` | `PFMM = 2`, `ITSM = 2` | Low CPU package power + adaptive fan curve (spins down/stops once chilled) |
| `thinkflow mode quiet` (Manual only) | **Quiet Mode** (`Policy 1`) | `0x000F5001` -> `0x0013B001` -> `0x001F6001` | `PFMM = 0`, `ITSM = 1` | Fan suppressed for acoustic silence |
| **Normal Mode (Light Load)** (`Temp < 72°C` & `Power < 26W`) or `thinkflow mode balance` | **Balance Mode** (`Policy 2`) | `0x000F5001` -> `0x000F6001` -> `0x000FB001` | `PFMM = 2`, `ITSM = 2` | Standard adaptive thermal curve |
| **Normal Mode (Heavy Load)** (`Temp >= 72°C` or `Power >= 26W` or Throttling) or `thinkflow mode perf` | **Performance / Beast Mode** (`Policy 3`) | `0x000F6001` -> `0x0012B001` -> `0x001F5001` | `PFMM = 1`, `ITSM = 4` | Maximum fan speed + unlocked PL1 (12s hysteresis hold) |

---

### 4. Build & CLI Usage

```bash
python3 build.py
# or
make build
```

```bash
thinkflow                   # Launch full-screen Ratatui interactive dashboard
thinkflow status            # Print current CPU & EC status once
thinkflow mode auto         # Adaptive mode (default; also press [D] in TUI)
thinkflow mode cool         # Cool mode: low CPU power + adaptive fan cooling
thinkflow mode quiet        # Manual quiet mode (fan suppressed)
thinkflow mode balance      # Manual balance mode
thinkflow mode performance  # Force Beast / Max Fan mode (also press [F] in TUI)
```

**TUI Hotkeys**: `[Q]` Quit │ `[M]` Cycle Policy │ `[D]` Restore Default (`Auto`) │ `[F]` Force Max Fan │ `[R]` Reset History

---

### 5. OpenCore Deployment

1. Copy `build/LenovoThinkFlow.kext` (or `build/ThinkFlow.kext`) to `EFI/OC/Kexts/` and add it to `config.plist -> Kernel -> Add`.
2. Copy `build/SSDT-THINKFLOW.aml` to `EFI/OC/ACPI/` and add it to `config.plist -> ACPI -> Add`.
3. *(For 13th Gen Raptor Lake-H CPUs, e.g. Core i7-13700H)* Add `lilucpu=17` to `boot-args` so `VirtualSMC` / `SMCProcessor.kext` properly recognizes mobile Raptor Lake (`0xB06A2`).

#### Boot Arguments

- `thinkflowmode=<0..4>`: Override initial policy at boot (`0`=Auto, `1`=Quiet, `2`=Balance, `3`=Performance, `4`=Cool).
- `-thinkflowoff`: Emergency disable flag.

---

<a name="chinese"></a>
## 中文说明

### 1. 核心问题与原理剖析

联想笔记本（昭阳、ThinkBook、小新/IdeaPad、拯救者/Legion、Yoga）在黑苹果 macOS 下常出现风扇不转或高负载不提速的问题，主要由以下原因导致：

1. **EC 固件操作系统模式死锁 (`LXOS` vs `OSTP`)**：
   - 联想 DSDT 的 `EC0._INI` 会检查 `_OSI("Linux")`。黑苹果常用的 `SSDT-XOSI.aml` 伪装为 Windows 后，EC 将 `OSTP` 设为 5（Windows 10+），而独立温控标志 `LXOS` 保持为 0，关闭内置独立风扇曲线，傻等 Windows 端 DPTF 与联想 ITS 驱动。
2. **`VPC0.DYTC` 缺乏调度**：
   - macOS 原生没有联想 `VPC2004` 驱动，`DYTC` 从未被调用，EC 无法在高负载时切入野兽模式 (`PFMM=1, ITSM=4`)。
3. **13 代移动端 Raptor Lake-H (`0xB06A2`) 识别缺失**：
   - `Lilu.kext 1.7.2` 默认缺少对移动版 i7-13700H（Model `0xBA` = 186）的代数映射，添加 `lilucpu=17` 即可恢复识别。

---

### 2. 模式设计与低电量联动

- **自动模式 (`Auto`, 默认)**：
  - **开启 macOS 低电量模式时**：自动进入 **清凉模式 (`Cool Mode`)** —— CPU 功耗被系统压低，同时 EC 保持自适应均衡曲线 (`PFMM=2, ITSM=2`) 正常排风散热；当机身吹凉（<45°C）后风扇可自适应减速或停转，彻底告别机身闷热积温。
  - **正常模式日常负载**：保持 **智能均衡档 (`Balance`)**。
  - **高负载 / 高温 (`>=72°C` 或 `>=26W`)**：立即切入 **野兽满血档 (`Performance`, `PFMM=1, ITSM=4`)**，并附带 12 秒迟滞防抖。
- **安静模式 (`Quiet`)**：仅限用户在 TUI/CLI 中手动开启，强制压低风扇转速。
- **TUI 一键复原**：在全屏终端监控界面 (`thinkflow`) 中按 **`[D]`** 键可随时一键恢复默认 `Auto` 模式。

---

## License

This project is licensed under the [BSD 3-Clause License](LICENSE).
