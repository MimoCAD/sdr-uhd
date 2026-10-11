use std::ffi::CString;
use std::ptr;
use std::sync::Arc;

use sdr_core::{Error, Result};
use crate::ffi;
use sdr_core::{OtwFormat, TunePolicy, TuneRequest, TuneResult};
use crate::stream::UsrpStream;
use crate::tx::UsrpTx;
use crate::{cstr_lossy, fetch_string, global_last_error};

/// A motherboard's identity ([`Usrp::mboard_info`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MboardInfo {
    /// The product, e.g. `B206mini`.
    pub id: String,
    /// The name set in its EEPROM (may be empty).
    pub name: String,
    pub serial: String,
}

/// The buffer `pp_string` reads into, bytes.
const PP_STRING_BYTES: usize = 64 * 1024;

/// Owns the raw device handle. Freeing happens exactly once, when the last
/// `Arc` clone drops — which is strictly after any streamer built from it,
/// since streamers hold a clone as a keep-alive.
pub(crate) struct UsrpInner(pub(crate) ffi::uhd_usrp_handle);

impl Drop for UsrpInner {
    fn drop(&mut self) {
        unsafe {
            ffi::uhd_usrp_free(&mut self.0);
        }
    }
}

// The handle is an opaque heap pointer, safe to move between threads. Sync
// is needed only so `Arc<UsrpInner>` is Send; the streamer's clone is a pure
// keep-alive and is never used to call uhd_usrp_* functions concurrently.
unsafe impl Send for UsrpInner {}
unsafe impl Sync for UsrpInner {}

/// A USRP device. All configuration goes through `&mut self`, all readback
/// through `&self`, and every failure carries the device's own error string.
pub struct Usrp {
    inner: Arc<UsrpInner>,
}

impl Usrp {
    /// Open a device by UHD args string, e.g. `"type=b200"` (empty string:
    /// first device found).
    pub fn open(args: &str) -> Result<Usrp> {
        let args = CString::new(args)?;
        let mut h: ffi::uhd_usrp_handle = ptr::null_mut();
        let e = unsafe { ffi::uhd_usrp_make(&mut h, args.as_ptr()) };
        if e != ffi::uhd_error_UHD_ERROR_NONE || h.is_null() {
            return Err(Error::Uhd {
                code: e,
                context: "uhd_usrp_make",
                message: global_last_error(),
            });
        }
        Ok(Usrp {
            inner: Arc::new(UsrpInner(h)),
        })
    }

    fn handle(&self) -> ffi::uhd_usrp_handle {
        self.inner.0
    }

    fn check(&self, e: ffi::uhd_error, context: &'static str) -> Result<()> {
        if e == ffi::uhd_error_UHD_ERROR_NONE {
            Ok(())
        } else {
            let h = self.handle();
            Err(Error::Uhd {
                code: e,
                context,
                message: fetch_string(|p, n| unsafe { ffi::uhd_usrp_last_error(h, p, n) }),
            })
        }
    }

    /// The receive sample rate. UHD picks the master clock itself when it
    /// has not been set, and on a B2xx it can pick one EQUAL to the rate
    /// (decimation 1) — where the AD9364 puts symmetric sideband clusters
    /// ±270–310 kHz around every strong carrier, −18 to −40 dBc. Measured:
    /// decimation 1 bad at 19.2 and 24 Msps, decimation 2 or more clean at
    /// every rate, tuning and band tried. Set the master clock to twice the
    /// rate first ([`Usrp::set_master_clock_rate`], or `master_clock_rate=`
    /// in the open args); above 30.72 Msps the AD9364's 61.44 MHz ceiling
    /// leaves decimation 1 as the only plan.
    pub fn set_rx_rate(&mut self, rate_sps: f64, chan: usize) -> Result<()> {
        let e = unsafe { ffi::uhd_usrp_set_rx_rate(self.handle(), rate_sps, chan) };
        self.check(e, "uhd_usrp_set_rx_rate")
    }

    /// The rate the device actually settled on, in samples/sec.
    pub fn rx_rate(&self, chan: usize) -> Result<f64> {
        let mut out = 0.0;
        let e = unsafe { ffi::uhd_usrp_get_rx_rate(self.handle(), chan, &mut out) };
        self.check(e, "uhd_usrp_get_rx_rate")?;
        Ok(out)
    }

    /// Overall RX gain in dB (the empty gain-element name).
    pub fn set_rx_gain(&mut self, gain_db: f64, chan: usize) -> Result<()> {
        let e = unsafe { ffi::uhd_usrp_set_rx_gain(self.handle(), gain_db, chan, c"".as_ptr()) };
        self.check(e, "uhd_usrp_set_rx_gain")
    }

