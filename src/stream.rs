use std::ffi::{CString, c_void};
use std::ptr;
use std::sync::Arc;

use sdr_core::Iq;
use sdr_core::{Error, Result};
use crate::ffi;
use sdr_core::IqStream;
use sdr_core::RxSource;
use sdr_core::{OtwFormat, RecvInfo, RxErrorCode, TimeSpec};
use crate::usrp::UsrpInner;
use crate::{fetch_string, global_last_error};

fn otw_cstring(otw: OtwFormat) -> CString {
    CString::from(match otw {
        OtwFormat::Sc16 => c"sc16",
        OtwFormat::Sc12 => c"sc12",
        OtwFormat::Sc8 => c"sc8",
    })
}

/// A single-channel continuous RX streamer in sc16 format. Dropping it stops
/// the stream (if running) and frees its handles; the device handle it was
/// built from is kept alive until then.
pub struct UsrpStream {
    handle: ffi::uhd_rx_streamer_handle,
    md: ffi::uhd_rx_metadata_handle,
    max_num_samps: usize,
    streaming: bool,
    // Backing storage for the pointers handed to uhd_stream_args_t. UHD
    // copies the strings during get_rx_stream, but pinning their lifetime to
    // the streamer removes any doubt about dangling pointers.
    _cpu: CString,
    _otw: CString,
    _args: CString,
    // Keep-alive only: guarantees uhd_usrp_free runs after this streamer is
    // freed. Never used to call uhd_usrp_* functions after construction.
    _usrp: Arc<UsrpInner>,
}

// The streamer moves whole into the async worker thread. recv takes
// `&mut self`, so no Sync — one thread drives it at a time.
unsafe impl Send for UsrpStream {}

