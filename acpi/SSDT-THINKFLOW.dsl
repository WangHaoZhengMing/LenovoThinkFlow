/*
 * SSDT-THINKFLOW.dsl
 *
 * ThinkFlow ACPI Helper Table for Lenovo Laptops with VPC2004 (DYTC) EC
 * (IdeaPad / ThinkBook / Yoga / ZhaoYang / Legion)
 *
 * Purpose:
 *   1. Snapshots BIOS boot values of DPTE/LXOS/TPA1/KTEE on first TFIN() call,
 *      then sets \_SB.IETM.DPTE = Zero so VPC0.DYTC executes PL1S() MCHBAR MMIO
 *      power limit updates on macOS.
 *   2. Asserts \_SB.PC00.LPCB.EC0.LXOS (0x40 for 8-bit field, or One if 1-bit
 *      FieldUnitObj), TPA1 = 1, KTEE = 1 so the EC autonomously ramps fan curves.
 *   3. Provides VPC0.TFSL() for S0ix Modern Standby coexistence: restores exact
 *      BIOS EC/DPTF flags before PEPD._DSM sleep entry so EC firmware never hangs.
 *   4. Provides VPC0.TFPL() to refresh \_TZ.TZS0._TMP / \_TZ.TZS1._TMP and read
 *      packed 64-bit EC telemetry in a single atomic evaluation.
 */
DefinitionBlock ("", "SSDT", 2, "THKFLW", "THKFLOW", 0x00001001)
{
    External (\_SB_.IETM.DPTE, IntObj)
    External (\_SB_.PC00.LPCB.EC0_, DeviceObj)
    External (\_SB_.PC00.LPCB.EC0_.FANS, FieldUnitObj)
    External (\_SB_.PC00.LPCB.EC0_.ITSM, FieldUnitObj)
    External (\_SB_.PC00.LPCB.EC0_.KTEE, FieldUnitObj)
    External (\_SB_.PC00.LPCB.EC0_.LXOS, FieldUnitObj)
    External (\_SB_.PC00.LPCB.EC0_.PFMM, FieldUnitObj)
    External (\_SB_.PC00.LPCB.EC0_.THS0, FieldUnitObj)
    External (\_SB_.PC00.LPCB.EC0_.THS1, FieldUnitObj)
    External (\_SB_.PC00.LPCB.EC0_.TPA1, FieldUnitObj)
    External (\_SB_.PC00.LPCB.EC0_.VPC0, DeviceObj)
    External (\_TZ_.TZS0._TMP, MethodObj)
    External (\_TZ_.TZS1._TMP, MethodObj)

    Scope (\_SB.PC00.LPCB.EC0.VPC0)
    {
        Name (TFS0, Zero)
        Name (SDPT, Zero)
        Name (SLXO, Zero)
        Name (STPA, Zero)
        Name (SKTE, Zero)

        Method (TFIN, 0, Serialized)
        {
            If (_OSI ("Darwin"))
            {
                If (LEqual (TFS0, Zero))
                {
                    Store (\_SB.IETM.DPTE, SDPT)
                    Store (\_SB.PC00.LPCB.EC0.LXOS, SLXO)
                    Store (\_SB.PC00.LPCB.EC0.TPA1, STPA)
                    Store (\_SB.PC00.LPCB.EC0.KTEE, SKTE)
                    Store (One, TFS0)
                }

                Store (Zero, \_SB.IETM.DPTE)
                Store (0x40, \_SB.PC00.LPCB.EC0.LXOS)
                If (LEqual (\_SB.PC00.LPCB.EC0.LXOS, Zero))
                {
                    Store (One, \_SB.PC00.LPCB.EC0.LXOS)
                }
                Store (One, \_SB.PC00.LPCB.EC0.TPA1)
                Store (One, \_SB.PC00.LPCB.EC0.KTEE)
            }
            Return (One)
        }

        Method (TFSL, 0, Serialized)
        {
            If (_OSI ("Darwin"))
            {
                If (LEqual (TFS0, One))
                {
                    Store (SLXO, \_SB.PC00.LPCB.EC0.LXOS)
                    Store (STPA, \_SB.PC00.LPCB.EC0.TPA1)
                    Store (SKTE, \_SB.PC00.LPCB.EC0.KTEE)
                    Store (SDPT, \_SB.IETM.DPTE)
                }
                Else
                {
                    Store (Zero, \_SB.PC00.LPCB.EC0.LXOS)
                    Store (Zero, \_SB.PC00.LPCB.EC0.TPA1)
                }
            }
            Return (One)
        }

        Method (TFPL, 0, Serialized)
        {
            \_TZ.TZS0._TMP ()
            \_TZ.TZS1._TMP ()

            Store (\_SB.PC00.LPCB.EC0.THS0, Local0)
            Or (Local0, ShiftLeft (\_SB.PC00.LPCB.EC0.THS1, 0x08), Local0)
            Or (Local0, ShiftLeft (\_SB.PC00.LPCB.EC0.FANS, 0x10), Local0)
            Or (Local0, ShiftLeft (\_SB.PC00.LPCB.EC0.PFMM, 0x18), Local0)
            Or (Local0, ShiftLeft (\_SB.PC00.LPCB.EC0.ITSM, 0x1C), Local0)
            Or (Local0, ShiftLeft (\_SB.PC00.LPCB.EC0.LXOS, 0x20), Local0)
            Or (Local0, ShiftLeft (\_SB.PC00.LPCB.EC0.TPA1, 0x28), Local0)
            Return (Local0)
        }
    }
}
