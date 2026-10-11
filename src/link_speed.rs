//! Negotiated USB link speed for the attached B2xx — the sc12 policy's
//! one input.
//!
//! WHY THIS EXISTS: sc16 at 4.8 Msps is 19.2 MB/s, ~55% of a practical
//! USB (Universal Serial Bus) 2.0 bulk link (~30–40 MB/s of the
//! 53 MB/s protocol ceiling), which leaves only ~1.8× catch-up
//! bandwidth after a host stall — and headroom is RECOVERY margin, not
//! average margin, which is why sc12 (14.4 MB/s) helps a USB 2.0 link's
//! overflows where thread priority and buffering do not. The AD9364
//! ADC (analog-to-digital converter) is 12-bit anyway, so sc12 loses
//! nothing real. On USB 3 (5%% utilisation, ~20× catch-up) sc16 stays
//! the default: bit-exact transport and comparable manifests.
//!
//! WHY DETECTION IS DONE HERE AND NOT ASKED OF LIBUHD: the B200 driver
//! knows the speed — `b200_impl.cpp` logs "Operating over USB N." and
//! publishes `usb_version` into its C++ property tree — but the UHD
//! C API exposes no property-tree access, no log subscription, and no
//! sensor for it (checked against UHD 4.10, uhd/usrp/usrp.h and
//! uhd/utils/log.h). So the negotiated speed is read from the OS the
//! same way the OS told libuhd: sysfs on Linux, the IOKit (I/O Kit,
//! Apple's driver framework) registry on macOS. Enumeration only —
//! nothing is opened or claimed, so it cannot disturb libuhd's claim
//! on the device.
//!
//! The IOKit FFI (foreign function interface) below is hand-rolled:
//! zero new crates, and the call surface is tiny (7 IOKit/CF calls).

/// Ettus Research's USB vendor id. Every B2xx re-enumerates under it
/// once the FX3 firmware is up — and our caller runs after
/// `Device::open`, i.e. after libuhd loaded the firmware, so the
/// pre-firmware Cypress id (0x04B4) can no longer be on the bus for
/// this device.
const ETTUS_VID: u16 = 0x2500;

/// USB 2.0-class or worse: at or below High Speed's nominal 480 Mbps.
/// The sc12 policy keys on this — see the module header for the
/// recovery-margin arithmetic.
pub fn is_usb2_class(mbps: f64) -> bool {
    mbps <= 480.0
}

/// The lowest negotiated speed (nominal Mbps) among attached Ettus
/// devices, `None` when none are visible (or on an OS this module
/// doesn't know how to ask). Lowest, because with several radios the
/// conservative answer is the one that protects the worst link.
pub fn ettus_negotiated_mbps() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        linux_min_mbps_for_vid(std::path::Path::new("/sys/bus/usb/devices"), ETTUS_VID)
    }
    #[cfg(target_os = "macos")]
    {
        macos_min_mbps_for_vid(ETTUS_VID)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Linux: every device under /sys/bus/usb/devices carries `idVendor`
/// (4 hex digits) and `speed` (nominal Mbps as text: "1.5", "12",
/// "480", "5000", "10000", "20000"). Interface entries ("1-1:1.0")
/// have neither file and fall out of the filter naturally. The root
/// is a parameter so the parser is testable against a fabricated tree
/// on any OS.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_min_mbps_for_vid(root: &std::path::Path, vid: u16) -> Option<f64> {
    let want = format!("{vid:04x}");
    let mut min: Option<f64> = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let dir = entry.path();
        let vendor = match std::fs::read_to_string(dir.join("idVendor")) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !vendor.trim().eq_ignore_ascii_case(&want) {
            continue;
        }
        if let Ok(s) = std::fs::read_to_string(dir.join("speed"))
            && let Ok(mbps) = s.trim().parse::<f64>()
        {
            min = Some(min.map_or(mbps, |m: f64| m.min(mbps)));
        }
    }
    min
}

/// macOS: the IOKit registry's `IOUSBHostDevice` entries carry
/// `idVendor` and `USBSpeed` as CFNumbers. `USBSpeed` is Apple's
/// connection-speed code, verified against a real registry (macOS 26):
/// every USB 2.0 hub read 3,
/// a SuperSpeed storage device read 4, the 10 Gbps hubs read 5 —
/// matching IOUSBHostFamily's tUSBHostConnectionSpeed (0 none, 1 low,
/// 2 full, 3 high, 4 super, 5 super+, 6 super+ ×2).
#[cfg(target_os = "macos")]
fn speed_code_mbps(code: i32) -> f64 {
    match code {
        0 | 1 => 1.5,
        2 => 12.0,
        3 => 480.0,
        4 => 5_000.0,
        5 => 10_000.0,
        6 => 20_000.0,
        // A future faster code must never downgrade the wire format:
        // unknown reads as fast, and the sc16 default stands.
        _ => f64::INFINITY,
    }
}

