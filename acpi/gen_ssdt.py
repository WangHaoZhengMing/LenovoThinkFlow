#!/usr/bin/env python3
"""
Pure-Python ACPI AML bytecode compiler for SSDT-THINKFLOW.aml.
Generates a spec-compliant ACPI 2.0 SSDT binary table matching SSDT-THINKFLOW.dsl.
"""

import struct
from pathlib import Path


def encode_pkg_length(payload_len: int) -> bytes:
    """Encode ACPI AML PkgLength (includes the PkgLength bytes themselves)."""
    if payload_len + 1 < 64:
        return bytes([payload_len + 1])
    elif payload_len + 2 < 4096:
        total = payload_len + 2
        b0 = 0x40 | (total & 0x0F)
        b1 = (total >> 4) & 0xFF
        return bytes([b0, b1])
    else:
        total = payload_len + 3
        b0 = 0x80 | (total & 0x0F)
        b1 = (total >> 4) & 0xFF
        b2 = (total >> 12) & 0xFF
        return bytes([b0, b1, b2])


def aml_name_path(segments: list[str], root: bool = True) -> bytes:
    """Encode an ACPI NameString (e.g. \\_SB.PC00.LPCB.EC0.VPC0)."""
    prefix = b"\\" if root else b""
    segs = [s.ljust(4, "_").encode("ascii")[:4] for s in segments]
    if len(segs) == 1:
        return prefix + segs[0]
    elif len(segs) == 2:
        return prefix + b"\x2E" + segs[0] + segs[1]
    else:
        return prefix + b"\x2F" + bytes([len(segs)]) + b"".join(segs)


def aml_string(s: str) -> bytes:
    return b"\x0D" + s.encode("ascii") + b"\x00"


def aml_byte(val: int) -> bytes:
    if val == 0:
        return b"\x00"  # ZeroOp
    elif val == 1:
        return b"\x01"  # OneOp
    return b"\x0A" + bytes([val & 0xFF])  # BytePrefix


def aml_name(name: str, data_obj: bytes) -> bytes:
    # DefName := NameOp (0x08) NameString DataRefObject
    return b"\x08" + name.ljust(4, "_").encode("ascii")[:4] + data_obj


def aml_store(source: bytes, target: bytes) -> bytes:
    return b"\x70" + source + target  # StoreOp


def aml_lequal(op1: bytes, op2: bytes) -> bytes:
    return b"\x93" + op1 + op2  # LEqualOp


def aml_shift_left(source: bytes, count: int) -> bytes:
    # ShiftLeftOp (0x79) TermArg ShiftCount Target(NullName=0x00)
    return b"\x79" + source + aml_byte(count) + b"\x00"


def aml_or(op1: bytes, op2: bytes, target: bytes) -> bytes:
    # OrOp (0x7D) TermArg TermArg Target
    return b"\x7D" + op1 + op2 + target


def aml_if(predicate: bytes, body: bytes) -> bytes:
    payload = predicate + body
    return b"\xA0" + encode_pkg_length(len(payload)) + payload


def aml_else(body: bytes) -> bytes:
    return b"\xA1" + encode_pkg_length(len(body)) + body


def aml_method(name: str, arg_count: int, serialized: bool, body: bytes) -> bytes:
    flags = (arg_count & 0x07) | (0x08 if serialized else 0x00)
    payload = name.ljust(4, "_").encode("ascii")[:4] + bytes([flags]) + body
    return b"\x14" + encode_pkg_length(len(payload)) + payload


def aml_scope(path_bytes: bytes, body: bytes) -> bytes:
    payload = path_bytes + body
    return b"\x10" + encode_pkg_length(len(payload)) + payload