impl UsrpStream {
    pub(crate) fn new(
        usrp: Arc<UsrpInner>,
        chan: usize,
        stream_args: &str,
        otw: OtwFormat,
    ) -> Result<UsrpStream> {
        let args = CString::new(stream_args)?;
        let mut handle: ffi::uhd_rx_streamer_handle = ptr::null_mut();
        let e = unsafe { ffi::uhd_rx_streamer_make(&mut handle) };
        if e != ffi::uhd_error_UHD_ERROR_NONE || handle.is_null() {
            return Err(Error::Uhd {
                code: e,
                context: "uhd_rx_streamer_make",
                message: global_last_error(),
            });
        }
        // From here the partially-built streamer owns the handle: any `?`
        // below runs Drop, which null-checks md and frees what exists.
        let mut s = UsrpStream {
            handle,
            md: ptr::null_mut(),
            max_num_samps: 0,
            streaming: false,
            _cpu: c"sc16".into(),
            _otw: otw_cstring(otw),
            _args: args,
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
        let e = unsafe { ffi::uhd_usrp_get_rx_stream(s._usrp.0, &mut sargs, s.handle) };
        if e != ffi::uhd_error_UHD_ERROR_NONE {
            let h = s._usrp.0;
            return Err(Error::Uhd {
                code: e,
                context: "uhd_usrp_get_rx_stream",
                message: fetch_string(|p, n| unsafe { ffi::uhd_usrp_last_error(h, p, n) }),
            });
        }

        let e = unsafe { ffi::uhd_rx_streamer_max_num_samps(s.handle, &mut s.max_num_samps) };
        s.check(e, "uhd_rx_streamer_max_num_samps")?;

        let e = unsafe { ffi::uhd_rx_metadata_make(&mut s.md) };
        if e != ffi::uhd_error_UHD_ERROR_NONE {
            return Err(Error::Uhd {
                code: e,
                context: "uhd_rx_metadata_make",
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
                message: fetch_string(|p, n| unsafe { ffi::uhd_rx_streamer_last_error(h, p, n) }),
            })
        }
    }

    fn md_check(&self, e: ffi::uhd_error, context: &'static str) -> Result<()> {
        if e == ffi::uhd_error_UHD_ERROR_NONE {
            Ok(())
        } else {
            let md = self.md;
            Err(Error::Uhd {
                code: e,
                context,
                message: fetch_string(|p, n| unsafe { ffi::uhd_rx_metadata_last_error(md, p, n) }),
            })
        }
    }

    /// Largest number of samples one `recv_into` call can deliver.
    pub fn max_num_samps(&self) -> usize {
        self.max_num_samps
    }

    pub fn start_continuous(&mut self) -> Result<()> {
        let cmd = stream_cmd(ffi::uhd_stream_mode_t_UHD_STREAM_MODE_START_CONTINUOUS);
        let e = unsafe { ffi::uhd_rx_streamer_issue_stream_cmd(self.handle, &cmd) };
        self.check(e, "start_continuous")?;
        self.streaming = true;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        let cmd = stream_cmd(ffi::uhd_stream_mode_t_UHD_STREAM_MODE_STOP_CONTINUOUS);
        let e = unsafe { ffi::uhd_rx_streamer_issue_stream_cmd(self.handle, &cmd) };
        self.check(e, "stop_continuous")?;
        self.streaming = false;
        Ok(())
    }

    /// Blocking receive into `buf`. Timeout and overflow are non-fatal and
    /// come back as flags on the `RecvInfo`; the four fatal metadata codes
    /// come back as `Error::Rx`.
    pub fn recv_into(&mut self, buf: &mut [Iq], timeout_secs: f64) -> Result<RecvInfo> {
        let mut bufs = [buf.as_mut_ptr() as *mut c_void];
        let mut n: usize = 0;
        let e = unsafe {
            ffi::uhd_rx_streamer_recv(
                self.handle,
                bufs.as_mut_ptr(),
                buf.len(),
                &mut self.md,
                timeout_secs,
                false,
                &mut n,
            )
        };
        self.check(e, "uhd_rx_streamer_recv")?;

        let mut code: ffi::uhd_rx_metadata_error_code_t = 0;
        let e = unsafe { ffi::uhd_rx_metadata_error_code(self.md, &mut code) };
        self.md_check(e, "uhd_rx_metadata_error_code")?;

        let (overflow, timed_out) = match code {
            ffi::uhd_rx_metadata_error_code_t_UHD_RX_METADATA_ERROR_CODE_NONE => (false, false),
            ffi::uhd_rx_metadata_error_code_t_UHD_RX_METADATA_ERROR_CODE_OVERFLOW => (true, false),
            ffi::uhd_rx_metadata_error_code_t_UHD_RX_METADATA_ERROR_CODE_TIMEOUT => (false, true),
            fatal => {
                let code = match fatal {
                    ffi::uhd_rx_metadata_error_code_t_UHD_RX_METADATA_ERROR_CODE_LATE_COMMAND => {
                        RxErrorCode::LateCommand
                    }
                    ffi::uhd_rx_metadata_error_code_t_UHD_RX_METADATA_ERROR_CODE_BROKEN_CHAIN => {
                        RxErrorCode::BrokenChain
                    }
                    ffi::uhd_rx_metadata_error_code_t_UHD_RX_METADATA_ERROR_CODE_ALIGNMENT => {
                        RxErrorCode::Alignment
                    }
                    ffi::uhd_rx_metadata_error_code_t_UHD_RX_METADATA_ERROR_CODE_BAD_PACKET => {
                        RxErrorCode::BadPacket
                    }
                    other => RxErrorCode::Unknown(other),
                };
                let md = self.md;
                return Err(Error::Rx {
                    code,
                    message: fetch_string(|p, len| unsafe {
                        ffi::uhd_rx_metadata_strerror(md, p, len)
                    }),
                });
            }
        };

        let mut out_of_sequence = false;
        let e = unsafe { ffi::uhd_rx_metadata_out_of_sequence(self.md, &mut out_of_sequence) };
        self.md_check(e, "uhd_rx_metadata_out_of_sequence")?;

        let mut more_fragments = false;
        let e = unsafe { ffi::uhd_rx_metadata_more_fragments(self.md, &mut more_fragments) };
        self.md_check(e, "uhd_rx_metadata_more_fragments")?;

        let mut has_time_spec = false;
        let e = unsafe { ffi::uhd_rx_metadata_has_time_spec(self.md, &mut has_time_spec) };
        self.md_check(e, "uhd_rx_metadata_has_time_spec")?;
        let time_spec = if has_time_spec {
            let mut full_secs = 0i64;
            let mut frac_secs = 0f64;
            let e =
                unsafe { ffi::uhd_rx_metadata_time_spec(self.md, &mut full_secs, &mut frac_secs) };
            self.md_check(e, "uhd_rx_metadata_time_spec")?;
            Some(TimeSpec {
                full_secs,
                frac_secs,
            })
        } else {
            None
        };

        Ok(RecvInfo {
            n_samples: n,
            overflow,
            timed_out,
            out_of_sequence,
            more_fragments,
            time_spec,
            link: None,
        })
    }

    /// Start continuous streaming and move this streamer onto a worker
    /// thread; see [`IqStream`].
    pub fn into_stream(self, depth: usize) -> Result<IqStream> {
        IqStream::spawn(self, depth)
    }
}

impl RxSource for UsrpStream {
    fn max_num_samps(&self) -> usize {
        UsrpStream::max_num_samps(self)
    }