    pub fn rx_gain(&self, chan: usize) -> Result<f64> {
        let mut out = 0.0;
        let e = unsafe { ffi::uhd_usrp_get_rx_gain(self.handle(), chan, c"".as_ptr(), &mut out) };
        self.check(e, "uhd_usrp_get_rx_gain")?;
        Ok(out)
    }

    pub fn set_rx_antenna(&mut self, name: &str, chan: usize) -> Result<()> {
        let name = CString::new(name)?;
        let e = unsafe { ffi::uhd_usrp_set_rx_antenna(self.handle(), name.as_ptr(), chan) };
        self.check(e, "uhd_usrp_set_rx_antenna")
    }

    /// What the device actually selected — read back, not echoed.
    pub fn rx_antenna(&self, chan: usize) -> Result<String> {
        let mut buf = [0u8; 64];
        let e = unsafe {
            ffi::uhd_usrp_get_rx_antenna(self.handle(), chan, buf.as_mut_ptr().cast(), buf.len())
        };
        self.check(e, "uhd_usrp_get_rx_antenna")?;
        Ok(cstr_lossy(&buf))
    }

    pub fn set_rx_bandwidth(&mut self, bw_hz: f64, chan: usize) -> Result<()> {
        let e = unsafe { ffi::uhd_usrp_set_rx_bandwidth(self.handle(), bw_hz, chan) };
        self.check(e, "uhd_usrp_set_rx_bandwidth")
    }

    pub fn set_rx_freq(&mut self, req: &TuneRequest, chan: usize) -> Result<TuneResult> {
        let mut creq = request_to_c(req);
        let mut cres = ffi::uhd_tune_result_t {
            clipped_rf_freq: 0.0,
            target_rf_freq: 0.0,
            actual_rf_freq: 0.0,
            target_dsp_freq: 0.0,
            actual_dsp_freq: 0.0,
        };
        let e = unsafe { ffi::uhd_usrp_set_rx_freq(self.handle(), &mut creq, chan, &mut cres) };
        self.check(e, "uhd_usrp_set_rx_freq")?;
        Ok(TuneResult {
            clipped_rf_freq: cres.clipped_rf_freq,
            target_rf_freq: cres.target_rf_freq,
            actual_rf_freq: cres.actual_rf_freq,
            target_dsp_freq: cres.target_dsp_freq,
            actual_dsp_freq: cres.actual_dsp_freq,
        })
    }

    /// The current RX center frequency in Hz — read back, not echoed.
    pub fn rx_freq(&self, chan: usize) -> Result<f64> {
        let mut out = 0.0;
        let e = unsafe { ffi::uhd_usrp_get_rx_freq(self.handle(), chan, &mut out) };
        self.check(e, "uhd_usrp_get_rx_freq")?;
        Ok(out)
    }

    /// The probe-style device tree description. The C API copies it with
    /// `strncpy`, which leaves no terminating NUL when the text fills the
    /// buffer (a B206mini's is past 8 KiB): the buffer is generous and its
    /// last byte kept NUL, so a long text comes back cut, not empty.
    pub fn pp_string(&self) -> Result<String> {
        let mut buf = vec![0u8; PP_STRING_BYTES];
        let e = unsafe {
            ffi::uhd_usrp_get_pp_string(self.handle(), buf.as_mut_ptr().cast(), buf.len() - 1)
        };
        self.check(e, "uhd_usrp_get_pp_string")?;
        Ok(cstr_lossy(&buf))
    }

