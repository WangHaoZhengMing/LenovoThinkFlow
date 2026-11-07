#ifndef THINKFLOW_HPP
#define THINKFLOW_HPP

#include <IOKit/IOService.h>
#include <IOKit/IOWorkLoop.h>
#include <IOKit/IOTimerEventSource.h>
#include <IOKit/IOCommandGate.h>
#include <IOKit/IOMemoryDescriptor.h>
#include <IOKit/IOMessage.h>
#include <IOKit/pwr_mgt/RootDomain.h>
#include <IOKit/acpi/IOACPIPlatformDevice.h>
#include <IOKit/IOLib.h>
#include <libkern/c++/OSContainers.h>

extern "C" {

// Kernel functions exported by XNU (/System/Library/Kernels/kernel)
extern int sysctlbyname(const char *name, void *oldp, size_t *oldlenp, void *newp, size_t newlen);
extern int rdmsr64_carefully(uint32_t msr, uint64_t *val);

// Rust #![no_std] Core Engine ABI (`libthinkflow_core.a`)
struct ThinkFlowTelemetryInput {
    uint64_t msr_temp_target;
    uint64_t msr_pkg_therm;
    uint64_t msr_core_therm;
    uint64_t msr_hwp_req;
    uint64_t msr_pkg_energy;
    uint64_t msr_rapl_unit;
    int32_t  lpm_sysctl;
    uint8_t  ec_temp0_c;
    uint8_t  ec_temp1_c;
    uint8_t  ec_fan_raw;
    uint8_t  ec_pfmm;
    uint8_t  ec_itsm;
    uint8_t  ec_lxos;
    uint8_t  ec_tpa1;
    uint8_t  _pad0;
    uint32_t elapsed_ms;
};

struct ThinkFlowActionOutput {
    uint32_t target_profile;
    uint8_t  apply_dytc;
    uint8_t  fix_ec_init;
    uint8_t  poll_thermal_zones;
    uint8_t  dytc_cmd_count;
    uint32_t dytc_cmds[4];
    uint32_t computed_tjmax_c;
    uint32_t computed_cpu_temp_c;
    uint32_t computed_pkg_power_mw;
    uint32_t computed_hwp_max_ratio;
    uint32_t computed_hwp_epp;
    uint8_t  is_lpm_active;
    uint8_t  is_throttling;
    uint8_t  _pad1[2];
    uint32_t hysteresis_counter;
};

struct ThinkFlowEngine {
    uint32_t policy;
    uint32_t current_profile;
    uint32_t prev_energy_raw;
    uint8_t  has_prev_energy;
    uint32_t perf_hold_ticks;
    uint32_t ticks_since_dytc_sync;
    uint64_t total_ticks;
    uint32_t mode_switch_count;
};

void     thinkflow_engine_init(ThinkFlowEngine *engine);
void     thinkflow_engine_reset_wake(ThinkFlowEngine *engine);
void     thinkflow_engine_set_policy(ThinkFlowEngine *engine, uint32_t policy);
uint32_t thinkflow_engine_get_policy(const ThinkFlowEngine *engine);
void     thinkflow_engine_step(ThinkFlowEngine *engine,
                               const ThinkFlowTelemetryInput *input,
                               ThinkFlowActionOutput *output);

} // extern "C"

class ThinkFlow : public IOService {
    OSDeclareDefaultStructors(ThinkFlow)

private:
    IOACPIPlatformDevice *fVpcDevice          {nullptr};
    IOACPIPlatformDevice *fEcDevice           {nullptr};
    IOACPIPlatformDevice *fTzs0Device         {nullptr};
    IOACPIPlatformDevice *fTzs1Device         {nullptr};
    IOMemoryDescriptor   *fEcmbDesc           {nullptr};
    IOMemoryMap          *fEcmbMap            {nullptr};
    IOWorkLoop           *fWorkLoop           {nullptr};
    IOTimerEventSource   *fTimer              {nullptr};
    IOCommandGate        *fCommandGate        {nullptr};
    IONotifier           *fSleepWakeNotifier  {nullptr};

