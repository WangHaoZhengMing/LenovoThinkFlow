#include "ThinkFlow.hpp"
#include <mach/kmod.h>
#include <pexpert/pexpert.h>

#ifndef THINKFLOW_VERSION
#define THINKFLOW_VERSION "1.0.2"
#endif

#define super IOService
OSDefineMetaClassAndStructors(ThinkFlow, IOService)

namespace {

enum {
    kThinkFlowPowerStateOff = 0,
    kThinkFlowPowerStateOn  = 1,
    kThinkFlowPowerStateCount = 2
};

static IOPMPowerState gPowerStates[kThinkFlowPowerStateCount] = {
    { kIOPMPowerStateVersion1, 0,                 0,        0,        0, 0, 0, 0, 0, 0, 0, 0 },
    { kIOPMPowerStateVersion1, kIOPMPowerOn,      kIOPMPowerOn, kIOPMPowerOn, 0, 0, 0, 0, 0, 0, 0, 0 }
};

static const char *policyToString(uint32_t policy) {
    switch (policy) {
        case 1:  return "Quiet";
        case 2:  return "Balance";
        case 3:  return "Performance";
        case 4:  return "Cool";
        default: return "Auto";
    }
}

static const char *profileToString(uint32_t profile) {
    switch (profile) {
        case 1:  return "Quiet";
        case 2:  return "Balance";
        case 3:  return "Performance";
        case 4:  return "Cool";
        default: return "Unknown";
    }
}

static IOACPIPlatformDevice *findAcpiDeviceByPaths(const char *const *paths, size_t count) {
    for (size_t i = 0; i < count; ++i) {
        IORegistryEntry *entry = IORegistryEntry::fromPath(paths[i], gIOACPIPlane);
        if (entry) {
            IOACPIPlatformDevice *dev = OSDynamicCast(IOACPIPlatformDevice, entry);
            if (dev) {
                return dev;
            }
            entry->release();
        }
    }
    return nullptr;
}

} // namespace

bool ThinkFlow::init(OSDictionary *dictionary) {
    if (!super::init(dictionary)) {
        return false;
    }
    thinkflow_engine_init(&fEngine);
    return true;
}

IOService *ThinkFlow::probe(IOService *provider, SInt32 *score) {
    int disableArg = 0;
    if (PE_parse_boot_argn("-thinkflowoff", &disableArg, sizeof(disableArg))) {
        IOLog("ThinkFlow: Disabled via -thinkflowoff boot-arg\n");
        return nullptr;
    }

    IOACPIPlatformDevice *vpc = OSDynamicCast(IOACPIPlatformDevice, provider);
    if (!vpc) {
        return nullptr;
    }

    if (vpc->validateObject("DYTC") != kIOReturnSuccess) {
        IOLog("ThinkFlow: VPC0 provider does not expose DYTC method\n");
        return nullptr;
    }

    return super::probe(provider, score);
}

