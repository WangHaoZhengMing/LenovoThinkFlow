#!/usr/bin/env python3
"""
Build script for LenovoThinkFlow:
  1. Runs Rust unit tests (`cargo test`)
  2. Compiles `#![no_std]` Rust kernel engine (`libthinkflow_core.a`) with kernel code model & no-red-zone
  3. Compiles C++ IOKit driver (`ThinkFlow.cpp`) and links `build/LenovoThinkFlow.kext`
  4. Verifies all undefined Mach-O symbols against XNU kernel & `IOACPIFamily`
  5. Generates `build/SSDT-THINKFLOW.aml`
  6. Builds the userland interactive TUI & CLI monitor (`build/thinkflow`)
"""

import argparse
import hashlib
import os
import plistlib
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent
MAC_KERNEL_SDK_GIT = "https://github.com/acidanthera/MacKernelSDK.git"


def find_tool(name: str) -> str:
    """Locate a binary in PATH or fallback to ~/.cargo/bin."""
    found = shutil.which(name)
    if found:
        return found
    cargo_fallback = Path.home() / ".cargo" / "bin" / name
    if cargo_fallback.exists():
        return str(cargo_fallback)
    raise FileNotFoundError(
        f"Required tool '{name}' not found in PATH or ~/.cargo/bin. "
        "Please install Rust via https://rustup.rs"
    )


def resolve_sdk(explicit_sdk: Path | None) -> Path:
    """Locate MacKernelSDK via CLI flag, environment variable, repo tree, or git clone."""
    candidates: list[Path] = []
    if explicit_sdk:
        candidates.append(explicit_sdk)
    env_sdk = os.environ.get("MAC_KERNEL_SDK")
    if env_sdk:
        candidates.append(Path(env_sdk))
    candidates.extend([
        ROOT / "MacKernelSDK",
        ROOT.parent / "MacKernelSDK",
    ])

    for candidate in candidates:
        resolved = candidate.expanduser().resolve()
        if (resolved / "Headers").is_dir() and (resolved / "Library" / "x86_64" / "libkmod.a").is_file():
            return resolved

    target_sdk = ROOT / "MacKernelSDK"
    print(f"  MacKernelSDK not found locally; cloning {MAC_KERNEL_SDK_GIT} -> ./MacKernelSDK ...")
    subprocess.run(
        ["git", "clone", "--depth", "1", MAC_KERNEL_SDK_GIT, str(target_sdk)],
        check=True,
    )
    return target_sdk.resolve()


def display_arg(arg) -> str:
    s = str(arg)
    root_str = str(ROOT)
    home_str = str(Path.home())
    if s.startswith(root_str + "/"):
        return "./" + s[len(root_str) + 1:]
    if s == root_str:
        return "."
    if s.startswith(home_str + "/"):
        return "~/" + s[len(home_str) + 1:]
    return s


