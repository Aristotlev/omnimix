//! Real-time streams: an event-driven WASAPI render client per engine bus, and
//! a process-loopback capture client per diverted app.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use omnimix_core::backend::TapRequest;
use omnimix_core::engine::ring::Producer;
use omnimix_core::engine::{BusControls, BusProcessor, LatencyMode, StripControls};
use windows::core::{implement, IUnknown, Interface, HRESULT};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{BLOB, CLSCTX_ALL};
use windows::Win32::System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;

use crate::util::{com_init, WStr};

pub struct Worker {
    stop: Arc<AtomicBool>,
    jh: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.jh.take() {
            let _ = j.join();
        }
    }
}

struct Event(HANDLE);
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn new_event() -> Result<Event, String> {
    unsafe { CreateEventW(None, false, false, None).map(Event).map_err(|e| format!("CreateEvent: {e}")) }
}

fn hns(ms: f64) -> i64 {
    (ms * 10_000.0) as i64
}

/// 32-bit float, stereo or more, as WAVEFORMATEXTENSIBLE.
pub fn float_format(rate: u32, channels: u16) -> WAVEFORMATEXTENSIBLE {
    let block = channels * 4;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * block as u32,
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: (std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
        dwChannelMask: if channels == 2 { 0x3 } else { 0 },
        SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    }
}

fn simple_format(rate: u32, channels: u16, float: bool) -> WAVEFORMATEX {
    let bits: u16 = if float { 32 } else { 16 };
    let block = channels * bits / 8;
    WAVEFORMATEX {
        wFormatTag: if float { WAVE_FORMAT_IEEE_FLOAT as u16 } else { 1 },
        nChannels: channels,
        nSamplesPerSec: rate,
        nAvgBytesPerSec: rate * block as u32,
        nBlockAlign: block,
        wBitsPerSample: bits,
        cbSize: 0,
    }
}

fn mmcss() -> Option<HANDLE> {
    let mut idx = 0u32;
    unsafe { AvSetMmThreadCharacteristicsW(WStr::new("Pro Audio").pcwstr(), &mut idx).ok() }
}

fn set_error(slot: &Mutex<Option<String>>, msg: String) {
    if let Ok(mut g) = slot.lock() {
        *g = Some(msg);
    }
}

// ------------------------------------------------------------------ render

/// Start rendering `ctl`'s bus to endpoint `device_id`. Returns once the
/// stream is running (or failed to start).
pub fn start_output(device_id: &str, ctl: Arc<BusControls>, latency: LatencyMode) -> Result<Worker, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let id = device_id.to_string();
    let s2 = stop.clone();
    let jh = std::thread::Builder::new()
        .name("omnimix-render".into())
        .spawn(move || {
            com_init();
            if let Err(e) = render_loop(&id, &ctl, latency, &s2, &tx) {
                set_error(&ctl.error, e.clone());
                let _ = tx.send(Err(e));
            }
            ctl.alive.store(false, Ordering::Relaxed);
        })
        .map_err(|e| e.to_string())?;
    let w = Worker { stop, jh: Some(jh) };
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(w),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Output stream did not start in time".into()),
    }
}

fn render_loop(id: &str, ctl: &Arc<BusControls>, latency: LatencyMode, stop: &AtomicBool, ready: &mpsc::Sender<Result<(), String>>) -> Result<(), String> {
    unsafe {
        let en: IMMDeviceEnumerator = windows::Win32::System::Com::CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("enumerator: {e}"))?;
        let dev = en.GetDevice(WStr::new(id).pcwstr()).map_err(|e| format!("open device: {e}"))?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(|e| format!("activate client: {e}"))?;
        let fmt = float_format(ctl.sample_rate, ctl.channels as u16);
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                hns(latency.output_ms()),
                0,
                &fmt.Format,
                None,
            )
            .map_err(|e| format!("initialize output: {e}"))?;
        let ev = new_event()?;
        client.SetEventHandle(ev.0).map_err(|e| format!("event: {e}"))?;
        let frames_total = client.GetBufferSize().map_err(|e| format!("buffer: {e}"))?;
        let render: IAudioRenderClient = client.GetService().map_err(|e| format!("render service: {e}"))?;
        if let Ok(sc) = client.GetService::<IAudioSessionControl>() {
            let _ = sc.SetDisplayName(WStr::new("OmniMix engine").pcwstr(), std::ptr::null());
        }
        let stream_lat = client.GetStreamLatency().unwrap_or(0) as f64 / 10_000.0;
        ctl.output_latency_ms.store((stream_lat + frames_total as f64 * 1000.0 / ctl.sample_rate as f64) as f32);

        let ch = ctl.channels;
        let mut proc_ = BusProcessor::new(ctl.clone());
        if let Ok(p) = render.GetBuffer(frames_total) {
            std::ptr::write_bytes(p, 0, frames_total as usize * ch * 4);
            let _ = render.ReleaseBuffer(frames_total, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
        }
        let task = mmcss();
        client.Start().map_err(|e| format!("start output: {e}"))?;
        ctl.alive.store(true, Ordering::Relaxed);
        let _ = ready.send(Ok(()));

        let mut result = Ok(());
        while !stop.load(Ordering::Relaxed) {
            if WaitForSingleObject(ev.0, 200) != WAIT_OBJECT_0 {
                continue;
            }
            let padding = match client.GetCurrentPadding() {
                Ok(p) => p,
                Err(e) => {
                    result = Err(format!("output lost: {e}"));
                    break;
                }
            };
            let avail = frames_total.saturating_sub(padding);
            if avail == 0 {
                continue;
            }
            match render.GetBuffer(avail) {
                Ok(p) => {
                    let out = std::slice::from_raw_parts_mut(p as *mut f32, avail as usize * ch);
                    proc_.render(out, avail as usize);
                    let _ = render.ReleaseBuffer(avail, 0);
                }
                Err(e) => {
                    result = Err(format!("output lost: {e}"));
                    break;
                }
            }
        }
        let _ = client.Stop();
        if let Some(t) = task {
            let _ = AvRevertMmThreadCharacteristics(t);
        }
        result
    }
}

