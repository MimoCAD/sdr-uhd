//! Drives a B2xx through libuhd, one named step at a time with a quiet
//! second between steps, so that a USB capture taken meanwhile can be read
//! step by step. Each step prints one line: `STEP <n> <name> host_ns=<unix
//! ns> dev=<device seconds or -> <result>`.
//!
//!   cargo run --release -p sdr-uhd --example capture_script -- <rx|tx|duplex> [uhd args]
//!
//! `tx` and `duplex` transmit from the TX/RX port: run them only with that
//! port cabled to a receiver through attenuation (the bench's 20 dB pad);
//! the transmit gain stays at `TX_GAIN_DB`.

use std::f64::consts::TAU;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sdr_core::{Iq, OtwFormat, TuneRequest};
use sdr_uhd::{StreamCommand, Usrp, UsrpStream, UsrpTx};

const GAP: Duration = Duration::from_secs(1);
const RX_HZ: f64 = 300e6;
const TX_GAIN_DB: f64 = 40.0;
const TONE_OFFSET_HZ: f64 = 250e3;

struct Steps {
    n: u32,
}

impl Steps {
    fn step(&mut self, u: &Usrp, name: &str, result: impl std::fmt::Display) {
        self.n += 1;
        let host = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dev = u.time_now().map(|t| format!("{t:.6}")).unwrap_or_else(|_| "-".into());
        println!("STEP {} {name} host_ns={host} dev={dev} {result}", self.n);
        std::thread::sleep(GAP);
    }
}

/// Receive for `secs`, returning (samples, overflows, first timestamp).
fn receive(rx: &mut UsrpStream, secs: f64) -> sdr_core::Result<String> {
    let mut buf = vec![Iq::default(); rx.max_num_samps()];
    let (mut n, mut overflows, mut first) = (0usize, 0u32, None);
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        let info = rx.recv_into(&mut buf, 0.2)?;
        n += info.n_samples;
        overflows += info.overflow as u32;
        if first.is_none() && info.n_samples > 0 {
            first = info.time_spec.map(|t| t.full_secs as f64 + t.frac_secs);
        }
    }
    Ok(format!("samples={n} overflows={overflows} first_time={first:?}"))
}

/// Receive until a finite command's samples have come (or `secs`).
fn receive_all(rx: &mut UsrpStream, secs: f64) -> sdr_core::Result<String> {
    let mut buf = vec![Iq::default(); rx.max_num_samps()];
    let (mut n, mut first) = (0usize, None);
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        let info = rx.recv_into(&mut buf, 0.2)?;
        if first.is_none() && info.n_samples > 0 {
            first = info.time_spec.map(|t| t.full_secs as f64 + t.frac_secs);
        }
        n += info.n_samples;
        if info.timed_out && n > 0 {
            break;
        }
    }
    Ok(format!("samples={n} first_time={first:?}"))
}

fn tone(n: usize, rate: f64, phase: &mut f64) -> Vec<Iq> {
    let a = 32767.0 * 0.3;
    (0..n)
        .map(|_| {
            let s = Iq { i: (a * phase.cos()) as i16, q: (a * phase.sin()) as i16 };
            *phase = (*phase + TAU * TONE_OFFSET_HZ / rate) % TAU;
            s
        })
        .collect()
}

fn send_for(tx: &mut UsrpTx, secs: f64, rate: f64, phase: &mut f64) -> sdr_core::Result<usize> {
    let chunk = tx.max_num_samps();
    let t0 = Instant::now();
    let mut n = 0;
    while t0.elapsed().as_secs_f64() < secs {
        tx.send(&tone(chunk, rate, phase), 1.0)?;
        n += chunk;
    }
    Ok(n)
}

