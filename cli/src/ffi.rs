use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;

// ============================================================================
// CoreFoundation & IOKit / libIOReport FFI Bindings
// ============================================================================

pub(crate) type CFAllocatorRef = *const c_void;
pub(crate) type CFStringRef = *const c_void;
pub(crate) type CFArrayRef = *const c_void;
pub(crate) type CFDataRef = *const c_void;
pub(crate) type CFDictionaryRef = *const c_void;
pub(crate) type CFMutableDictionaryRef = *mut c_void;
pub(crate) type CFNumberRef = *const c_void;
pub(crate) type CFTypeRef = *const c_void;
pub(crate) type CFIndex = isize;
pub(crate) type CFStringEncoding = u32;

pub(crate) const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;
pub(crate) const K_CF_NUMBER_SINT32_TYPE: CFIndex = 3;
pub(crate) const K_CF_NUMBER_SINT64_TYPE: CFIndex = 4;

pub(crate) type MachPortT = u32;
pub(crate) type IoObjectT = u32;
pub(crate) type IoRegistryEntryT = IoObjectT;
pub(crate) type IoServiceT = IoObjectT;
pub(crate) type KernReturnT = c_int;
pub(crate) type NaturalT = u32;
pub(crate) type ProcessorFlavorT = c_int;
pub(crate) type ProcessorInfoArrayT = *mut c_int;
pub(crate) type MachMsgTypeNumberT = u32;

pub(crate) const PROCESSOR_CPU_LOAD_INFO: ProcessorFlavorT = 2;
pub(crate) const CPU_STATE_USER: usize = 0;
pub(crate) const CPU_STATE_SYSTEM: usize = 1;
pub(crate) const CPU_STATE_IDLE: usize = 2;
pub(crate) const CPU_STATE_NICE: usize = 3;
pub(crate) const CPU_STATE_MAX: usize = 4;

pub(crate) type IOReportSubscriptionRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub(crate) static kCFAllocatorDefault: CFAllocatorRef;

    pub(crate) fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: CFStringEncoding,
    ) -> CFStringRef;
    pub(crate) fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: CFStringEncoding,
    ) -> bool;
    pub(crate) fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type: CFIndex,
        value_ptr: *const c_void,
    ) -> CFNumberRef;
    pub(crate) fn CFNumberGetValue(
        number: CFNumberRef,
        the_type: CFIndex,
        value_ptr: *mut c_void,
    ) -> bool;
    pub(crate) fn CFDictionaryGetValue(
        the_dict: CFDictionaryRef,
        key: *const c_void,
    ) -> *const c_void;
    pub(crate) fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    pub(crate) fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
    pub(crate) fn CFDataGetLength(the_data: CFDataRef) -> CFIndex;
    pub(crate) fn CFDataGetBytePtr(the_data: CFDataRef) -> *const u8;
    pub(crate) fn CFGetTypeID(cf: CFTypeRef) -> usize;
    pub(crate) fn CFNumberGetTypeID() -> usize;
    pub(crate) fn CFStringGetTypeID() -> usize;
    pub(crate) fn CFBooleanGetTypeID() -> usize;
    pub(crate) fn CFBooleanGetValue(boolean: CFTypeRef) -> bool;
    pub(crate) fn CFRelease(cf: CFTypeRef);
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    pub(crate) fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
    pub(crate) fn IOServiceGetMatchingService(
        main_port: MachPortT,
        matching: CFDictionaryRef,
    ) -> IoServiceT;
    pub(crate) fn IORegistryEntryFromPath(
        main_port: MachPortT,
        path: *const c_char,
    ) -> IoRegistryEntryT;
    pub(crate) fn IORegistryEntryCreateCFProperties(
        entry: IoRegistryEntryT,
        properties: *mut CFMutableDictionaryRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> KernReturnT;
    pub(crate) fn IORegistryEntrySetCFProperty(
        entry: IoRegistryEntryT,
        property_name: CFStringRef,
        property: CFTypeRef,
    ) -> KernReturnT;
    pub(crate) fn IORegistryEntryGetChildIterator(
        entry: IoRegistryEntryT,
        plane: *const c_char,
        iterator: *mut IoObjectT,
    ) -> KernReturnT;
    pub(crate) fn IORegistryEntryGetLocationInPlane(
        entry: IoRegistryEntryT,
        plane: *const c_char,
        location: *mut c_char,
    ) -> KernReturnT;
    pub(crate) fn IOIteratorNext(iterator: IoObjectT) -> IoObjectT;
    pub(crate) fn IOObjectRelease(object: IoObjectT) -> KernReturnT;
}

#[link(name = "IOReport")]
extern "C" {
    pub(crate) fn IOReportCopyChannelsInGroup(
        group: CFStringRef,
        subgroup: CFStringRef,
        a: u64,
        b: u64,
        c: u64,
    ) -> CFMutableDictionaryRef;
    pub(crate) fn IOReportCreateSubscription(
        a: *const c_void,
        channels: CFMutableDictionaryRef,
        subbed_channels: *mut CFMutableDictionaryRef,
        channel_id: u64,
        b: CFTypeRef,
    ) -> IOReportSubscriptionRef;
    pub(crate) fn IOReportCreateSamples(
        sub: IOReportSubscriptionRef,
        subbed_channels: CFMutableDictionaryRef,
        a: CFTypeRef,
    ) -> CFDictionaryRef;
}