def build_ssdt_thinkflow() -> bytes:
    # External Paths
    p_dpte = aml_name_path(["_SB_", "IETM", "DPTE"])
    p_lxos = aml_name_path(["_SB_", "PC00", "LPCB", "EC0_", "LXOS"])
    p_tpa1 = aml_name_path(["_SB_", "PC00", "LPCB", "EC0_", "TPA1"])
    p_ktee = aml_name_path(["_SB_", "PC00", "LPCB", "EC0_", "KTEE"])
    p_ths0 = aml_name_path(["_SB_", "PC00", "LPCB", "EC0_", "THS0"])
    p_ths1 = aml_name_path(["_SB_", "PC00", "LPCB", "EC0_", "THS1"])
    p_fans = aml_name_path(["_SB_", "PC00", "LPCB", "EC0_", "FANS"])
    p_pfmm = aml_name_path(["_SB_", "PC00", "LPCB", "EC0_", "PFMM"])
    p_itsm = aml_name_path(["_SB_", "PC00", "LPCB", "EC0_", "ITSM"])
    p_vpc0 = aml_name_path(["_SB_", "PC00", "LPCB", "EC0_", "VPC0"])
    p_tzs0_tmp = aml_name_path(["_TZ_", "TZS0", "_TMP"])
    p_tzs1_tmp = aml_name_path(["_TZ_", "TZS1", "_TMP"])

    # Local Scope Names under VPC0 for S0ix sleep/wake state preservation
    n_tfs0 = aml_name_path(["TFS0"], root=False)
    n_sdpt = aml_name_path(["SDPT"], root=False)
    n_slxo = aml_name_path(["SLXO"], root=False)
    n_stpa = aml_name_path(["STPA"], root=False)
    n_skte = aml_name_path(["SKTE"], root=False)

    state_defs = (
        aml_name("TFS0", aml_byte(0))
        + aml_name("SDPT", aml_byte(0))
        + aml_name("SLXO", aml_byte(0))
        + aml_name("STPA", aml_byte(0))
        + aml_name("SKTE", aml_byte(0))
    )

    local0 = b"\x60"  # Local0Op
    osi_darwin = b"_OSI" + aml_string("Darwin")

    # Method (TFIN, 0, Serialized)
    save_boot_state = aml_if(
        aml_lequal(n_tfs0, aml_byte(0)),
        aml_store(p_dpte, n_sdpt)
        + aml_store(p_lxos, n_slxo)
        + aml_store(p_tpa1, n_stpa)
        + aml_store(p_ktee, n_skte)
        + aml_store(aml_byte(1), n_tfs0),
    )
    lxos_1bit_fallback = aml_if(
        aml_lequal(p_lxos, aml_byte(0)),
        aml_store(aml_byte(1), p_lxos),
    )
    tfin_if_body = (
        save_boot_state
        + aml_store(aml_byte(0), p_dpte)
        + aml_store(aml_byte(0x40), p_lxos)
        + lxos_1bit_fallback
        + aml_store(aml_byte(1), p_tpa1)
        + aml_store(aml_byte(1), p_ktee)
    )
    tfin_body = aml_if(osi_darwin, tfin_if_body) + b"\xA4" + aml_byte(1)
    m_tfin = aml_method("TFIN", 0, True, tfin_body)

    # Method (TFSL, 0, Serialized) - Restore BIOS EC/DPTF state before S0ix Modern Standby
    tfsl_restore = (
        aml_if(
            aml_lequal(n_tfs0, aml_byte(1)),
            aml_store(n_slxo, p_lxos)
            + aml_store(n_stpa, p_tpa1)
            + aml_store(n_skte, p_ktee)
            + aml_store(n_sdpt, p_dpte),
        )
        + aml_else(
            aml_store(aml_byte(0), p_lxos)
            + aml_store(aml_byte(0), p_tpa1)
        )
    )
    tfsl_body = aml_if(osi_darwin, tfsl_restore) + b"\xA4" + aml_byte(1)
    m_tfsl = aml_method("TFSL", 0, True, tfsl_body)

    # Method (TFPL, 0, Serialized)
    tfpl_body = (
        p_tzs0_tmp  # Call \_TZ.TZS0._TMP() (0 args)
        + p_tzs1_tmp  # Call \_TZ.TZS1._TMP() (0 args)
        + aml_store(p_ths0, local0)
        + aml_or(local0, aml_shift_left(p_ths1, 0x08), local0)
        + aml_or(local0, aml_shift_left(p_fans, 0x10), local0)
        + aml_or(local0, aml_shift_left(p_pfmm, 0x18), local0)
        + aml_or(local0, aml_shift_left(p_itsm, 0x1C), local0)
        + aml_or(local0, aml_shift_left(p_lxos, 0x20), local0)
        + aml_or(local0, aml_shift_left(p_tpa1, 0x28), local0)
        + b"\xA4"
        + local0  # Return (Local0)
    )
    m_tfpl = aml_method("TFPL", 0, True, tfpl_body)

    # Scope (\_SB.PC00.LPCB.EC0.VPC0) { state_defs, TFIN, TFSL, TFPL }
    aml_body = aml_scope(p_vpc0, state_defs + m_tfin + m_tfsl + m_tfpl)

    # 36-byte ACPI Standard Header
    total_len = 36 + len(aml_body)
    header_without_csum = struct.pack(
        "<4sIBB6s8sI4sI",
        b"SSDT",
        total_len,
        2,              # Revision 2 (64-bit integer arithmetic for Local0)
        0,              # Checksum placeholder
        b"THKFLW",      # OEM ID
        b"THKFLOW ",    # OEM Table ID
        0x00001001,     # OEM Revision (1.0.1 S0ix Coexistence)
        b"TFAC",        # Creator ID
        0x20261107,     # Creator Revision
    )
    raw = bytearray(header_without_csum + aml_body)
    raw[9] = (-sum(raw)) & 0xFF
    return bytes(raw)


def main():
    out_dir = Path(__file__).resolve().parent
    aml_bytes = build_ssdt_thinkflow()
    out_path = out_dir / "SSDT-THINKFLOW.aml"
    out_path.write_bytes(aml_bytes)
    print(f"  Generated acpi/{out_path.name} ({len(aml_bytes)} bytes, checksum=0x{aml_bytes[9]:02X}, sum={sum(aml_bytes) & 0xFF})")


if __name__ == "__main__":
    main()