bool ThinkFlow::start(IOService *provider) {
    if (!super::start(provider)) {
        return false;
    }

    fVpcDevice = OSDynamicCast(IOACPIPlatformDevice, provider);
    if (!fVpcDevice) {
        return false;
    }
    fVpcDevice->retain();

    // Locate parent EC device in IOACPIPlane (works across EC0, EC, H_EC, etc.)
    IORegistryEntry *parentEntry = fVpcDevice->getParentEntry(gIOACPIPlane);
    fEcDevice = OSDynamicCast(IOACPIPlatformDevice, parentEntry);
    if (fEcDevice) {
        fEcDevice->retain();
    }

    // Locate ACPI Thermal Zones across Lenovo DSDT naming conventions
    static const char *const kTz0Paths[] = { "/TZS0", "/_TZ/TZS0", "/TZ00", "/_TZ/TZ00" };
    static const char *const kTz1Paths[] = { "/TZS1", "/_TZ/TZS1", "/TZ01", "/_TZ/TZ01" };
    fTzs0Device = findAcpiDeviceByPaths(kTz0Paths, sizeof(kTz0Paths) / sizeof(kTz0Paths[0]));
    fTzs1Device = findAcpiDeviceByPaths(kTz1Paths, sizeof(kTz1Paths) / sizeof(kTz1Paths[0]));

    // Map ECMB (0xFE802000, len 0x200) read-only for direct zero-overhead thermal sensor reads
    fEcmbDesc = IOMemoryDescriptor::withPhysicalAddress(
        kEcmbPhysicalAddress,
        kEcmbMappingSize,
        kIODirectionIn
    );
    if (fEcmbDesc) {
        fEcmbMap = fEcmbDesc->map(kIOMapInhibitCache);
    }

    // Check if SSDT-THINKFLOW helper methods (`TFIN` / `TFPL`) are present on VPC0
    fHasSsdtHelper = (fVpcDevice->validateObject("TFIN") == kIOReturnSuccess) &&
                     (fVpcDevice->validateObject("TFPL") == kIOReturnSuccess);

    // Optional initial policy boot-arg (`thinkflowmode=0..4`)
    uint32_t bootPolicy = 0;
    if (PE_parse_boot_argn("thinkflowmode", &bootPolicy, sizeof(bootPolicy))) {
        thinkflow_engine_set_policy(&fEngine, bootPolicy);
    }

    fWorkLoop = getWorkLoop();
    if (!fWorkLoop) {
        IOLog("ThinkFlow: Failed to obtain IOWorkLoop\n");
        return false;
    }
    fWorkLoop->retain();

    fCommandGate = IOCommandGate::commandGate(this);
    if (!fCommandGate || fWorkLoop->addEventSource(fCommandGate) != kIOReturnSuccess) {
        IOLog("ThinkFlow: Failed to register IOCommandGate\n");
        return false;
    }

    fTimer = IOTimerEventSource::timerEventSource(this, &ThinkFlow::timerCallback);
    if (!fTimer || fWorkLoop->addEventSource(fTimer) != kIOReturnSuccess) {
        IOLog("ThinkFlow: Failed to register IOTimerEventSource\n");
        return false;
    }

    // Initialize hardware EC thermal loop & perform first control step
    initHardware();
    pollTick();
    fTimer->setTimeoutMS(kPollIntervalMs);

    // Register with IOKit power management to re-initialize EC after sleep/wake
    PMinit();
    provider->joinPMtree(this);
    registerPowerDriver(this, gPowerStates, kThinkFlowPowerStateCount);

    setProperty("Version", THINKFLOW_VERSION);
    setProperty("SSDTHelperPresent", fHasSsdtHelper);
    registerService();

    IOLog("ThinkFlow: Started on Lenovo VPC0 (VPC2004), SSDT=%s\n",
          fHasSsdtHelper ? "present" : "direct-ec");
    return true;
}

void ThinkFlow::stop(IOService *provider) {
    PMstop();

    if (fTimer) {
        fTimer->cancelTimeout();
        if (fWorkLoop) {
            fWorkLoop->removeEventSource(fTimer);
        }
        OSSafeReleaseNULL(fTimer);
    }

    if (fCommandGate) {
        if (fWorkLoop) {
            fWorkLoop->removeEventSource(fCommandGate);
        }
        OSSafeReleaseNULL(fCommandGate);
    }

    OSSafeReleaseNULL(fWorkLoop);
    OSSafeReleaseNULL(fEcmbMap);
    OSSafeReleaseNULL(fEcmbDesc);
    OSSafeReleaseNULL(fTzs0Device);
    OSSafeReleaseNULL(fTzs1Device);
    OSSafeReleaseNULL(fEcDevice);
    OSSafeReleaseNULL(fVpcDevice);

    super::stop(provider);
}

void ThinkFlow::free() {
    super::free();
}

IOReturn ThinkFlow::setPowerState(unsigned long powerStateOrdinal, IOService *whatDevice) {
    if (powerStateOrdinal == kThinkFlowPowerStateOff) {
        fIsAwake = false;
        if (fTimer) {
            fTimer->cancelTimeout();
        }
    } else if (powerStateOrdinal == kThinkFlowPowerStateOn) {
        fIsAwake = true;
        thinkflow_engine_reset_wake(&fEngine);
        initHardware();
        if (fTimer) {
            fTimer->setTimeoutMS(500);
        }
    }
    return kIOPMAckImplied;
}

