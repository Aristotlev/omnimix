//! Real-time paths: engine bus rendered through an IOProc on the real output,
//! and per-app process taps read through private aggregate devices.

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use omnimix_core::backend::TapRequest;
use omnimix_core::engine::ring::Producer;
use omnimix_core::engine::{BusControls, BusProcessor, StripControls};

use crate::ffi::*;
use crate::objc::{nsarray, nsdict, nsnumber_bool, nsstring, release, tap_description};

unsafe fn buffers<'a>(list: *const AudioBufferList) -> &'a [AudioBuffer] {
    if list.is_null() {
        return &[];
    }
    let n = (*list).mNumberBuffers as usize;
    std::slice::from_raw_parts((*list).mBuffers.as_ptr(), n)
}

unsafe fn buffers_mut<'a>(list: *mut AudioBufferList) -> &'a mut [AudioBuffer] {
    if list.is_null() {
        return &mut [];
    }
    let n = (*list).mNumberBuffers as usize;
    std::slice::from_raw_parts_mut((*list).mBuffers.as_mut_ptr(), n)
}

unsafe fn zero_output(list: *mut AudioBufferList) {
    for b in buffers_mut(list) {
        if !b.mData.is_null() {
            std::ptr::write_bytes(b.mData as *mut u8, 0, b.mDataByteSize as usize);
        }
    }
}

// ------------------------------------------------------------------ bus output

struct BusCtx {
    proc_: BusProcessor,
    scratch: Vec<f32>,
}

unsafe extern "C" fn bus_ioproc(
    _dev: AudioObjectID,
    _now: *const c_void,
    _input: *const AudioBufferList,
    _in_time: *const c_void,
    output: *mut AudioBufferList,
    _out_time: *const c_void,
    client: *mut c_void,
) -> OSStatus {
    let ctx = &mut *(client as *mut BusCtx);
    let bufs = buffers_mut(output);
    if bufs.is_empty() || bufs[0].mData.is_null() {
        return 0;
    }
    let ch0 = bufs[0].mNumberChannels.max(1) as usize;
    let frames = bufs[0].mDataByteSize as usize / (4 * ch0);
    let interleaved = bufs.len() == 1 || ch0 > 1;
    let mut done = 0;
    while done < frames {
        let n = (frames - done).min(ctx.scratch.len() / 2);
        let s = &mut ctx.scratch[..n * 2];
        ctx.proc_.render(s, n);
        if interleaved {
            let out = std::slice::from_raw_parts_mut(bufs[0].mData as *mut f32, frames * ch0);
            for f in 0..n {
                let row = &mut out[(done + f) * ch0..(done + f + 1) * ch0];
                row.fill(0.0);
                if ch0 >= 2 {
                    row[0] = s[f * 2];
                    row[1] = s[f * 2 + 1];
                } else {
                    row[0] = 0.5 * (s[f * 2] + s[f * 2 + 1]);
                }
            }
        } else {
            for (bi, b) in bufs.iter_mut().enumerate() {
                if b.mData.is_null() {
                    continue;
                }
                let out = std::slice::from_raw_parts_mut(b.mData as *mut f32, frames);
                for f in 0..n {
                    out[done + f] = if bi < 2 { s[f * 2 + bi] } else { 0.0 };
                }
            }
        }
        done += n;
    }
    // Remaining buffers of an interleaved multi-stream device stay silent.
    for b in bufs.iter_mut().skip(1) {
        if interleaved && !b.mData.is_null() {
            std::ptr::write_bytes(b.mData as *mut u8, 0, b.mDataByteSize as usize);
        }
    }
    0
}

pub struct Output {
    dev: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    ctx: *mut BusCtx,
    ctl: Arc<BusControls>,
}

