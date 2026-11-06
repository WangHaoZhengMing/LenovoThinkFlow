#!/usr/bin/env python3
"""
Build script for ThinkFlow:
  1. Runs Rust unit tests (`cargo test`)
  2. Compiles `#![no_std]` Rust kernel engine (`libthinkflow_core.a`) with kernel code model & no-red-zone
  3. Compiles C++ IOKit driver (`ThinkFlow.cpp`) and links `build/ThinkFlow.kext`
  4. Verifies all undefined Mach-O symbols in `ThinkFlow.kext` against XNU kernel & `IOACPIFamily`
  5. Generates `build/SSDT-THINKFLOW.aml`
  6. Builds the userland `build/thinkflow` Rust CLI tool
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
    parser.add_argument(
        "--sdk",
        type=Path,
        default=None,
        help="Path to Acidanthera MacKernelSDK (defaults to $MAC_KERNEL_SDK or ./MacKernelSDK)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "build",
        help="Output directory for built artifacts (default: ./build)",
    )
    parser.add_argument(
        "--symbols",
        action="store_true",
        help="Generate a .dSYM bundle alongside ThinkFlow.kext for kernel debugging",
    )
    args = parser.parse_args()

    cargo_bin = find_tool("cargo")
    rustc_bin = find_tool("rustc")
    sdk = resolve_sdk(args.sdk)
    out = args.output.expanduser().resolve()

    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True, exist_ok=True)
    objdir = out / "obj"
    objdir.mkdir(parents=True, exist_ok=True)

    name = "ThinkFlow"
    ident = "org.thinkflow.driver"
    version = "1.0.0"

    # Strip local filesystem path prefixes (including spaces) from compiler outputs
    remap_env = os.environ.copy()
    remap_flag = f"--remap-path-prefix={ROOT}=."
    home_remap_flag = f"--remap-path-prefix={Path.home()}=~"
    remap_env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join([remap_flag, home_remap_flag])

    print("=== [1/6] Running Rust Core Unit Tests ===")
    run([cargo_bin, "test", "--manifest-path", ROOT / "Cargo.toml"], env=remap_env)

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
        home_remap_flag,
        "-o", rust_lib,
        ROOT / "rust-core/src/lib.rs",
    ])

    print("=== [3/6] Compiling C++ IOKit Kext Wrapper & Linking ThinkFlow.kext ===")
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
    if args.symbols:
        cxx_flags.append("-g")

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

    if args.symbols:
        run(["xcrun", "dsymutil", exe, "-o", out / f"{name}.kext.dSYM"])
    run(["xcrun", "strip", "-S", exe])

    # Remove intermediate object files after linking
    shutil.rmtree(objdir, ignore_errors=True)

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
            "ThinkFlow_VPC2004": {
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

    print("=== [4/6] Auditing ThinkFlow.kext Kernel Symbols ===")
    symbols = subprocess.check_output(["xcrun", "nm", str(exe)], text=True)
    assert " _kmod_info" in symbols, "Missing _kmod_info"
    assert " __start" in symbols and " __stop" in symbols, "Missing kmod entrypoints"
    assert " _thinkflow_engine_step" in symbols, "Missing Rust FFI symbol _thinkflow_engine_step"

    unresolved = [
        line.strip().split()[-1]
        for line in subprocess.check_output(["xcrun", "nm", "-u", str(exe)], text=True).splitlines()
        if line.strip()
    ]
    forbidden = [
        "chkstk_darwin",
        "___memcpy_chk",
        "___memmove_chk",
        "rust_eh_personality",
        "__Unwind_Resume",
        "DispatchE5IORPC",
    ]
    for sym in forbidden:
        assert all(sym not in u for u in unresolved), f"Forbidden symbol imported: {sym}"

    print(f"  Verified {len(unresolved)} undefined kernel/IOACPIFamily symbols (0 forbidden symbols).")

    print("=== [5/6] Generating SSDT-THINKFLOW.aml ===")
    run(["python3", ROOT / "acpi/gen_ssdt.py"])
    shutil.copy2(ROOT / "acpi/SSDT-THINKFLOW.aml", out / "SSDT-THINKFLOW.aml")
    shutil.copy2(ROOT / "acpi/SSDT-THINKFLOW.dsl", out / "SSDT-THINKFLOW.dsl")

    print("=== [6/6] Building Userland Rust CLI (`thinkflow`) ===")
    run(
        [cargo_bin, "build", "--release", "--manifest-path", ROOT / "Cargo.toml", "-p", "thinkflow-cli"],
        env=remap_env,
    )
    cli_bin = ROOT / "target/release/thinkflow"
    shutil.copy2(cli_bin, out / "thinkflow")
    run(["xcrun", "strip", "-S", out / "thinkflow"])

    kext_sha = hashlib.sha256(exe.read_bytes()).hexdigest()
    ssdt_sha = hashlib.sha256((out / "SSDT-THINKFLOW.aml").read_bytes()).hexdigest()
    cli_sha = hashlib.sha256((out / "thinkflow").read_bytes()).hexdigest()

    rel_out = out.relative_to(ROOT) if out.is_relative_to(ROOT) else out
    print("\n========================================================================")
    print("  ThinkFlow Build Complete!")
    print("========================================================================")
    print(f"  Kext Bundle : {rel_out / 'ThinkFlow.kext'}")
    print(f"                SHA256 = {kext_sha}")
    print(f"  ACPI Table  : {rel_out / 'SSDT-THINKFLOW.aml'}")
    print(f"                SHA256 = {ssdt_sha}")
    print(f"  User CLI    : {rel_out / 'thinkflow'}")
    print(f"                SHA256 = {cli_sha}")
    print("========================================================================")


if __name__ == "__main__":
    main()