    fn start(&mut self) -> Result<()> {
        self.start_continuous()
    }

    fn recv_into(&mut self, buf: &mut [Iq], timeout_secs: f64) -> Result<RecvInfo> {
        UsrpStream::recv_into(self, buf, timeout_secs)
    }

    fn setup_worker_thread(&self) {
        // UHD maps this to SCHED_RR on macOS. Best-effort — but NOT
        // silent: an unprivileged host refuses it (EPERM), and a receive
        // loop left at normal priority loses scheduler races to busy
        // threads at exactly peak load, its symptom device overflows that
        // look like a capacity problem. A refusal is said once, here.
        let rc = unsafe { ffi::uhd_set_thread_priority(0.75, true) };
        if rc != ffi::uhd_error_UHD_ERROR_NONE {
            eprintln!(
                "sdr-uhd: realtime elevation REFUSED (uhd_error {rc}) — the USB receive loop \
                 runs at normal priority and can lose samples under load"
            );
        }
    }
}

impl Drop for UsrpStream {
    fn drop(&mut self) {
        unsafe {
            if self.streaming {
                // Best-effort — never panic in Drop.
                let cmd = stream_cmd(ffi::uhd_stream_mode_t_UHD_STREAM_MODE_STOP_CONTINUOUS);
                let _ = ffi::uhd_rx_streamer_issue_stream_cmd(self.handle, &cmd);
            }
            if !self.md.is_null() {
                let _ = ffi::uhd_rx_metadata_free(&mut self.md);
            }
            let _ = ffi::uhd_rx_streamer_free(&mut self.handle);
            // self._usrp drops after this body, so uhd_usrp_free comes last.
        }
    }
}

/// A receive stream command ([`UsrpStream::issue`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StreamCommand {
    StartContinuous,
    StopContinuous,
    /// This many samples, then the stream ends.
    NumSampsAndDone(usize),
    /// This many samples, then wait for the next command.
    NumSampsAndMore(usize),
}

impl UsrpStream {
    /// Issue a stream command, now or at device time `at` seconds.
    pub fn issue(&mut self, cmd: StreamCommand, at: Option<f64>) -> Result<()> {
        let (mode, n) = match cmd {
            StreamCommand::StartContinuous => (ffi::uhd_stream_mode_t_UHD_STREAM_MODE_START_CONTINUOUS, 0),
            StreamCommand::StopContinuous => (ffi::uhd_stream_mode_t_UHD_STREAM_MODE_STOP_CONTINUOUS, 0),
            StreamCommand::NumSampsAndDone(n) => (ffi::uhd_stream_mode_t_UHD_STREAM_MODE_NUM_SAMPS_AND_DONE, n),
            StreamCommand::NumSampsAndMore(n) => (ffi::uhd_stream_mode_t_UHD_STREAM_MODE_NUM_SAMPS_AND_MORE, n),
        };
        let mut c = stream_cmd(mode);
        c.num_samps = n;
        if let Some(t) = at {
            c.stream_now = false;
            c.time_spec_full_secs = t.floor() as i64;
            c.time_spec_frac_secs = t.fract();
        }
        let e = unsafe { ffi::uhd_rx_streamer_issue_stream_cmd(self.handle, &c) };
        self.check(e, "issue_stream_cmd")?;
        self.streaming = !matches!(cmd, StreamCommand::StopContinuous);
        Ok(())
    }
}

fn stream_cmd(mode: ffi::uhd_stream_mode_t) -> ffi::uhd_stream_cmd_t {
    ffi::uhd_stream_cmd_t {
        stream_mode: mode,
        num_samps: 0,
        stream_now: true,
        time_spec_full_secs: 0,
        time_spec_frac_secs: 0.0,
    }
}