fn rx_script(u: &mut Usrp, s: &mut Steps) -> sdr_core::Result<()> {
    s.step(u, "opened", format!("mcr={:?}", u.master_clock_rate()));
    u.set_rx_rate(1e6, 0)?;
    s.step(u, "rx_rate_1M", format!("rate={:?} mcr={:?}", u.rx_rate(0), u.master_clock_rate()));
    let r = u.set_rx_freq(&TuneRequest::auto(RX_HZ), 0)?;
    s.step(u, "rx_freq_300M", format!("{r:?}"));
    u.set_rx_gain(30.0, 0)?;
    s.step(u, "rx_gain_30", format!("gain={:?}", u.rx_gain(0)));
    u.set_rx_antenna("TX/RX", 0)?;
    s.step(u, "rx_antenna_TXRX", format!("antenna={:?}", u.rx_antenna(0)));
    u.set_rx_antenna("RX2", 0)?;
    s.step(u, "rx_antenna_RX2", format!("antenna={:?}", u.rx_antenna(0)));
    u.set_rx_bandwidth(1e6, 0)?;
    s.step(u, "rx_bandwidth_1M", "");
    s.step(u, "sensor_names", format!("{:?}", u.rx_sensor_names(0)));
    s.step(u, "sensor_lo_locked", u.rx_sensor("lo_locked", 0).unwrap_or_else(|e| e.to_string()));
    s.step(u, "sensor_rssi", u.rx_sensor("rssi", 0).unwrap_or_else(|e| e.to_string()));
    s.step(u, "sensor_temp", u.rx_sensor("temp", 0).unwrap_or_else(|e| e.to_string()));
    s.step(u, "mboard_ref_locked", u.mboard_sensor("ref_locked").unwrap_or_else(|e| e.to_string()));
    u.set_time_now_us(0)?;
    s.step(u, "set_time_0", "");
    let mut rx = u.rx_streamer(0, "", OtwFormat::Sc16)?;
    s.step(u, "rx_streamer_sc16", format!("max_num_samps={}", rx.max_num_samps()));
    rx.issue(StreamCommand::StartContinuous, None)?;
    let got = receive(&mut rx, 1.0)?;
    s.step(u, "start_continuous_recv_1s", got);
    let r = u.set_rx_freq(&TuneRequest::auto(RX_HZ + 100e3), 0)?;
    let got = receive(&mut rx, 0.5)?;
    s.step(u, "retune_streaming_300.1M", format!("{r:?} {got}"));
    u.set_rx_gain(50.0, 0)?;
    let got = receive(&mut rx, 0.5)?;
    s.step(u, "gain_50_streaming", got);
    std::thread::sleep(Duration::from_secs(1));
    let got = receive(&mut rx, 0.5)?;
    s.step(u, "overflow_paused_1s", got);
    rx.issue(StreamCommand::StopContinuous, None)?;
    let got = receive(&mut rx, 0.3)?;
    s.step(u, "stop_continuous_drain", got);
    rx.issue(StreamCommand::NumSampsAndDone(10_000), None)?;
    let got = receive_all(&mut rx, 1.0)?;
    s.step(u, "num_samps_10000_now", got);
    let at = u.time_now()? + 0.5;
    rx.issue(StreamCommand::NumSampsAndDone(5_000), Some(at))?;
    let got = receive_all(&mut rx, 1.5)?;
    s.step(u, "num_samps_5000_timed", format!("at={at:.6} {got}"));
    let at = u.time_now()? + 0.5;
    u.set_command_time(at)?;
    u.set_rx_freq(&TuneRequest::auto(RX_HZ + 200e3), 0)?;
    u.clear_command_time()?;
    s.step(u, "timed_retune_300.2M", format!("at={at:.6}"));
    drop(rx);
    s.step(u, "rx_streamer_dropped", "");
    let mut rx = u.rx_streamer(0, "", OtwFormat::Sc8)?;
    rx.issue(StreamCommand::StartContinuous, None)?;
    let got = receive(&mut rx, 0.5)?;
    rx.issue(StreamCommand::StopContinuous, None)?;
    let _ = receive(&mut rx, 0.3)?;
    s.step(u, "sc8_recv_0.5s", got);
    drop(rx);
    u.set_rx_rate(4e6, 0)?;
    let mut rx = u.rx_streamer(0, "", OtwFormat::Sc16)?;
    rx.issue(StreamCommand::StartContinuous, None)?;
    let got = receive(&mut rx, 0.5)?;
    rx.issue(StreamCommand::StopContinuous, None)?;
    let _ = receive(&mut rx, 0.3)?;
    s.step(u, "rate_4M_sc16_recv_0.5s", format!("rate={:?} mcr={:?} {got}", u.rx_rate(0), u.master_clock_rate()));
    drop(rx);
    u.set_master_clock_rate(32e6)?;
    s.step(u, "master_clock_32M", format!("mcr={:?}", u.master_clock_rate()));
    u.set_rx_rate(2e6, 0)?;
    s.step(u, "rx_rate_2M_at_mcr32", format!("rate={:?}", u.rx_rate(0)));
    s.step(u, "end", "");
    Ok(())
}