extern "C" {
    pub(crate) static mach_task_self_: MachPortT;

    pub(crate) fn mach_host_self() -> MachPortT;
    pub(crate) fn mach_port_deallocate(task: MachPortT, name: MachPortT) -> KernReturnT;
    pub(crate) fn host_processor_info(
        host: MachPortT,
        flavor: ProcessorFlavorT,
        out_processor_count: *mut NaturalT,
        out_processor_info: *mut ProcessorInfoArrayT,
        out_processor_info_cnt: *mut MachMsgTypeNumberT,
    ) -> KernReturnT;
    pub(crate) fn vm_deallocate(
        target_task: MachPortT,
        address: usize,
        size: usize,
    ) -> KernReturnT;

    pub(crate) fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *const c_void,
        newlen: usize,
    ) -> c_int;
}

// ============================================================================
// Helper Wrappers for CoreFoundation / sysctl / IOKit
// ============================================================================

pub(crate) fn cfstr(s: &str) -> CFStringRef {
    let c = CString::new(s).unwrap();
    unsafe { CFStringCreateWithCString(kCFAllocatorDefault, c.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
}

pub(crate) fn cfstr_to_string(cf: CFStringRef) -> Option<String> {
    if cf.is_null() {
        return None;
    }
    let mut buf = [0u8; 256];
    let ok = unsafe {
        CFStringGetCString(
            cf,
            buf.as_mut_ptr() as *mut c_char,
            buf.len() as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if !ok {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) };
    Some(cstr.to_string_lossy().into_owned())
}

pub(crate) fn read_sysctl_i32(name: &str) -> Option<i32> {
    let cname = CString::new(name).ok()?;
    let mut val: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    let ret = unsafe {
        sysctlbyname(
            cname.as_ptr(),
            &mut val as *mut i32 as *mut c_void,
            &mut len,
            ptr::null(),
            0,
        )
    };
    if ret == 0 {
        Some(val)
    } else {
        None
    }
}

pub(crate) fn read_sysctl_u64(name: &str) -> Option<u64> {
    let cname = CString::new(name).ok()?;
    let mut val: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let ret = unsafe {
        sysctlbyname(
            cname.as_ptr(),
            &mut val as *mut u64 as *mut c_void,
            &mut len,
            ptr::null(),
            0,
        )
    };
    if ret == 0 {
        Some(val)
    } else {
        None
    }
}

pub(crate) fn read_sysctl_string(name: &str) -> Option<String> {
    let cname = CString::new(name).ok()?;
    let mut buf = [0u8; 256];
    let mut len = buf.len();
    let ret = unsafe {
        sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr() as *mut c_void,
            &mut len,
            ptr::null(),
            0,
        )
    };
    if ret == 0 && len > 0 {
        let s = CStr::from_bytes_until_nul(&buf).ok()?;
        Some(s.to_string_lossy().into_owned())
    } else {
        None
    }
}

pub(crate) fn dict_get_u64(dict: CFDictionaryRef, key: &str) -> Option<u64> {
    let k = cfstr(key);
    let val_ptr = unsafe { CFDictionaryGetValue(dict, k) };
    unsafe { CFRelease(k) };
    if val_ptr.is_null() {
        return None;
    }
    let tid = unsafe { CFGetTypeID(val_ptr) };
    if tid == unsafe { CFNumberGetTypeID() } {
        let mut out: i64 = 0;
        if unsafe {
            CFNumberGetValue(
                val_ptr as CFNumberRef,
                K_CF_NUMBER_SINT64_TYPE,
                &mut out as *mut i64 as *mut c_void,
            )
        } {
            return Some(out as u64);
        }
    } else if tid == unsafe { CFBooleanGetTypeID() } {
        return Some(if unsafe { CFBooleanGetValue(val_ptr) } { 1 } else { 0 });
    }
    None
}

pub(crate) fn dict_get_string(dict: CFDictionaryRef, key: &str) -> Option<String> {
    let k = cfstr(key);
    let val_ptr = unsafe { CFDictionaryGetValue(dict, k) };
    unsafe { CFRelease(k) };
    if val_ptr.is_null() {
        return None;
    }
    let tid = unsafe { CFGetTypeID(val_ptr) };
    if tid == unsafe { CFStringGetTypeID() } {
        return cfstr_to_string(val_ptr as CFStringRef);
    }
    None
}

pub(crate) fn check_acpi_node_exists(path: &str) -> bool {
    let cpath = CString::new(path).unwrap();
    let entry = unsafe { IORegistryEntryFromPath(0, cpath.as_ptr()) };
    if entry != 0 {
        unsafe { IOObjectRelease(entry) };
        true
    } else {
        false
    }
}