    /// The motherboard's identity as UHD reports it for receive channel
    /// `chan` (`uhd_usrp_get_rx_info`).
    pub fn mboard_info(&self, chan: usize) -> Result<MboardInfo> {
        let mut info: ffi::uhd_usrp_rx_info_t = unsafe { std::mem::zeroed() };
        let e = unsafe { ffi::uhd_usrp_get_rx_info(self.handle(), chan, &mut info) };
        self.check(e, "uhd_usrp_get_rx_info")?;
        let text = |p: *mut std::ffi::c_char| if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned() };
        let out = MboardInfo { id: text(info.mboard_id), name: text(info.mboard_name), serial: text(info.mboard_serial) };
        unsafe { ffi::uhd_usrp_rx_info_free(&mut info) };
        Ok(out)
    }

    /// The master clock (the AD9364's sample clock and the FPGA's), Hz. At
    /// twice the sample rate or more the receive path decimates, which keeps
    /// it clear of the decimation-1 spurs ([`Usrp::set_rx_rate`]).
    pub fn set_master_clock_rate(&mut self, hz: f64) -> Result<()> {
        let e = unsafe { ffi::uhd_usrp_set_master_clock_rate(self.handle(), hz, 0) };
        self.check(e, "uhd_usrp_set_master_clock_rate")
    }

    pub fn master_clock_rate(&self) -> Result<f64> {
        let mut out = 0.0;
        let e = unsafe { ffi::uhd_usrp_get_master_clock_rate(self.handle(), 0, &mut out) };
        self.check(e, "uhd_usrp_get_master_clock_rate")?;
        Ok(out)
    }

    /// The names of receive channel `chan`'s sensors.
    pub fn rx_sensor_names(&self, chan: usize) -> Result<Vec<String>> {
        let mut vec: ffi::uhd_string_vector_handle = ptr::null_mut();
        let mut out = Vec::new();
        unsafe {
            if ffi::uhd_string_vector_make(&mut vec) != ffi::uhd_error_UHD_ERROR_NONE {
                return Err(Error::Uhd { code: 0, context: "uhd_string_vector_make", message: global_last_error() });
            }
            let e = ffi::uhd_usrp_get_rx_sensor_names(self.handle(), chan, &mut vec);
            let mut n = 0usize;
            if e == ffi::uhd_error_UHD_ERROR_NONE && ffi::uhd_string_vector_size(vec, &mut n) == ffi::uhd_error_UHD_ERROR_NONE {
                for i in 0..n {
                    out.push(fetch_string(|p, len| ffi::uhd_string_vector_at(vec, i, p, len)));
                }
            }
            ffi::uhd_string_vector_free(&mut vec);
            self.check(e, "uhd_usrp_get_rx_sensor_names")?;
        }
        Ok(out)
    }

    /// A receive sensor's reading, as UHD prints it (`lo_locked`, `rssi`, …).
    pub fn rx_sensor(&self, name: &str, chan: usize) -> Result<String> {
        let name = CString::new(name)?;
        self.sensor(|h, v| unsafe { ffi::uhd_usrp_get_rx_sensor(h, name.as_ptr(), chan, v) }, "uhd_usrp_get_rx_sensor")
    }

    /// A motherboard sensor's reading, as UHD prints it (`ref_locked`, …).
    pub fn mboard_sensor(&self, name: &str) -> Result<String> {
        let name = CString::new(name)?;
        self.sensor(|h, v| unsafe { ffi::uhd_usrp_get_mboard_sensor(h, name.as_ptr(), 0, v) }, "uhd_usrp_get_mboard_sensor")
    }

    fn sensor(&self, get: impl FnOnce(ffi::uhd_usrp_handle, *mut ffi::uhd_sensor_value_handle) -> ffi::uhd_error, context: &'static str) -> Result<String> {
        let mut v: ffi::uhd_sensor_value_handle = ptr::null_mut();
        unsafe {
            if ffi::uhd_sensor_value_make(&mut v) != ffi::uhd_error_UHD_ERROR_NONE {
                return Err(Error::Uhd { code: 0, context: "uhd_sensor_value_make", message: global_last_error() });
            }
            let e = get(self.handle(), &mut v);
            let text = fetch_string(|p, len| ffi::uhd_sensor_value_to_pp_string(v, p, len));
            ffi::uhd_sensor_value_free(&mut v);
            self.check(e, context)?;
            Ok(text)
        }
    }

    /// Every command after this one is executed at device time `t`
    /// seconds, until [`Usrp::clear_command_time`].
    pub fn set_command_time(&mut self, t: f64) -> Result<()> {
        let e = unsafe { ffi::uhd_usrp_set_command_time(self.handle(), t.floor() as i64, t.fract(), 0) };
        self.check(e, "uhd_usrp_set_command_time")
    }

    pub fn clear_command_time(&mut self) -> Result<()> {
        let e = unsafe { ffi::uhd_usrp_clear_command_time(self.handle(), 0) };
        self.check(e, "uhd_usrp_clear_command_time")
    }

    /// The device clock now, seconds (the time received packets are
    /// stamped with), as close as the control transfer that reads it.
    pub fn time_now(&self) -> Result<f64> {
        let (mut full, mut frac) = (0i64, 0f64);
        let e = unsafe { ffi::uhd_usrp_get_time_now(self.handle(), 0, &mut full, &mut frac) };
        self.check(e, "uhd_usrp_get_time_now")?;
        Ok(full as f64 + frac)
    }

    /// Sets the device clock (the time every received packet is stamped
    /// with) to `us` microseconds of Unix time, now — as close as the
    /// control transfer that carries it.
    pub fn set_time_now_us(&mut self, us: i64) -> Result<()> {
        let full = us.div_euclid(1_000_000);
        let frac = us.rem_euclid(1_000_000) as f64 / 1e6;
        let e = unsafe { ffi::uhd_usrp_set_time_now(self.handle(), full, frac, 0) };
        self.check(e, "set_time_now")
    }

    pub fn set_tx_rate(&mut self, rate_sps: f64, chan: usize) -> Result<()> {
        let e = unsafe { ffi::uhd_usrp_set_tx_rate(self.handle(), rate_sps, chan) };
        self.check(e, "uhd_usrp_set_tx_rate")
    }

    /// The TX rate the device actually settled on, in samples/sec.
    pub fn tx_rate(&self, chan: usize) -> Result<f64> {
        let mut out = 0.0;
        let e = unsafe { ffi::uhd_usrp_get_tx_rate(self.handle(), chan, &mut out) };
        self.check(e, "uhd_usrp_get_tx_rate")?;
        Ok(out)
    }

    /// Overall TX gain in dB (the empty gain-element name).
    pub fn set_tx_gain(&mut self, gain_db: f64, chan: usize) -> Result<()> {
        let e = unsafe { ffi::uhd_usrp_set_tx_gain(self.handle(), gain_db, chan, c"".as_ptr()) };
        self.check(e, "uhd_usrp_set_tx_gain")
    }

    pub fn tx_gain(&self, chan: usize) -> Result<f64> {
        let mut out = 0.0;
        let e = unsafe { ffi::uhd_usrp_get_tx_gain(self.handle(), chan, c"".as_ptr(), &mut out) };
        self.check(e, "uhd_usrp_get_tx_gain")?;
        Ok(out)
    }

    pub fn set_tx_antenna(&mut self, name: &str, chan: usize) -> Result<()> {
        let name = CString::new(name)?;
        let e = unsafe { ffi::uhd_usrp_set_tx_antenna(self.handle(), name.as_ptr(), chan) };
        self.check(e, "uhd_usrp_set_tx_antenna")
    }

    pub fn set_tx_freq(&mut self, req: &TuneRequest, chan: usize) -> Result<TuneResult> {
        let mut creq = request_to_c(req);
        let mut cres = ffi::uhd_tune_result_t { clipped_rf_freq: 0.0, target_rf_freq: 0.0, actual_rf_freq: 0.0, target_dsp_freq: 0.0, actual_dsp_freq: 0.0 };
        let e = unsafe { ffi::uhd_usrp_set_tx_freq(self.handle(), &mut creq, chan, &mut cres) };
        self.check(e, "uhd_usrp_set_tx_freq")?;
        Ok(TuneResult {
            clipped_rf_freq: cres.clipped_rf_freq,
            target_rf_freq: cres.target_rf_freq,
            actual_rf_freq: cres.actual_rf_freq,
            target_dsp_freq: cres.target_dsp_freq,
            actual_dsp_freq: cres.actual_dsp_freq,
        })
    }

    /// Single-channel TX streamer taking sc16 ([`crate::Iq`]) on the host
    /// side, sc16 on the wire: bench signals ([`UsrpTx`]).
    pub fn tx_streamer(&self, chan: usize) -> Result<UsrpTx> {
        UsrpTx::new(Arc::clone(&self.inner), chan)
    }

    /// Single-channel RX streamer delivering sc16 ([`crate::Iq`]) on the
    /// host side. `stream_args` is UHD's extra stream-args string, usually
    /// `""`; `otw` picks the wire format (bandwidth vs precision).
    pub fn rx_streamer(
        &self,
        chan: usize,
        stream_args: &str,
        otw: OtwFormat,
    ) -> Result<UsrpStream> {
        UsrpStream::new(Arc::clone(&self.inner), chan, stream_args, otw)
    }
}

fn policy_to_c(p: TunePolicy) -> (ffi::uhd_tune_request_policy_t, f64) {
    match p {
        TunePolicy::Default => (
            ffi::uhd_tune_request_policy_t_UHD_TUNE_REQUEST_POLICY_NONE,
            0.0,
        ),
        TunePolicy::Auto => (
            ffi::uhd_tune_request_policy_t_UHD_TUNE_REQUEST_POLICY_AUTO,
            0.0,
        ),
        TunePolicy::Manual(freq) => (
            ffi::uhd_tune_request_policy_t_UHD_TUNE_REQUEST_POLICY_MANUAL,
            freq,
        ),
    }
}

fn request_to_c(r: &TuneRequest) -> ffi::uhd_tune_request_t {
    let (rf_freq_policy, rf_freq) = policy_to_c(r.rf);
    let (dsp_freq_policy, dsp_freq) = policy_to_c(r.dsp);
    ffi::uhd_tune_request_t {
        target_freq: r.target_freq,
        rf_freq_policy,
        rf_freq,
        dsp_freq_policy,
        dsp_freq,
        args: ptr::null_mut(),
    }
}