void ThinkFlow::timerCallback(OSObject *owner, IOTimerEventSource *sender) {
    ThinkFlow *self = OSDynamicCast(ThinkFlow, owner);
    if (!self || !self->fIsAwake) {
        return;
    }
    self->pollTick();
    if (sender && self->fIsAwake) {
        sender->setTimeoutMS(kPollIntervalMs);
    }
}

void ThinkFlow::initHardware() {
    if (!fVpcDevice) {
        return;
    }

    if (fHasSsdtHelper) {
        uint32_t res = 0;
        fVpcDevice->evaluateInteger("TFIN", &res);
    }

    // Assert Lenovo EC standalone OS thermal flag (LXOS = 0x40) and Thermal Participant Active (TPA1 = 1)
    writeEcByte(EC_OFFSET_LXOS, 0x40);
    writeEcByte(EC_OFFSET_TPA1, 0x01);

    // Query DYTC status once to unlock DYTC state machine if uninitialized
    uint32_t dytcStatus = 0;
    evaluateDytc(0x00000002, &dytcStatus);
}

bool ThinkFlow::evaluateDytc(uint32_t cmd, uint32_t *outResult) {
    if (!fVpcDevice) {
        return false;
    }
    OSNumber *arg = OSNumber::withNumber(cmd, 32);
    if (!arg) {
        return false;
    }
    OSObject *params[1] = { arg };
    uint32_t result = 0;
    IOReturn ret = fVpcDevice->evaluateInteger("DYTC", &result, params, 1);
    arg->release();

    if (outResult) {
        *outResult = result;
    }
    return (ret == kIOReturnSuccess);
}

bool ThinkFlow::readEcByte(uint32_t offset, uint8_t *outVal) {
    if (!outVal) {
        return false;
    }
    IOACPIPlatformDevice *target = fEcDevice ? fEcDevice : fVpcDevice;
    if (!target) {
        return false;
    }
    IOACPIAddress addr {};
    addr.addr64 = offset;
    UInt64 val64 = 0;
    IOReturn ret = target->readAddressSpace(
        &val64,
        kIOACPIAddressSpaceIDEmbeddedController,
        addr,
        8,
        0,
        0
    );
    if (ret == kIOReturnSuccess) {
        *outVal = static_cast<uint8_t>(val64 & 0xFF);
        return true;
    }
    return false;
}

bool ThinkFlow::writeEcByte(uint32_t offset, uint8_t val) {
    IOACPIPlatformDevice *target = fEcDevice ? fEcDevice : fVpcDevice;
    if (!target) {
        return false;
    }
    IOACPIAddress addr {};
    addr.addr64 = offset;
    IOReturn ret = target->writeAddressSpace(
        static_cast<UInt64>(val),
        kIOACPIAddressSpaceIDEmbeddedController,
        addr,
        8,
        0,
        0
    );
    return (ret == kIOReturnSuccess);
}