    ThinkFlowEngine       fEngine             {};
    bool                  fHasSsdtHelper      {false};
    bool                  fHasSleepHelper     {false};
    bool                  fIsAwake            {true};
    uint32_t              fTicksSinceEcInit   {0};
    uint32_t              fSleepHandoffCount  {0};

    static constexpr uint32_t kPollIntervalMs       = 2000;
    static constexpr uint32_t kWakeResumeDelayMs    = 1500;
    static constexpr uint32_t kEcInitCooldownTicks  = 30; // Min 60s between EC re-init checks
    static constexpr uint64_t kEcmbPhysicalAddress  = 0xFE802000ULL;
    static constexpr uint64_t kEcmbMappingSize      = 0x200ULL;

    // Intel Architectural & Package MSRs
    static constexpr uint32_t MSR_IA32_THERM_STATUS         = 0x19C;
    static constexpr uint32_t MSR_TEMPERATURE_TARGET        = 0x1A2;
    static constexpr uint32_t MSR_IA32_PACKAGE_THERM_STATUS = 0x1B1;
    static constexpr uint32_t MSR_RAPL_POWER_UNIT           = 0x606;
    static constexpr uint32_t MSR_PKG_ENERGY_STATUS         = 0x611;
    static constexpr uint32_t MSR_IA32_HWP_REQUEST          = 0x774;

    // Lenovo EC0 EmbeddedControl offsets (fallback only when SSDT-THINKFLOW is absent)
    static constexpr uint32_t EC_OFFSET_LXOS = 0x58;
    static constexpr uint32_t EC_OFFSET_FANS = 0x95;
    static constexpr uint32_t EC_OFFSET_PFMM = 0xC5; // bits [2:1]
    static constexpr uint32_t EC_OFFSET_ITSM = 0xC6; // bits [5:3]
    static constexpr uint32_t EC_OFFSET_TPA1 = 0xCF;

    // Lenovo ECMB MMIO offsets (0xFE802000 + offset)
    static constexpr uint32_t ECMB_OFFSET_THS0 = 0x20;
    static constexpr uint32_t ECMB_OFFSET_THS1 = 0x21;

    static void timerCallback(OSObject *owner, IOTimerEventSource *sender);
    static IOReturn setPolicyGated(OSObject *owner, void *arg0, void *arg1, void *arg2, void *arg3);
    static IOReturn handleSleepGated(OSObject *owner, void *arg0, void *arg1, void *arg2, void *arg3);
    static IOReturn handleWakeGated(OSObject *owner, void *arg0, void *arg1, void *arg2, void *arg3);
    static IOReturn systemPowerCallback(void *target, void *refCon,
                                        UInt32 messageType, IOService *provider,
                                        void *messageArgument, vm_size_t argSize);

    void initHardware();
    void prepareForSleep();
    void pollTick();
    bool evaluateDytc(uint32_t cmd, uint32_t *outResult = nullptr);
    bool readEcByte(uint32_t offset, uint8_t *outVal);
    bool writeEcByte(uint32_t offset, uint8_t val);
    void sampleTelemetry(ThinkFlowTelemetryInput *input);
    void publishProperties(const ThinkFlowTelemetryInput &input,
                           const ThinkFlowActionOutput &output);

public:
    virtual bool init(OSDictionary *dictionary = nullptr) APPLE_KEXT_OVERRIDE;
    virtual IOService *probe(IOService *provider, SInt32 *score) APPLE_KEXT_OVERRIDE;
    virtual bool start(IOService *provider) APPLE_KEXT_OVERRIDE;
    virtual void stop(IOService *provider) APPLE_KEXT_OVERRIDE;
    virtual void free() APPLE_KEXT_OVERRIDE;
    virtual IOReturn setPowerState(unsigned long powerStateOrdinal,
                                   IOService *whatDevice) APPLE_KEXT_OVERRIDE;
    virtual IOReturn setProperties(OSObject *properties) APPLE_KEXT_OVERRIDE;
};

#endif // THINKFLOW_HPP
