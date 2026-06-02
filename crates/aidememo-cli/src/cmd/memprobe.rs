//! OS-native physical memory probes for `aidememo bench`.
//!
//! `memory-stats` reports RSS (`physical_mem`), but on Darwin/Linux RSS
//! includes large file-backed mmap regions — Model2Vec weights, simsimd
//! shared libs, redb's mmap. For workloads that mmap multi-GB artifacts
//! this overstates real memory by 10×+.
//!
//! These helpers expose two narrower views:
//! - **phys_footprint** (macOS): the kernel's "real memory pressure" number,
//!   what Activity Monitor calls "Memory" — anonymous + compressed + swapped,
//!   excluding shared file-backed pages.
//! - **rss_anon** (Linux): just anonymous resident memory from
//!   `/proc/self/status:RssAnon`, similarly excluding mmap'd files.
//!
//! On unsupported OSes both return `None`; callers should fall back to
//! plain RSS.

#[cfg(target_os = "macos")]
pub fn phys_footprint_bytes() -> Option<u64> {
    use mach2::mach_types::task_t;
    use mach2::message::mach_msg_type_number_t;
    use mach2::traps::mach_task_self;
    use std::mem::MaybeUninit;

    // task_vm_info struct laid out manually — mach2 doesn't expose
    // the full struct fields by name across versions. Layout matches
    // <mach/task_info.h> as of macOS 14.
    #[repr(C)]
    #[allow(non_snake_case)]
    struct TaskVmInfo {
        virtual_size: u64,
        region_count: u32,
        page_size: u32,
        resident_size: u64,
        resident_size_peak: u64,
        device: u64,
        device_peak: u64,
        internal: u64,
        internal_peak: u64,
        external: u64,
        external_peak: u64,
        reusable: u64,
        reusable_peak: u64,
        purgeable_volatile_pmap: u64,
        purgeable_volatile_resident: u64,
        purgeable_volatile_virtual: u64,
        compressed: u64,
        compressed_peak: u64,
        compressed_lifetime: u64,
        phys_footprint: u64,
        // Newer fields exist past this point; we only need phys_footprint.
        _padding: [u64; 16],
    }

    unsafe extern "C" {
        fn task_info(
            target_task: task_t,
            flavor: u32,
            task_info_out: *mut i32,
            task_info_count: *mut mach_msg_type_number_t,
        ) -> i32;
    }

    const TASK_VM_INFO: u32 = 22;
    let count_words = std::mem::size_of::<TaskVmInfo>() / std::mem::size_of::<i32>();
    let mut count = count_words as mach_msg_type_number_t;
    let mut info = MaybeUninit::<TaskVmInfo>::zeroed();

    let kr = unsafe {
        task_info(
            mach_task_self(),
            TASK_VM_INFO,
            info.as_mut_ptr() as *mut i32,
            &mut count,
        )
    };
    if kr != 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some(info.phys_footprint)
}

#[cfg(target_os = "linux")]
pub fn rss_anon_bytes() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("RssAnon:") {
            // Format: "RssAnon:    12345 kB"
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

// Cross-OS façade. Each platform fills in whichever metric it can; the
// other returns None.

#[cfg(target_os = "macos")]
pub fn os_native_bytes() -> Option<u64> {
    phys_footprint_bytes()
}

#[cfg(target_os = "linux")]
pub fn os_native_bytes() -> Option<u64> {
    rss_anon_bytes()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn os_native_bytes() -> Option<u64> {
    None
}

/// One-line label describing what `os_native_bytes` is on the current
/// host — for human-readable bench output.
pub fn os_native_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "phys_footprint"
    }
    #[cfg(target_os = "linux")]
    {
        "rss_anon"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "unsupported"
    }
}