impl Output {
    pub fn start(dev: AudioObjectID, ctl: Arc<BusControls>) -> Result<Output, String> {
        let ctx = Box::into_raw(Box::new(BusCtx { proc_: BusProcessor::new(ctl.clone()), scratch: vec![0.0; 4096 * 2] }));
        let mut proc_id: AudioDeviceIOProcID = None;
        unsafe {
            if let Err(e) = check(AudioDeviceCreateIOProcID(dev, bus_ioproc, ctx as *mut c_void, &mut proc_id), "create output") {
                drop(Box::from_raw(ctx));
                return Err(e);
            }
            if let Err(e) = check(AudioDeviceStart(dev, proc_id), "start output") {
                AudioDeviceDestroyIOProcID(dev, proc_id);
                drop(Box::from_raw(ctx));
                return Err(e);
            }
        }
        let rate = ctl.sample_rate as f64;
        let frames = get::<u32>(dev, addr(kAudioDevicePropertyBufferFrameSize, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or(512);
        let lat = get::<u32>(dev, addr(kAudioDevicePropertyLatency, SCOPE_OUTPUT, ELEMENT_MAIN)).unwrap_or(0);
        ctl.output_latency_ms.store(((frames + lat) as f64 * 1000.0 / rate) as f32);
        ctl.alive.store(true, Ordering::Relaxed);
        Ok(Output { dev, proc_id, ctx, ctl })
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.dev, self.proc_id);
            AudioDeviceDestroyIOProcID(self.dev, self.proc_id);
            drop(Box::from_raw(self.ctx));
        }
        self.ctl.alive.store(false, Ordering::Relaxed);
    }
}

// ------------------------------------------------------------------ app taps

struct TapCtx {
    producer: Producer,
    strip: Arc<StripControls>,
}

unsafe extern "C" fn tap_ioproc(
    _dev: AudioObjectID,
    _now: *const c_void,
    input: *const AudioBufferList,
    _in_time: *const c_void,
    output: *mut AudioBufferList,
    _out_time: *const c_void,
    client: *mut c_void,
) -> OSStatus {
    let ctx = &mut *(client as *mut TapCtx);
    zero_output(output);
    let bufs = buffers(input);
    // The tap is the last input stream (after any inputs of the clock device).
    if let Some(b) = bufs.iter().rev().find(|b| b.mNumberChannels == 2 && !b.mData.is_null()) {
        let frames = b.mDataByteSize as usize / 8;
        let s = std::slice::from_raw_parts(b.mData as *const f32, frames * 2);
        ctx.producer.push(s);
    } else if bufs.len() >= 2 {
        let (l, r) = (&bufs[bufs.len() - 2], &bufs[bufs.len() - 1]);
        if l.mNumberChannels == 1 && r.mNumberChannels == 1 && !l.mData.is_null() && !r.mData.is_null() {
            let frames = (l.mDataByteSize.min(r.mDataByteSize) / 4) as usize;
            let ls = std::slice::from_raw_parts(l.mData as *const f32, frames);
            let rs = std::slice::from_raw_parts(r.mData as *const f32, frames);
            ctx.producer.push_with(frames, |i| if i % 2 == 0 { ls[i / 2] } else { rs[i / 2] });
        }
    } else if let Some(b) = bufs.first().filter(|b| b.mNumberChannels == 1 && !b.mData.is_null()) {
        let frames = b.mDataByteSize as usize / 4;
        let s = std::slice::from_raw_parts(b.mData as *const f32, frames);
        ctx.producer.push_with(frames, |i| s[i / 2]);
    }
    ctx.strip.capture_alive.store(true, Ordering::Relaxed);
    0
}

pub struct Tap {
    tap_id: AudioObjectID,
    agg: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    ctx: *mut TapCtx,
}

impl Tap {
    /// Tap `process_object` (silencing it while tapped) and stream it into the strip.
    pub fn start(pid: u32, process_object: AudioObjectID, target: AudioObjectID, target_uid: &str, req: TapRequest) -> Result<Tap, String> {
        let target_has_input = channel_count(target, SCOPE_INPUT);
        objc2::rc::autoreleasepool(|_| unsafe { Self::start_inner(pid, process_object, target_uid, target_has_input, req) })
    }