#[cfg(target_os = "macos")]
mod iokit {
    //! The 7-call IOKit/CoreFoundation surface. io_object_t and
    //! io_iterator_t are mach ports (u32); CF handles are opaque
    //! pointers. Reference rules that matter here, each from the
    //! Apple docs: IOServiceGetMatchingServices CONSUMES the matching
    //! dictionary (never release it, even on failure);
    //! IORegistryEntryCreateCFProperty and CFStringCreateWithCString
    //! return +1 references the caller must CFRelease; iterator and
    //! device handles release through IOObjectRelease.
    use std::ffi::{c_char, c_void};

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        pub fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        pub fn IOServiceGetMatchingServices(
            main_port: u32,
            matching: *mut c_void,
            existing: *mut u32,
        ) -> i32;
        pub fn IOIteratorNext(iterator: u32) -> u32;
        pub fn IOObjectRelease(object: u32) -> i32;
        pub fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: *const c_void,
            allocator: *const c_void,
            options: u32,
        ) -> *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        pub fn CFGetTypeID(cf: *const c_void) -> usize;
        pub fn CFNumberGetTypeID() -> usize;
        pub fn CFNumberGetValue(
            number: *const c_void,
            the_type: isize,
            value_ptr: *mut c_void,
        ) -> bool;
        pub fn CFRelease(cf: *const c_void);
    }

    /// kCFStringEncodingUTF8, CFString.h.
    pub const UTF8: u32 = 0x0800_0100;
    /// kCFNumberSInt32Type, CFNumber.h.
    pub const SINT32: isize = 3;
}

/// One CFNumber property of a registry entry, as i32. Type-checked
/// before the read: CFNumberGetValue on a non-number is undefined
/// behavior, and a registry key changing type must read as "absent",
/// never as a crash.
#[cfg(target_os = "macos")]
fn cf_number_i32(entry: u32, key: &std::ffi::CStr) -> Option<i32> {
    use iokit::*;
    unsafe {
        let k = CFStringCreateWithCString(std::ptr::null(), key.as_ptr(), UTF8);
        if k.is_null() {
            return None;
        }
        let v = IORegistryEntryCreateCFProperty(entry, k, std::ptr::null(), 0);
        CFRelease(k);
        if v.is_null() {
            return None;
        }
        let mut out: i32 = 0;
        let ok = CFGetTypeID(v) == CFNumberGetTypeID()
            && CFNumberGetValue(v, SINT32, &mut out as *mut i32 as *mut std::ffi::c_void);
        CFRelease(v);
        ok.then_some(out)
    }
}

#[cfg(target_os = "macos")]
fn macos_min_mbps_for_vid(vid: u16) -> Option<f64> {
    use iokit::*;
    unsafe {
        let matching = IOServiceMatching(c"IOUSBHostDevice".as_ptr());
        if matching.is_null() {
            return None;
        }
        let mut iter: u32 = 0;
        // Port 0 = kIOMainPortDefault. The call consumes `matching`
        // on success AND failure — see the iokit module header.
        if IOServiceGetMatchingServices(0, matching, &mut iter) != 0 {
            return None;
        }
        let mut min: Option<f64> = None;
        loop {
            let dev = IOIteratorNext(iter);
            if dev == 0 {
                break;
            }
            if cf_number_i32(dev, c"idVendor") == Some(vid as i32)
                && let Some(code) = cf_number_i32(dev, c"USBSpeed")
            {
                let mbps = speed_code_mbps(code);
                min = Some(min.map_or(mbps, |m: f64| m.min(mbps)));
            }
            IOObjectRelease(dev);
        }
        IOObjectRelease(iter);
        min
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The policy boundary: High Speed (480) and below take sc12;
    /// SuperSpeed and up keep sc16.
    #[test]
    fn usb2_class_boundary_sits_at_high_speed() {
        assert!(is_usb2_class(1.5));
        assert!(is_usb2_class(12.0));
        assert!(is_usb2_class(480.0));
        assert!(!is_usb2_class(5_000.0));
        assert!(!is_usb2_class(10_000.0));
    }

    /// The sysfs parser against a fabricated tree: an Ettus device at
    /// 480, another at 5000 (lowest wins), a foreign device, and an
    /// interface-style entry with no idVendor at all.
    #[test]
    fn sysfs_parser_takes_the_lowest_ettus_speed() {
        let root = std::env::temp_dir().join(format!("linkspeed-test-{}", std::process::id()));
        let mk = |name: &str, vendor: Option<&str>, speed: Option<&str>| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            if let Some(v) = vendor {
                std::fs::write(d.join("idVendor"), format!("{v}\n")).unwrap();
            }
            if let Some(s) = speed {
                std::fs::write(d.join("speed"), format!("{s}\n")).unwrap();
            }
        };
        mk("1-1", Some("2500"), Some("480"));
        mk("2-1", Some("2500"), Some("5000"));
        mk("1-2", Some("0bda"), Some("480"));
        mk("1-1:1.0", None, None);
        assert_eq!(linux_min_mbps_for_vid(&root, 0x2500), Some(480.0));
        assert_eq!(linux_min_mbps_for_vid(&root, 0x1d50), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// macOS: sweep the REAL registry for a vendor id nobody owns.
    /// Must come back None without crashing — this exercises the whole
    /// FFI path (matching, iteration, typed property reads, releases)
    /// on every Mac the tests run on.
    #[cfg(target_os = "macos")]
    #[test]
    fn absent_vendor_scans_clean_on_the_real_registry() {
        assert_eq!(macos_min_mbps_for_vid(0xFFFF), None);
    }

    /// The speed-code table, pinned to the registry observation
    /// (module doc): 3 = the USB 2.0 hubs, 4 = SuperSpeed, 5 = 10 G.
    #[cfg(target_os = "macos")]
    #[test]
    fn speed_codes_match_the_observed_registry() {
        assert_eq!(speed_code_mbps(3), 480.0);
        assert_eq!(speed_code_mbps(4), 5_000.0);
        assert_eq!(speed_code_mbps(5), 10_000.0);
        assert!(!is_usb2_class(speed_code_mbps(99)), "unknown reads fast");
    }
}