void ThinkFlow::sampleTelemetry(ThinkFlowTelemetryInput *input) {
    bzero(input, sizeof(*input));
    input->elapsed_ms = kPollIntervalMs;

    // 1. Read Intel architectural & package MSRs safely via XNU rdmsr64_carefully
    uint64_t val = 0;
    if (rdmsr64_carefully(MSR_TEMPERATURE_TARGET, &val) == 0) {
        input->msr_temp_target = val;
    }
    if (rdmsr64_carefully(MSR_IA32_PACKAGE_THERM_STATUS, &val) == 0) {
        input->msr_pkg_therm = val;
    }
    if (rdmsr64_carefully(MSR_IA32_THERM_STATUS, &val) == 0) {
        input->msr_core_therm = val;
    }
    if (rdmsr64_carefully(MSR_IA32_HWP_REQUEST, &val) == 0) {
        input->msr_hwp_req = val;
    }
    if (rdmsr64_carefully(MSR_PKG_ENERGY_STATUS, &val) == 0) {
        input->msr_pkg_energy = val;
    }
    if (rdmsr64_carefully(MSR_RAPL_POWER_UNIT, &val) == 0) {
        input->msr_rapl_unit = val;
    }

    // 2. Read macOS Low Power Mode sysctl (`machdep.xcpm.lpm_enabled`)
    int32_t lpmVal = 0;
    size_t lpmLen = sizeof(lpmVal);
    if (sysctlbyname("machdep.xcpm.lpm_enabled", &lpmVal, &lpmLen, nullptr, 0) == 0) {
        input->lpm_sysctl = lpmVal;
    } else {
        input->lpm_sysctl = -1;
    }

    // 3. Read Lenovo EC & ECMB telemetry via SSDT helper `TFPL` (if loaded) or direct EC/MMIO
    bool gotSsdtTelemetry = false;
    if (fHasSsdtHelper && fVpcDevice) {
        uint64_t packed = 0;
        if (fVpcDevice->evaluateInteger("TFPL", &packed) == kIOReturnSuccess && packed != 0) {
            // TFPL packs:
            //   bits [7:0]   = THS0 (°C)
            //   bits [15:8]  = THS1 (°C)
            //   bits [23:16] = FANS (raw fan speed)
            //   bits [27:24] = PFMM (bits 1..2 of 0xC5)
            //   bits [31:28] = ITSM (bits 3..5 of 0xC6)
            //   bits [39:32] = LXOS (0x58)
            //   bits [47:40] = TPA1 (0xCF)
            input->ec_temp0_c = static_cast<uint8_t>(packed & 0xFF);
            input->ec_temp1_c = static_cast<uint8_t>((packed >> 8) & 0xFF);
            input->ec_fan_raw = static_cast<uint8_t>((packed >> 16) & 0xFF);
            input->ec_pfmm    = static_cast<uint8_t>((packed >> 24) & 0x0F);
            input->ec_itsm    = static_cast<uint8_t>((packed >> 28) & 0x0F);
            input->ec_lxos    = static_cast<uint8_t>((packed >> 32) & 0xFF);
            input->ec_tpa1    = static_cast<uint8_t>((packed >> 40) & 0xFF);
            gotSsdtTelemetry  = true;
        }
    }

    if (!gotSsdtTelemetry) {
        if (fEcmbMap) {
            const volatile uint8_t *ecmb = reinterpret_cast<const volatile uint8_t *>(
                fEcmbMap->getVirtualAddress()
            );
            if (ecmb) {
                input->ec_temp0_c = ecmb[ECMB_OFFSET_THS0];
                input->ec_temp1_c = ecmb[ECMB_OFFSET_THS1];
            }
        }

        uint8_t b = 0;
        if (readEcByte(EC_OFFSET_FANS, &b)) {
            input->ec_fan_raw = b;
        }
        if (readEcByte(EC_OFFSET_PFMM, &b)) {
            input->ec_pfmm = (b >> 1) & 0x03;
        }
        if (readEcByte(EC_OFFSET_ITSM, &b)) {
            input->ec_itsm = (b >> 3) & 0x07;
        }
        if (readEcByte(EC_OFFSET_LXOS, &b)) {
            input->ec_lxos = b;
        }
        if (readEcByte(EC_OFFSET_TPA1, &b)) {
            input->ec_tpa1 = b;
        }
    }
}

void ThinkFlow::pollTick() {
    ThinkFlowTelemetryInput input {};
    ThinkFlowActionOutput output {};

    sampleTelemetry(&input);
    thinkflow_engine_step(&fEngine, &input, &output);

    if (output.fix_ec_init) {
        initHardware();
    }

    // Keep ACPI thermal zones TZS0 & TZS1 active when not already polled by TFPL
    if (output.poll_thermal_zones && !fHasSsdtHelper) {
        uint32_t tmpKelvin = 0;
        if (fTzs0Device) {
            fTzs0Device->evaluateInteger("_TMP", &tmpKelvin);
        }
        if (fTzs1Device) {
            fTzs1Device->evaluateInteger("_TMP", &tmpKelvin);
        }
    }

    // Execute VPC0.DYTC thermal/fan mode command sequence when requested by the Rust engine
    if (output.apply_dytc && output.dytc_cmd_count > 0) {
        uint8_t count = (output.dytc_cmd_count <= 4) ? output.dytc_cmd_count : 4;
        for (uint8_t i = 0; i < count; ++i) {
            uint32_t cmd = output.dytc_cmds[i];
            if (cmd != 0) {
                evaluateDytc(cmd);
            }
        }
    }

    publishProperties(input, output);
}

