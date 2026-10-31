use std::ffi::{CString, c_void};
use std::ptr;
use std::sync::Arc;

use crate::ffi;
use crate::usrp::UsrpInner;
use crate::{fetch_string, global_last_error};
use sdr_core::{Error, Iq, Result};

/// A single-channel TX streamer taking sc16 ([`Iq`]) on the host side, for
/// bench signals. `send` queues samples (blocking while UHD's buffers are
/// full, which paces the caller at the sample rate); `end_burst` ends the
/// transmission. Dropping it ends the burst and frees its handles; the
/// device handle it was built from is kept alive until then.
pub struct UsrpTx {
    handle: ffi::uhd_tx_streamer_handle,
    /// Metadata for every send in the burst (no time spec, no flags).
    md: ffi::uhd_tx_metadata_handle,
    max_num_samps: usize,
    /// A send has gone out since the last `end_burst`.
    in_burst: bool,
    // Backing storage for the pointers handed to uhd_stream_args_t.
    _cpu: CString,
    _otw: CString,
    _args: CString,
    // Keep-alive only: uhd_usrp_free runs after this streamer is freed.
    _usrp: Arc<UsrpInner>,
}

// One thread drives it at a time (`send` takes `&mut self`).
unsafe impl Send for UsrpTx {}

impl UsrpTx {
    pub(crate) fn new(usrp: Arc<UsrpInner>, chan: usize) -> Result<UsrpTx> {
        let mut handle: ffi::uhd_tx_streamer_handle = ptr::null_mut();
        let e = unsafe { ffi::uhd_tx_streamer_make(&mut handle) };
        if e != ffi::uhd_error_UHD_ERROR_NONE || handle.is_null() {
            return Err(Error::Uhd {
                code: e,
                context: "uhd_tx_streamer_make",
                message: global_last_error(),
            });
        }
        // From here the partly built streamer owns the handle: any `?`
        // below runs Drop, which null-checks md and frees what exists.
        let mut s = UsrpTx {
            handle,
            md: ptr::null_mut(),
            max_num_samps: 0,
            in_burst: false,
            _cpu: c"sc16".into(),
            _otw: c"sc16".into(),
            _args: c"".into(),
            _usrp: usrp,
        };
        let mut chan_list = chan; // only read during the call below
        let mut sargs = ffi::uhd_stream_args_t {
            cpu_format: s._cpu.as_ptr() as *mut _,
            otw_format: s._otw.as_ptr() as *mut _,
            args: s._args.as_ptr() as *mut _,
            channel_list: &mut chan_list,
            n_channels: 1,
        };
        let e = unsafe { ffi::uhd_usrp_get_tx_stream(s._usrp.0, &mut sargs, s.handle) };
        if e != ffi::uhd_error_UHD_ERROR_NONE {
            let h = s._usrp.0;
            return Err(Error::Uhd {
                code: e,
                context: "uhd_usrp_get_tx_stream",
                message: fetch_string(|p, n| unsafe { ffi::uhd_usrp_last_error(h, p, n) }),
            });
        }
        let e = unsafe { ffi::uhd_tx_streamer_max_num_samps(s.handle, &mut s.max_num_samps) };
        s.check(e, "uhd_tx_streamer_max_num_samps")?;
        let e = unsafe { ffi::uhd_tx_metadata_make(&mut s.md, false, 0, 0.0, false, false) };
        if e != ffi::uhd_error_UHD_ERROR_NONE {
            return Err(Error::Uhd {
                code: e,
                context: "uhd_tx_metadata_make",
                message: global_last_error(),
            });
        }
        Ok(s)
    }

    fn check(&self, e: ffi::uhd_error, context: &'static str) -> Result<()> {
        if e == ffi::uhd_error_UHD_ERROR_NONE {
            Ok(())
        } else {
            let h = self.handle;
            Err(Error::Uhd {
                code: e,
                context,
                message: fetch_string(|p, n| unsafe { ffi::uhd_tx_streamer_last_error(h, p, n) }),
            })
        }
    }

    /// The most samples one send passes to UHD at a time.
    pub fn max_num_samps(&self) -> usize {
        self.max_num_samps
    }