// ------------------------------------------------------------------ process loopback

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivateDone(Mutex<Option<mpsc::Sender<()>>>);

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivateDone_Impl {
    fn ActivateCompleted(&self, _op: windows::core::Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        if let Ok(mut g) = self.0.lock() {
            if let Some(tx) = g.take() {
                let _ = tx.send(());
            }
        }
        Ok(())
    }
}

/// Activate a process-loopback IAudioClient capturing `pid` and its children.
pub fn activate_process_loopback(pid: u32) -> Result<IAudioClient, String> {
    unsafe {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS { TargetProcessId: pid, ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE },
            },
        };
        let mut pv = PROPVARIANT::default();
        {
            let inner = &mut *pv.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            inner.Anonymous.blob = BLOB { cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32, pBlobData: &mut params as *mut _ as *mut u8 };
        }
        let (tx, rx) = mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler = ActivateDone(Mutex::new(Some(tx))).into();
        let op = ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&pv), &handler)
            .map_err(|e| format!("process loopback unavailable: {e}"))?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "process loopback activation timed out".to_string())?;
        let mut hr = HRESULT(0);
        let mut unk: Option<IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unk).map_err(|e| format!("activation result: {e}"))?;
        hr.ok().map_err(|e| format!("process loopback refused: {e}"))?;
        let client = unk.ok_or("no audio client")?.cast::<IAudioClient>().map_err(|e| e.to_string())?;
        // `pv` points into `params` on this stack frame: never PropVariantClear it.
        std::mem::forget(pv);
        Ok(client)
    }
}

enum CapFmt {
    Float,
    Pcm16,
}

fn init_capture(pid: u32, rate: u32, ch: u16) -> Result<(IAudioClient, CapFmt), String> {
    let attempts: [(bool, bool); 4] = [(true, true), (true, false), (false, true), (false, false)];
    let mut last = String::new();
    for (float, autoconv) in attempts {
        let client = activate_process_loopback(pid)?;
        let fmt = simple_format(rate, ch, float);
        let mut flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
        if autoconv {
            flags |= AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        }
        match unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, hns(20.0), 0, &fmt, None) } {
            Ok(()) => return Ok((client, if float { CapFmt::Float } else { CapFmt::Pcm16 })),
            Err(e) => last = format!("initialize capture: {e}"),
        }
    }
    Err(last)
}

/// Start capturing process `pid` into the strip's ring.
pub fn start_tap(pid: u32, tap: TapRequest) -> Result<Worker, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let s2 = stop.clone();
    let TapRequest { producer, strip, sample_rate, channels } = tap;
    let jh = std::thread::Builder::new()
        .name(format!("omnimix-tap-{pid}"))
        .spawn(move || {
            com_init();
            let strip2 = strip.clone();
            if let Err(e) = capture_loop(pid, producer, &strip, sample_rate, channels, &s2, &tx) {
                set_error(&strip2.capture_error, e.clone());
                let _ = tx.send(Err(e));
            }
            strip2.capture_alive.store(false, Ordering::Relaxed);
        })
        .map_err(|e| e.to_string())?;
    let w = Worker { stop, jh: Some(jh) };
    match rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(())) => Ok(w),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Capture did not start in time".into()),
    }
}

fn capture_loop(
    pid: u32,
    mut producer: Producer,
    strip: &Arc<StripControls>,
    rate: u32,
    ch: u16,
    stop: &AtomicBool,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let (client, fmt) = init_capture(pid, rate, ch)?;
    unsafe {
        let ev = new_event()?;
        client.SetEventHandle(ev.0).map_err(|e| format!("event: {e}"))?;
        let cap: IAudioCaptureClient = client.GetService().map_err(|e| format!("capture service: {e}"))?;
        let task = mmcss();
        client.Start().map_err(|e| format!("start capture: {e}"))?;
        strip.capture_alive.store(true, Ordering::Relaxed);
        let _ = ready.send(Ok(()));
        let n_ch = ch as usize;
        let mut result = Ok(());
        'outer: while !stop.load(Ordering::Relaxed) {
            let _ = WaitForSingleObject(ev.0, 100);
            loop {
                let n = match cap.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(e) => {
                        result = Err(format!("capture lost: {e}"));
                        break 'outer;
                    }
                };
                if n == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if let Err(e) = cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None) {
                    result = Err(format!("capture lost: {e}"));
                    break 'outer;
                }
                let f = frames as usize;
                if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                    producer.push_silence(f);
                } else {
                    match fmt {
                        CapFmt::Float => {
                            let s = std::slice::from_raw_parts(data as *const f32, f * n_ch);
                            producer.push(s);
                        }
                        CapFmt::Pcm16 => {
                            let s = std::slice::from_raw_parts(data as *const i16, f * n_ch);
                            producer.push_with(f, |i| s[i] as f32 / 32768.0);
                        }
                    }
                }
                let _ = cap.ReleaseBuffer(frames);
            }
        }
        let _ = client.Stop();
        if let Some(t) = task {
            let _ = AvRevertMmThreadCharacteristics(t);
        }
        result
    }
}