void ThinkFlow::publishProperties(const ThinkFlowTelemetryInput &input,
                                  const ThinkFlowActionOutput &output) {
    uint32_t policy = thinkflow_engine_get_policy(&fEngine);
    setProperty("PolicyMode", policy, 32);
    setProperty("PolicyName", policyToString(policy));
    setProperty("ActiveProfile", output.target_profile, 32);
    setProperty("ProfileName", profileToString(output.target_profile));
    setProperty("CPUTempC", output.computed_cpu_temp_c, 32);
    setProperty("CPUTjMaxC", output.computed_tjmax_c, 32);
    setProperty("PackagePowerMW", output.computed_pkg_power_mw, 32);
    setProperty("ECTemp0C", input.ec_temp0_c, 32);
    setProperty("ECTemp1C", input.ec_temp1_c, 32);
    setProperty("ECFanRaw", input.ec_fan_raw, 32);
    setProperty("ECPFMM", input.ec_pfmm, 32);
    setProperty("ECITSM", input.ec_itsm, 32);
    setProperty("ECLXOS", input.ec_lxos, 32);
    setProperty("ECTPA1", input.ec_tpa1, 32);
    setProperty("HWPMaxRatio", output.computed_hwp_max_ratio, 32);
    setProperty("HWPEPP", output.computed_hwp_epp, 32);
    setProperty("LowPowerMode", output.is_lpm_active != 0);
    setProperty("ThermalThrottling", output.is_throttling != 0);
    setProperty("HysteresisTicks", output.hysteresis_counter, 32);
    setProperty("ModeSwitchCount", fEngine.mode_switch_count, 32);
    setProperty("TotalTicks", fEngine.total_ticks, 64);
}

IOReturn ThinkFlow::setPolicyGated(OSObject *owner, void *arg0, void *, void *, void *) {
    ThinkFlow *self = OSDynamicCast(ThinkFlow, owner);
    if (!self || !arg0) {
        return kIOReturnBadArgument;
    }
    uint32_t newPolicy = *reinterpret_cast<uint32_t *>(arg0);
    thinkflow_engine_set_policy(&self->fEngine, newPolicy);
    self->pollTick();
    return kIOReturnSuccess;
}

IOReturn ThinkFlow::setProperties(OSObject *properties) {
    OSDictionary *dict = OSDynamicCast(OSDictionary, properties);
    if (!dict) {
        return kIOReturnBadArgument;
    }

    OSNumber *num = OSDynamicCast(OSNumber, dict->getObject("PolicyMode"));
    if (num && fCommandGate) {
        uint32_t mode = num->unsigned32BitValue();
        if (mode <= 4) {
            return fCommandGate->runAction(&ThinkFlow::setPolicyGated, &mode);
        }
    }
    return kIOReturnUnsupported;
}

// ============================================================================
// XNU Kmod Module Declarations
// ============================================================================

extern "C" {
extern kern_return_t _start(kmod_info_t *, void *);
extern kern_return_t _stop(kmod_info_t *, void *);

__attribute__((visibility("default")))
KMOD_EXPLICIT_DECL(org.thinkflow.driver, THINKFLOW_VERSION, _start, _stop)

__attribute__((visibility("hidden")))
kmod_start_func_t *_realmain = nullptr;

__attribute__((visibility("hidden")))
kmod_stop_func_t *_antimain = nullptr;

__attribute__((visibility("hidden")))
int _kext_apple_cc = __APPLE_CC__;
}