fn tx_script(u: &mut Usrp, s: &mut Steps) -> sdr_core::Result<()> {
    let rate = 1e6;
    s.step(u, "opened", format!("mcr={:?}", u.master_clock_rate()));
    u.set_tx_rate(rate, 0)?;
    s.step(u, "tx_rate_1M", format!("rate={:?}", u.tx_rate(0)));
    let r = u.set_tx_freq(&TuneRequest::auto(RX_HZ), 0)?;
    s.step(u, "tx_freq_300M", format!("{r:?}"));
    u.set_tx_gain(TX_GAIN_DB, 0)?;
    s.step(u, "tx_gain_40", format!("gain={:?}", u.tx_gain(0)));
    u.set_tx_antenna("TX/RX", 0)?;
    s.step(u, "tx_antenna_TXRX", "");
    u.set_time_now_us(0)?;
    s.step(u, "set_time_0", "");
    let mut tx = u.tx_streamer(0)?;
    s.step(u, "tx_streamer", format!("max_num_samps={}", tx.max_num_samps()));
    let mut phase = 0.0;
    let n = send_for(&mut tx, 1.0, rate, &mut phase)?;
    tx.end_burst()?;
    s.step(u, "tone_1s_then_end_burst", format!("sent={n} events={:?}", tx.async_events(0.5)?));
    let at = u.time_now()? + 0.5;
    tx.send_burst(&tone(10_000, rate, &mut phase), Some(at), 2.0)?;
    s.step(u, "timed_burst_10000", format!("at={at:.6} events={:?}", tx.async_events(1.0)?));
    let a = send_for(&mut tx, 0.2, rate, &mut phase)?;
    std::thread::sleep(Duration::from_millis(500));
    let b = send_for(&mut tx, 0.2, rate, &mut phase)?;
    tx.end_burst()?;
    s.step(u, "underflow_paused_0.5s", format!("sent={} events={:?}", a + b, tx.async_events(0.5)?));
    let a = send_for(&mut tx, 0.3, rate, &mut phase)?;
    let r = u.set_tx_freq(&TuneRequest::auto(RX_HZ + 100e3), 0)?;
    let b = send_for(&mut tx, 0.3, rate, &mut phase)?;
    u.set_tx_gain(TX_GAIN_DB - 10.0, 0)?;
    let c = send_for(&mut tx, 0.3, rate, &mut phase)?;
    tx.end_burst()?;
    s.step(u, "retune_and_gain_while_sending", format!("sent={} {r:?} events={:?}", a + b + c, tx.async_events(0.5)?));
    s.step(u, "end", "");
    Ok(())
}

fn duplex_script(u: &mut Usrp, s: &mut Steps) -> sdr_core::Result<()> {
    let rate = 1e6;
    u.set_rx_rate(rate, 0)?;
    u.set_tx_rate(rate, 0)?;
    u.set_rx_freq(&TuneRequest::auto(RX_HZ), 0)?;
    u.set_tx_freq(&TuneRequest::auto(RX_HZ), 0)?;
    u.set_rx_gain(30.0, 0)?;
    u.set_tx_gain(TX_GAIN_DB, 0)?;
    u.set_rx_antenna("RX2", 0)?;
    u.set_tx_antenna("TX/RX", 0)?;
    s.step(u, "configured", "");
    let mut rx = u.rx_streamer(0, "", OtwFormat::Sc16)?;
    let mut tx = u.tx_streamer(0)?;
    rx.issue(StreamCommand::StartContinuous, None)?;
    let mut phase = 0.0;
    let mut buf = vec![Iq::default(); rx.max_num_samps()];
    let (mut sent, mut got) = (0usize, 0usize);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(1) {
        tx.send(&tone(tx.max_num_samps(), rate, &mut phase), 1.0)?;
        sent += tx.max_num_samps();
        got += rx.recv_into(&mut buf, 0.0)?.n_samples;
    }
    tx.end_burst()?;
    rx.issue(StreamCommand::StopContinuous, None)?;
    s.step(u, "duplex_1s", format!("sent={sent} got={got} events={:?}", tx.async_events(0.5)?));
    s.step(u, "end", "");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let which = args.first().map(String::as_str).unwrap_or("rx");
    let uhd_args = args.get(1).map(String::as_str).unwrap_or("type=b200");
    let mut u = Usrp::open(uhd_args)?;
    let mut s = Steps { n: 0 };
    match which {
        "rx" => rx_script(&mut u, &mut s)?,
        "tx" => tx_script(&mut u, &mut s)?,
        "duplex" => duplex_script(&mut u, &mut s)?,
        other => return Err(format!("unknown script {other}").into()),
    }
    Ok(())
}
