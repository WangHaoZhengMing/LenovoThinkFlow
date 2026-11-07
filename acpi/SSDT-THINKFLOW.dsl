/*
 * SSDT-THINKFLOW.dsl
 *
 * ThinkFlow ACPI Helper Table for Lenovo Laptops with VPC2004 (DYTC) EC
 * (IdeaPad / ThinkBook / Yoga / ZhaoYang / Legion)
 *
 * Purpose:
 *   1. Sets \_SB.IETM.DPTE = Zero so VPC0.DYTC executes PL1S() MCHBAR MMIO power
 *      limit updates on macOS (where the Windows Intel DPTF driver is absent).
 *   2. Asserts \_SB.PC00.LPCB.EC0.LXOS (0x40 for 8-bit field, or One if 1-bit
 *      FieldUnitObj) and TPA1 = 1, KTEE = 1 so the EC autonomously ramps fan curves on CPU heat.
 *   3. Provides VPC0.TFIN() and VPC0.TFPL() helper methods for ThinkFlow.kext to
 *      poll \_TZ.TZS0._TMP / \_TZ.TZS1._TMP and read packed EC telemetry in a
 *      single atomic evaluation.
 *
 * Note:
 *   If your DSDT uses PCI0 instead of PC00 (e.g. 10th Gen Comet Lake / Ice Lake or
 *   older), ThinkFlow.kext automatically falls back to direct EC/MMIO mode even
 *   without this SSDT, or you can replace PC00 with PCI0 and recompile via `iasl`.
 */
DefinitionBlock ("", "SSDT", 2, "THKFLW", "THKFLOW", 0x00001000)
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
        Method (TFIN, 0, Serialized)
        {
            If (_OSI ("Darwin"))
            {
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