    /// Queue `iq` for transmission, all of it: blocks while UHD's buffers
    /// are full (at most `timeout_secs` per piece).
    pub fn send(&mut self, iq: &[Iq], timeout_secs: f64) -> Result<()> {
        let mut at = 0;
        while at < iq.len() {
            let n = (iq.len() - at).min(self.max_num_samps);
            let buf: *const c_void = iq[at..].as_ptr().cast();
            let mut buffs = [buf];
            let mut sent = 0usize;
            let e = unsafe {
                ffi::uhd_tx_streamer_send(
                    self.handle,
                    buffs.as_mut_ptr(),
                    n,
                    &mut self.md,
                    timeout_secs,
                    &mut sent,
                )
            };
            self.check(e, "uhd_tx_streamer_send")?;
            if sent == 0 {
                return Err(Error::Device {
                    backend: "uhd",
                    context: "uhd_tx_streamer_send",
                    message: "no samples taken within the timeout".into(),
                });
            }
            self.in_burst = true;
            at += sent;
        }
        Ok(())
    }

    /// One burst sent whole: marked start-of-burst and end-of-burst, at
    /// device time `at` seconds when given (else now).
    pub fn send_burst(&mut self, iq: &[Iq], at: Option<f64>, timeout_secs: f64) -> Result<()> {
        let mut md: ffi::uhd_tx_metadata_handle = ptr::null_mut();
        let (has_time, full, frac) =
            at.map_or((false, 0, 0.0), |t| (true, t.floor() as i64, t.fract()));
        let e = unsafe { ffi::uhd_tx_metadata_make(&mut md, has_time, full, frac, true, true) };
        if e != ffi::uhd_error_UHD_ERROR_NONE {
            return Err(Error::Uhd {
                code: e,
                context: "uhd_tx_metadata_make",
                message: global_last_error(),
            });
        }
        let mut buffs: [*const c_void; 1] = [iq.as_ptr().cast()];
        let mut sent = 0usize;
        let e = unsafe {
            ffi::uhd_tx_streamer_send(
                self.handle,
                buffs.as_mut_ptr(),
                iq.len(),
                &mut md,
                timeout_secs,
                &mut sent,
            )
        };
        unsafe { ffi::uhd_tx_metadata_free(&mut md) };
        self.check(e, "uhd_tx_streamer_send (burst)")
    }

    /// The transmit events the radio reported (burst acknowledged,
    /// underflow, sequence or time errors), waiting at most `timeout_secs`
    /// for the first: UHD's event codes.
    pub fn async_events(&mut self, timeout_secs: f64) -> Result<Vec<u32>> {
        let mut md: ffi::uhd_async_metadata_handle = ptr::null_mut();
        let e = unsafe { ffi::uhd_async_metadata_make(&mut md) };
        if e != ffi::uhd_error_UHD_ERROR_NONE {
            return Err(Error::Uhd {
                code: e,
                context: "uhd_async_metadata_make",
                message: global_last_error(),
            });
        }
        let mut out = Vec::new();
        let mut wait = timeout_secs;
        loop {
            let mut valid = false;
            let e = unsafe {
                ffi::uhd_tx_streamer_recv_async_msg(self.handle, &mut md, wait, &mut valid)
            };
            if e != ffi::uhd_error_UHD_ERROR_NONE || !valid {
                break;
            }
            let mut code: ffi::uhd_async_metadata_event_code_t = 0;
            unsafe { ffi::uhd_async_metadata_event_code(md, &mut code) };
            out.push(code);
            wait = 0.0;
        }
        unsafe { ffi::uhd_async_metadata_free(&mut md) };
        Ok(out)
    }

    /// End the transmission: an empty send marked end-of-burst.
    pub fn end_burst(&mut self) -> Result<()> {
        if !self.in_burst {
            return Ok(());
        }
        let mut eob: ffi::uhd_tx_metadata_handle = ptr::null_mut();
        let e = unsafe { ffi::uhd_tx_metadata_make(&mut eob, false, 0, 0.0, false, true) };
        if e != ffi::uhd_error_UHD_ERROR_NONE {
            return Err(Error::Uhd {
                code: e,
                context: "uhd_tx_metadata_make",
                message: global_last_error(),
            });
        }
        let zero = [Iq::default()];
        let mut buffs: [*const c_void; 1] = [zero.as_ptr().cast()];
        let mut sent = 0usize;
        let e = unsafe {
            ffi::uhd_tx_streamer_send(self.handle, buffs.as_mut_ptr(), 0, &mut eob, 1.0, &mut sent)
        };
        unsafe { ffi::uhd_tx_metadata_free(&mut eob) };
        self.in_burst = false;
        self.check(e, "uhd_tx_streamer_send (end of burst)")
    }
}

impl Drop for UsrpTx {
    fn drop(&mut self) {
        let _ = self.end_burst();
        unsafe {
            if !self.md.is_null() {
                ffi::uhd_tx_metadata_free(&mut self.md);
            }
            ffi::uhd_tx_streamer_free(&mut self.handle);
        }
    }
}
