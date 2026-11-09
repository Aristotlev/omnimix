#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! OmniMix desktop shell: hosts the core mixer on its own thread, bridges it
//! to the React dashboard (commands in, state/levels/notices out), tray icon,
//! single instance, and guaranteed system restore on exit.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use omnimix_core::backend::AudioBackend;
use omnimix_core::mixer::{spawn, Command, MixerEvent, MixerHandle, MixerOptions, UiState};
use omnimix_core::mock::MockBackend;
use omnimix_core::state::{default_config_dir, StateStore};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

struct Shell {
    mixer: MixerHandle,
    close_to_tray: Arc<AtomicBool>,
    stopped: AtomicBool,
}

#[tauri::command]
fn get_state(shell: State<'_, Shell>) -> Result<UiState, String> {
    shell.mixer.state().ok_or_else(|| "The audio engine is not responding".to_string())
}

#[tauri::command]
fn command(shell: State<'_, Shell>, cmd: Command) {
    shell.mixer.send(cmd);
}

/// Called right before an update installs: hand every app back to the OS first.
#[tauri::command]
fn prepare_update(app: AppHandle) {
    stop_engine(&app);
}

fn make_backend(dir: PathBuf, demo: bool) -> Box<dyn AudioBackend> {
    if demo {
        return Box::new(MockBackend::new());
    }
    #[cfg(windows)]
    {
        match omnimix_windows::WindowsBackend::new(Some(dir)) {
            Ok(b) => Box::new(b),
            Err(e) => Box::new(DemoFallback(MockBackend::new(), e.to_string())),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = dir;
        match omnimix_macos::MacBackend::new() {
            Ok(b) => Box::new(b),
            Err(e) => Box::new(DemoFallback(MockBackend::new(), e.to_string())),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = dir;
        Box::new(MockBackend::new())
    }
}

/// Used only when the native backend cannot start: clearly labelled demo data.
#[allow(dead_code)]
struct DemoFallback(MockBackend, String);

macro_rules! fwd {
    ($($name:ident($($a:ident: $t:ty),*) -> $r:ty;)*) => {
        $(fn $name(&mut self, $($a: $t),*) -> $r { self.0.$name($($a),*) })*
    };
}

impl AudioBackend for DemoFallback {
    fn platform(&self) -> omnimix_core::backend::PlatformInfo {
        let mut p = self.0.platform();
        p.backend = format!("Demo data (native audio failed: {})", self.1);
        p
    }
    fn self_pid(&self) -> u32 {
        self.0.self_pid()
    }
    fwd! {
        refresh() -> omnimix_core::backend::BResult<omnimix_core::model::Snapshot>;
        levels(d: &[String], s: &[String]) -> omnimix_core::model::Levels;
        poll_events() -> Vec<omnimix_core::backend::BackendEvent>;
        set_device_volume(id: &str, v: f32) -> omnimix_core::backend::BResult<()>;
        set_device_mute(id: &str, m: bool) -> omnimix_core::backend::BResult<()>;
        set_device_channel_volume(id: &str, c: u16, v: f32) -> omnimix_core::backend::BResult<()>;
        set_default_device(id: &str) -> omnimix_core::backend::BResult<()>;
        set_source_volume(id: &str, v: f32) -> omnimix_core::backend::BResult<()>;
        set_source_mute(id: &str, m: bool) -> omnimix_core::backend::BResult<()>;
        set_source_channel_volume(id: &str, c: u16, v: f32) -> omnimix_core::backend::BResult<()>;
        dsp_support() -> omnimix_core::backend::DspSupport;
        close_output(id: &str) -> ();
        release(pid: u32) -> omnimix_core::backend::BResult<()>;
        shutdown() -> ();
    }
    fn open_output(&mut self, d: &omnimix_core::model::Device, l: omnimix_core::engine::LatencyMode) -> omnimix_core::backend::BResult<Arc<omnimix_core::engine::BusControls>> {
        self.0.open_output(d, l)
    }
    fn divert(
        &mut self,
        s: &omnimix_core::model::Source,
        t: &omnimix_core::model::Device,
        k: Option<&omnimix_core::model::Device>,
        tap: omnimix_core::backend::TapRequest,
    ) -> omnimix_core::backend::BResult<()> {
        self.0.divert(s, t, k, tap)
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Restore every routing/mute change before the process goes away.
fn stop_engine(app: &AppHandle) {
    if let Some(shell) = app.try_state::<Shell>() {
        if !shell.stopped.swap(true, Ordering::SeqCst) {
            shell.mixer.shutdown();
        }
    }
}

fn main() {
    let demo = std::env::args().any(|a| a == "--demo") || std::env::var_os("OMNIMIX_DEMO").is_some();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(move |app| {
            let dir = app.path().app_config_dir().unwrap_or_else(|_| default_config_dir());
            let _ = std::fs::create_dir_all(&dir);
            let store = StateStore::in_dir(&dir);
            let emitter = app.handle().clone();
            let close_to_tray = Arc::new(AtomicBool::new(true));
            let ctt = close_to_tray.clone();
            let bdir = dir.clone();
            let (mixer, _thread) = spawn(move || make_backend(bdir, demo), store, MixerOptions::default(), move |ev| match ev {
                MixerEvent::State(s) => {
                    ctt.store(s.prefs.close_to_tray, Ordering::Relaxed);
                    let _ = emitter.emit("omx://state", &*s);
                }
                MixerEvent::Levels(l) => {
                    let _ = emitter.emit("omx://levels", &l);
                }
                MixerEvent::Notice(n) => {
                    let _ = emitter.emit("omx://notice", &n);
                }
            });
            app.manage(Shell { mixer, close_to_tray, stopped: AtomicBool::new(false) });

            let show = MenuItem::with_id(app, "show", "Open OmniMix", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit OmniMix", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("omnimix").tooltip("OmniMix").menu(&menu).show_menu_on_left_click(false);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, e| match e.id.as_ref() {
                "show" => show_main(app),
                "quit" => {
                    stop_engine(app);
                    app.exit(0);
                }
                _ => {}
            })
            .on_tray_icon_event(|tray, e| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    show_main(tray.app_handle());
                }
            })
            .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let to_tray = window.app_handle().try_state::<Shell>().map(|s| s.close_to_tray.load(Ordering::Relaxed)).unwrap_or(false);
                if to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![get_state, command, prepare_update])
        .build(tauri::generate_context!())
        .expect("failed to start OmniMix");

    app.run(|app, event| {
        if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
            stop_engine(app);
        }
    });
}