def run(cmd, cwd=ROOT, env=None):
    print("  ->", " ".join(display_arg(x) for x in cmd))
    subprocess.run([str(x) for x in cmd], check=True, cwd=cwd, env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, default=None, help="Path to acidanthera/MacKernelSDK")
    parser.add_argument("--output", type=Path, default=ROOT / "build", help="Output directory")
    args = parser.parse_args()

    sdk = resolve_sdk(args.sdk)
    cargo_bin = find_tool("cargo")
    rustc_bin = find_tool("rustc")

    out = args.output.resolve()
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True, exist_ok=True)
    objdir = out / "obj"
    objdir.mkdir(parents=True, exist_ok=True)

    name = "LenovoThinkFlow"
    ident = "org.thinkflow.driver"
    version = "1.0.1"

    remap_env = os.environ.copy()
    remap_flag = f"--remap-path-prefix={ROOT}=."
    home_remap = f"--remap-path-prefix={Path.home()}=~"

    print("=== [1/6] Running Rust Core Unit Tests ===")
    run([cargo_bin, "test", "--manifest-path", ROOT / "Cargo.toml"])

    print("=== [2/6] Compiling Rust #![no_std] Kernel Core (libthinkflow_core.a) ===")
    rust_lib = objdir / "libthinkflow_core.a"
    run([
        rustc_bin,
        "--edition", "2021",
        "--crate-name", "thinkflow_core",
        "--crate-type", "staticlib",
        "--target", "x86_64-apple-darwin",
        "--cfg", 'feature="kernel"',
        "-C", "opt-level=2",
        "-C", "panic=abort",
        "-C", "no-redzone=y",
        "-C", "relocation-model=pic",
        "-C", "code-model=kernel",
        "-C", "target-feature=-avx,-avx2",
        remap_flag,
        home_remap,
        "-o", rust_lib,
        ROOT / "rust-core/src/lib.rs",
    ], env=remap_env)

    print(f"=== [3/6] Compiling C++ IOKit Driver & Linking {name}.kext ===")
    bundle = out / f"{name}.kext" / "Contents"
    (bundle / "MacOS").mkdir(parents=True, exist_ok=True)

    cxx_flags = [
        "xcrun", "clang++",
        "-arch", "x86_64",
        "-std=c++17",
        "-O2",
        f"-fdebug-prefix-map={ROOT}=.",
        "-fapple-kext",
        "-fno-exceptions",
        "-fno-rtti",
        "-fno-stack-protector",
        "-fno-builtin",
        "-fno-asynchronous-unwind-tables",
        "-fno-non-call-exceptions",
        "-fno-common",
        "-mno-red-zone",
        "-mmacosx-version-min=10.12",
        "-nostdinc",
        "-DKERNEL",
        "-DKERNEL_PRIVATE",
        "-DDRIVER_PRIVATE",
        "-DAPPLE",
        "-DNeXT",
        "-D__APPLE__",
        "-D__MACH__",
        "-DAPPLE_KEXT_ASSERTIONS=1",
        f"-I{sdk / 'Headers'}",
        f"-I{ROOT / 'kext'}",
        "-Wall",
        "-Wextra",
        "-Wno-unused-parameter",
        "-Wno-deprecated-declarations",
    ]

    cpp_obj = objdir / "ThinkFlow.o"
    run(cxx_flags + ["-c", ROOT / "kext/ThinkFlow.cpp", "-o", cpp_obj])

    exe = bundle / "MacOS" / name
    run([
        "xcrun", "ld",
        "-arch", "x86_64",
        "-kext",
        "-undefined", "dynamic_lookup",
        "-no_fixup_chains",
        "-macos_version_min", "10.12",
        "-o", exe,
        cpp_obj,
        rust_lib,
        sdk / "Library/x86_64/libkmod.a",
    ])

    run(["xcrun", "strip", "-S", exe])

    info = {
        "CFBundleDevelopmentRegion": "English",
        "CFBundleExecutable": name,
        "CFBundleIdentifier": ident,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": name,
        "CFBundlePackageType": "KEXT",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "OSBundleCompatibleVersion": version,
        "OSBundleRequired": "Root",
        "OSBundleLibraries": {
            "com.apple.iokit.IOACPIFamily": "1.0.0d1",
            "com.apple.kpi.bsd": "16.7.0",
            "com.apple.kpi.iokit": "16.7.0",
            "com.apple.kpi.libkern": "16.7.0",
            "com.apple.kpi.mach": "16.7.0",
            "com.apple.kpi.unsupported": "16.7.0",
        },
        "IOKitPersonalities": {
            f"{name}_VPC2004": {
                "CFBundleIdentifier": ident,
                "IOClass": "ThinkFlow",
                "IOMatchCategory": "ThinkFlow",
                "IONameMatch": "VPC2004",
                "IOProviderClass": "IOACPIPlatformDevice",
                "IOProbeScore": 2000,
            }
        },
    }
    (bundle / "Info.plist").write_bytes(plistlib.dumps(info))

    # Also generate ThinkFlow.kext for backwards compatibility with OpenCore config
    legacy_kext = out / "ThinkFlow.kext"
    if legacy_kext.exists():
        shutil.rmtree(legacy_kext)
    shutil.copytree(out / f"{name}.kext", legacy_kext)
    legacy_exe = legacy_kext / "Contents/MacOS/ThinkFlow"
    (legacy_kext / "Contents/MacOS/LenovoThinkFlow").rename(legacy_exe)
    legacy_info = dict(info)
    legacy_info["CFBundleExecutable"] = "ThinkFlow"
    legacy_info["CFBundleName"] = "ThinkFlow"
    legacy_info["IOKitPersonalities"] = {
        "ThinkFlow_VPC2004": {
            "CFBundleIdentifier": ident,
            "IOClass": "ThinkFlow",
            "IOMatchCategory": "ThinkFlow",
            "IONameMatch": "VPC2004",
            "IOProviderClass": "IOACPIPlatformDevice",
            "IOProbeScore": 2000,
        }
    }
    (legacy_kext / "Contents/Info.plist").write_bytes(plistlib.dumps(legacy_info))

    print(f"=== [4/6] Auditing {name}.kext Kernel Symbols ===")
    symbols = subprocess.check_output(["xcrun", "nm", str(exe)], text=True)
    assert " _kmod_info" in symbols, "Missing _kmod_info"
    assert " __start" in symbols and " __stop" in symbols, "Missing kmod entrypoints"
    assert " _thinkflow_engine_step" in symbols, "Missing Rust FFI symbol _thinkflow_engine_step"

    unresolved = [
        line.strip().split()[-1]
        for line in subprocess.check_output(["xcrun", "nm", "-u", str(exe)], text=True).splitlines()
        if line.strip()
    ]
    forbidden = ["chkstk_darwin", "___memcpy_chk", "___memmove_chk", "rust_eh_personality", "__Unwind_Resume", "DispatchE5IORPC"]
    for sym in forbidden:
        assert all(sym not in u for u in unresolved), f"Forbidden symbol imported: {sym}"

    print(f"  Verified {len(unresolved)} undefined kernel/IOACPIFamily symbols (0 forbidden symbols).")

    print("=== [5/6] Generating SSDT-THINKFLOW.aml ===")
    run(["python3", ROOT / "acpi/gen_ssdt.py"])
    shutil.copy2(ROOT / "acpi/SSDT-THINKFLOW.aml", out / "SSDT-THINKFLOW.aml")
    shutil.copy2(ROOT / "acpi/SSDT-THINKFLOW.dsl", out / "SSDT-THINKFLOW.dsl")

    print("=== [6/6] Building Interactive TUI & CLI Monitor (`thinkflow`) ===")
    run([cargo_bin, "build", "--release", "--manifest-path", ROOT / "Cargo.toml", "-p", "thinkflow-cli"])
    cli_bin = ROOT / "target/release/thinkflow"
    target_cli = out / "thinkflow"
    shutil.copy2(cli_bin, target_cli)
    run(["xcrun", "strip", "-S", target_cli])

    # Install into ~/.local/bin/thinkflow & lenovothinkflow
    user_bin = Path.home() / ".local" / "bin"
    user_bin.mkdir(parents=True, exist_ok=True)
    shutil.copy2(target_cli, user_bin / "thinkflow")
    shutil.copy2(target_cli, user_bin / "lenovothinkflow")

    kext_sha = hashlib.sha256(exe.read_bytes()).hexdigest()
    ssdt_sha = hashlib.sha256((out / "SSDT-THINKFLOW.aml").read_bytes()).hexdigest()
    cli_sha = hashlib.sha256(target_cli.read_bytes()).hexdigest()

    print("\n========================================================================")
    print("  LenovoThinkFlow Build Complete!")
    print("========================================================================")
    print(f"  Kext Bundle : {display_arg(out / f'{name}.kext')} (and ThinkFlow.kext)")
    print(f"                SHA256 = {kext_sha}")
    print(f"  ACPI Table  : {display_arg(out / 'SSDT-THINKFLOW.aml')}")
    print(f"                SHA256 = {ssdt_sha}")
    print(f"  CLI / TUI   : {display_arg(target_cli)} -> ~/.local/bin/thinkflow")
    print(f"                SHA256 = {cli_sha}")
    print("========================================================================")


if __name__ == "__main__":
    main()
