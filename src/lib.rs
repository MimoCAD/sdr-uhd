//! A USRP B2xx (B200, B210, B205mini, B206mini) driver: a thin safe
//! wrapper over libuhd's C API (UHD, the USRP Hardware Driver; USRP, the
//! Universal Software Radio Peripheral). [`Usrp`] opens and configures a
//! radio; [`UsrpStream`] receives sc16 samples ([`sdr_core::Iq`]) with
//! device timestamps; [`UsrpTx`] transmits them. [`find`] lists attached
//! radios without opening any; [`link_speed`] reads the negotiated USB
//! speed from the OS.
//!
//! libuhd is found at build time under `$SDRPREFIX` (default
//! `~/.sdr/prefix`); README.md builds it from source, with the firmware and
//! FPGA images it loads.

mod ffi;
pub mod link_speed;
mod stream;
mod tx;
mod usrp;

pub use stream::{StreamCommand, UsrpStream};
pub use tx::UsrpTx;
pub use usrp::{MboardInfo, Usrp};

/// The backend name every error carries.
pub const BACKEND: &str = "uhd";

use std::ffi::{CStr, c_char};

/// Read the NUL-terminated prefix of `buf` as a lossily-decoded String.
pub(crate) fn cstr_lossy(buf: &[u8]) -> String {
    CStr::from_bytes_until_nul(buf)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Every string-out function in the C API has the shape
/// `f(buf, len) -> uhd_error`; drive one of them into a String.
pub(crate) fn fetch_string(f: impl FnOnce(*mut c_char, usize) -> ffi::uhd_error) -> String {
    let mut buf = [0u8; 1024];
    let _ = f(buf.as_mut_ptr().cast(), buf.len());
    cstr_lossy(&buf)
}

/// Message for failures with no live handle to ask (`uhd_usrp_make` itself,
/// `uhd_rx_streamer_make`, `uhd_rx_metadata_make`).
pub(crate) fn global_last_error() -> String {
    fetch_string(|p, n| unsafe { ffi::uhd_get_last_error(p, n) })
}

/// Enumerate connected USRPs WITHOUT opening any (`uhd_usrp_find`) —
/// each entry is an args string `uhd_usrp_make` accepts back verbatim
/// (`type=b200,serial=…,product=…`). Failures degrade to an empty list:
/// detection is advisory, and its callers fall back to trying an open,
/// whose error reporting is the good one.
pub fn find() -> Vec<String> {
    let mut vec: ffi::uhd_string_vector_handle = std::ptr::null_mut();
    let mut out = Vec::new();
    unsafe {
        if ffi::uhd_string_vector_make(&mut vec) != ffi::uhd_error_UHD_ERROR_NONE {
            return out;
        }
        if ffi::uhd_usrp_find(c"".as_ptr(), &mut vec) == ffi::uhd_error_UHD_ERROR_NONE {
            let mut n = 0usize;
            if ffi::uhd_string_vector_size(vec, &mut n) == ffi::uhd_error_UHD_ERROR_NONE {
                for i in 0..n {
                    out.push(fetch_string(|p, len| ffi::uhd_string_vector_at(vec, i, p, len)));
                }
            }
        }
        ffi::uhd_string_vector_free(&mut vec);
    }
    out
}

#[cfg(test)]
mod tests {
    /// Diagnostic, not a correctness test: does UHD's realtime elevation
    /// actually succeed on this system? Needs libuhd but no radio. Run with
    /// `cargo test --release thread_priority_probe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn thread_priority_probe() {
        let e = unsafe { crate::ffi::uhd_set_thread_priority(0.75, true) };
        println!("uhd_set_thread_priority(0.75, true) -> {e}");
        assert_eq!(e, 0, "UHD could not elevate thread priority");
    }
}
