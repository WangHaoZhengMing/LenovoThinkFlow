# ThinkFlow (`v1.0.0`)

[![License: BSD-3-Clause](https://img.shields.io/badge/License-BSD--3--Clause-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-macOS%2010.15+-lightgrey.svg)]()
[![Rust](https://img.shields.io/badge/Rust-2021%20edition-orange.svg)]()
[![OpenCore](https://img.shields.io/badge/OpenCore-0.8.0+-brightgreen.svg)]()

**Lenovo EC Thermal & Fan Scheduler (`VPC2004` / `DYTC`) + macOS Low Power Mode Bridge**  
Designed for Lenovo laptops featuring `VPC2004` (`VPC0`) / `DYTC` Intelligent Thermal Solution (IdeaPad, ThinkBook, Yoga, ZhaoYang, Legion) running macOS under OpenCore.

[English](#english) | [中文说明](#chinese)

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
   - Without active `VPC0.DYTC` evaluations, EC registers `PFMM` (`0xC5` bits `[2:1]`) and `ITSM` (`0xC6` bits `[5:3]`) never transition to **Performance / Beast Mode** (`PFMM=1, ITSM=4`) under load or **Quiet / Cool Mode** (`PFMM=0, ITSM=1`) when macOS Low Power Mode is active.
   - Furthermore, when `\_SB.IETM.DPTE` is non-zero, `DYTC` skips `PL1S()` MCHBAR MMIO power-limit updates.
3. **`SMCProcessor.kext` Missing on 13th Gen Raptor Lake-H (`0xB06A2`)**:
   - `Lilu.kext <= 1.7.2` includes desktop Raptor Lake (`0xB7`, `0xBF`) in its CPUID jump table, but mobile Raptor Lake-H (`0xBA` = 186, e.g. Core i7-13700H) defaults to `Unknown (0)` unless `lilucpu=17` is added to `boot-args`.

---

### 2. Architecture

```text
ThinkFlow/
├── Cargo.toml                  # Rust workspace manifest
├── Makefile                    # Make entrypoint (make build / test / clean)
├── build.py                    # One-click compiler & kernel symbol auditor
├── rust-core/                  # #![no_std] Rust kernel state machine (libthinkflow_core.a)
│   ├── Cargo.toml
│   └── src/lib.rs
├── kext/                       # C++ IOKit driver matching IOACPIPlatformDevice "VPC2004"
│   ├── ThinkFlow.hpp
│   └── ThinkFlow.cpp
├── acpi/                       # SSDT-THINKFLOW.dsl & pure-Python AML compiler
│   ├── SSDT-THINKFLOW.dsl
│   └── gen_ssdt.py
├── cli/                        # Pure Rust userland CLI & live telemetry monitor (`thinkflow`)
│   ├── Cargo.toml
│   └── src/main.rs
└── build/                      # Built artifacts output directory
    ├── ThinkFlow.kext
    ├── SSDT-THINKFLOW.aml
    └── thinkflow
```

---

### 3. Thermal State Machine (`rust-core`)

| Policy / Trigger | Lenovo EC Profile | `VPC0.DYTC` Command Sequence | EC Registers (`0xC5` / `0xC6`) |
| :--- | :--- | :--- | :--- |
| **macOS Low Power Mode ON** (`sysctl machdep.xcpm.lpm_enabled = 1`) or `thinkflow mode quiet` | **Quiet / Cool Mode** | `0x000F5001` -> `0x0013B001` -> `0x001F6001` | `PFMM = 0`, `ITSM = 1` |
| **Normal Mode (Light Load)** (`Temp < 72°C` & `Power < 26W`) or `thinkflow mode balance` | **Balance / Auto Mode** | `0x000F5001` -> `0x000F6001` -> `0x000FB001` | `PFMM = 2`, `ITSM = 2` |
| **Normal Mode (Heavy Load)** (`Temp >= 72°C` or `Power >= 26W` or Thermal Throttling) or `thinkflow mode performance` | **Performance / Beast Mode** (with 12s hysteresis hold) | `0x000F6001` -> `0x0012B001` -> `0x001F5001` | `PFMM = 1`, `ITSM = 4` |

---

### 4. Build Instructions

#### Prerequisites

- macOS with Xcode Command Line Tools (`clang++`, `ld`, `nm`)
- Rust toolchain (`cargo`, `rustc` via [rustup.rs](https://rustup.rs))
- Python 3.9+
- [MacKernelSDK](https://github.com/acidanthera/MacKernelSDK) (automatically cloned by `build.py` if not present)

#### Compilation

```bash
# One-click build via python or make:
python3 build.py
# or
make build
```

The script runs Rust unit tests, builds `libthinkflow_core.a` (`#![no_std]`, zero-FPU), links `ThinkFlow.kext`, audits Mach-O kernel symbols against `/System/Library/Kernels/kernel`, generates `SSDT-THINKFLOW.aml`, and builds the `thinkflow` CLI binary.

---

### 5. OpenCore Deployment

1. Copy `build/ThinkFlow.kext` to `EFI/OC/Kexts/` and add it to `config.plist -> Kernel -> Add`.
2. Copy `build/SSDT-THINKFLOW.aml` to `EFI/OC/ACPI/` and add it to `config.plist -> ACPI -> Add`.
3. *(For 13th Gen Raptor Lake-H CPUs)* Add `lilucpu=17` to `config.plist -> NVRAM -> Add -> 7C436110-AB2A-4BBB-A880-FE41995C9F82 -> boot-args` so `VirtualSMC` / `SMCProcessor.kext` properly maps CPU generation.

#### Boot Arguments

- `thinkflowmode=<0..3>`: Override initial policy at boot (`0`=Auto, `1`=Quiet, `2`=Balance, `3`=Performance).
- `-thinkflowoff`: Emergency disable flag for `ThinkFlow.kext`.

---

<a name="chinese"></a>
## 中文说明

### 1. 核心问题与原理剖析

联想笔记本（昭阳、ThinkBook、小新/IdeaPad、拯救者/Legion、Yoga）在黑苹果 macOS 下常出现风扇不转或高负载不提速的问题，主要由以下原因导致：

1. **EC 固件操作系统模式死锁 (`LXOS` vs `OSTP`)**：
   - 联想 DSDT 的 `EC0._INI` 会检查 `_OSI("Linux")`。
   - 黑苹果常用的 `SSDT-XOSI.aml` 会伪装为 Windows，导致 EC 寄存器 `OSTP` 被写入 5（Windows 10+），而独立温控标志 `LXOS` 保持为 0。
   - 此时 EC 会关闭内置的独立风扇升温曲线，等待 Windows 专用的 Intel DPTF (`\_SB.IETM`) 与联想 ITS (`VPC2004`) 驱动下发策略。
2. **`VPC0.DYTC` 缺乏调度**：
   - macOS 原生没有联想 ITS 驱动，`DYTC` 从未被调用，导致 EC 内部状态一直卡在默认或低功耗档，无法进入野兽模式（Performance / Beast）。
3. **13 代移动端 Raptor Lake-H (`0xB06A2`) 识别缺失**：
   - `Lilu.kext 1.7.2` 的 CPU 表默认缺少对移动版 i7-13700H（Model `0xBA` = 186）的代数映射，导致 `SMCProcessor.kext` 无法识别 CPU。添加 `lilucpu=17` 即可解决。

---

### 2. ThinkFlow 的工作方式

- **内核扩展 (`ThinkFlow.kext`)**：
  - 核心状态机基于 Rust `#![no_std]` 编写，零浮点、内核安全。
  - 自动读取 Intel MSR 封装功耗与温度、联想 EC 真实传感器 (`THS0`/`THS1`)、风扇读数 (`FANS`)。
  - **原生联动 macOS 电池低电量模式**：在「系统设置 -> 电池」中开启低电量模式时，自动下发 `DYTC` 切换至**静音省电档** (`PFMM=0, ITSM=1`)；平时保持**智能均衡档** (`PFMM=2, ITSM=2`)；高负载或过热时自动进入**野兽满血档** (`PFMM=1, ITSM=4`)，并带有 12 秒迟滞防抖，避免风扇反复启停。
- **ACPI 补丁 (`SSDT-THINKFLOW.aml`)**：
  - 在 `\_SB.PC00.LPCB.EC0.VPC0` 下注入 `TFIN` 与 `TFPL`，激活 `EC0.LXOS = 0x40`、`TPA1 = 1`，将 `_SB.IETM.DPTE` 置零以解锁 MCHBAR MMIO PL1 功耗墙控制。
- **用户态 CLI (`thinkflow`)**：
  - 免 root 读取 CPU 温度、功耗、真实 EC 传感器及风扇档位，并支持运行时无感切换模式。

---

### 3. CLI 工具使用指南

```bash
# 查看实时状态（CPU 功耗、核心温度、EC 风扇读数、当前档位）
thinkflow status

# 实时滚动监测（带频率与功耗）
thinkflow monitor 1.0

# 运行全核压测验证风扇调度与野兽模式提速
thinkflow bench 6

# 手动切换档位
thinkflow mode auto         # 自动跟随 macOS 低电量模式 + 负载
thinkflow mode quiet        # 强制静音省电 (PFMM=0, ITSM=1)
thinkflow mode balance      # 强制日常均衡 (PFMM=2, ITSM=2)
thinkflow mode performance  # 强制野兽满血 (PFMM=1, ITSM=4)
```

---

## License

This project is licensed under the [BSD 3-Clause License](LICENSE).