    unsafe fn start_inner(pid: u32, process_object: AudioObjectID, target_uid: &str, target_has_input: u32, req: TapRequest) -> Result<Tap, String> {
        let (desc, tap_uid) = tap_description(process_object, &format!("OmniMix {pid}"))?;
        let mut tap_id: AudioObjectID = 0;
        let st = AudioHardwareCreateProcessTap(desc as *mut c_void, &mut tap_id);
        release(desc);
        check(st, "create process tap")?;

        let destroy_tap = |id| {
            AudioHardwareDestroyProcessTap(id);
        };
        if let Some(fmt) = get::<AudioStreamBasicDescription>(tap_id, addr(kAudioTapPropertyFormat, SCOPE_GLOBAL, ELEMENT_MAIN)) {
            if fmt.mSampleRate > 0.0 && (fmt.mSampleRate - req.sample_rate as f64).abs() > 1.0 {
                destroy_tap(tap_id);
                return Err(format!("app runs at {:.0} Hz but the output runs at {} Hz", fmt.mSampleRate, req.sample_rate));
            }
        }

        let agg_uid = format!("app.omnimix.tap.{pid}.{tap_uid}");
        let tap_entry = nsdict(&[("uid", nsstring(&tap_uid)), ("drift", nsnumber_bool(true))]);
        let mut pairs: Vec<(&str, crate::objc::Id)> = vec![
            ("name", nsstring(&format!("OmniMix tap {pid}"))),
            ("uid", nsstring(&agg_uid)),
            ("private", nsnumber_bool(true)),
            ("stacked", nsnumber_bool(false)),
            ("tapautostart", nsnumber_bool(true)),
            ("taps", nsarray(&[tap_entry])),
        ];
        // Clock the tap from the output it plays on. Outputs that also have a
        // microphone (AirPods, USB headsets) are left out: opening their input
        // would drop Bluetooth into hands-free quality. The tap clocks itself then.
        if target_has_input == 0 {
            pairs.push(("master", nsstring(target_uid)));
            pairs.push(("subdevices", nsarray(&[nsdict(&[("uid", nsstring(target_uid))])])));
        }
        let desc = nsdict(&pairs);
        let mut agg: AudioObjectID = 0;
        if let Err(e) = check(AudioHardwareCreateAggregateDevice(desc as *const c_void, &mut agg), "create aggregate") {
            destroy_tap(tap_id);
            return Err(e);
        }
        // A fresh aggregate needs a moment before its IOProc delivers audio.
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(1500) {
            if get::<u32>(agg, addr(kAudioDevicePropertyDeviceIsAlive, SCOPE_GLOBAL, ELEMENT_MAIN)).unwrap_or(0) != 0 && channel_count(agg, SCOPE_INPUT) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        let TapRequest { producer, strip, .. } = req;
        let ctx = Box::into_raw(Box::new(TapCtx { producer, strip }));
        let mut proc_id: AudioDeviceIOProcID = None;
        let cleanup = |ctx: *mut TapCtx, proc_id: AudioDeviceIOProcID| {
            if proc_id.is_some() {
                AudioDeviceDestroyIOProcID(agg, proc_id);
            }
            AudioHardwareDestroyAggregateDevice(agg);
            AudioHardwareDestroyProcessTap(tap_id);
            drop(Box::from_raw(ctx));
        };
        if let Err(e) = check(AudioDeviceCreateIOProcID(agg, tap_ioproc, ctx as *mut c_void, &mut proc_id), "create tap reader") {
            cleanup(ctx, None);
            return Err(e);
        }
        if let Err(e) = check(AudioDeviceStart(agg, proc_id), "start tap reader") {
            cleanup(ctx, proc_id);
            return Err(e);
        }
        Ok(Tap { tap_id, agg, proc_id, ctx })
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.agg, self.proc_id);
            AudioDeviceDestroyIOProcID(self.agg, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.agg);
            AudioHardwareDestroyProcessTap(self.tap_id);
            let ctx = Box::from_raw(self.ctx);
            ctx.strip.capture_alive.store(false, Ordering::Relaxed);
            drop(ctx);
        }
    }
}
