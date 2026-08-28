use hangar_api::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    App, AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder, Window,
    WindowEvent, Wry,
};

const QUICK_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
// A full reconciliation is a safety net for deep creations whose parent was not
// in the focused metadata probe. It must not become the resident app's normal
// duty cycle: one least-recent root every six hours is progressive without
// chaining heavyweight scans all day.
const COMPLETE_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone)]
pub struct ResidentControl {
    quitting: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    force_refresh: Arc<AtomicBool>,
    started_hidden: bool,
}

impl Default for ResidentControl {
    fn default() -> Self {
        Self::new(false)
    }
}

impl ResidentControl {
    fn new(started_hidden: bool) -> Self {
        Self {
            quitting: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            force_refresh: Arc::new(AtomicBool::new(false)),
            started_hidden,
        }
    }

    pub fn started_hidden(&self) -> bool {
        self.started_hidden
    }

    pub fn request_refresh(&self) {
        self.force_refresh.store(true, Ordering::Release);
    }

    fn begin_quit(&self) {
        self.quitting.store(true, Ordering::Release);
        self.stop.store(true, Ordering::Release);
    }

    pub(crate) fn is_quitting(&self) -> bool {
        self.quitting.load(Ordering::Acquire)
    }
}

pub fn setup(app: &mut App<Wry>, state: AppState, start_hidden: bool) -> tauri::Result<()> {
    let control = ResidentControl::new(start_hidden);
    app.manage(control.clone());
    create_tray(app)?;
    start_refresh_loop(app.handle().clone(), state, control);

    if !start_hidden {
        show_main_window(app.handle());
    }
    Ok(())
}

fn create_tray(app: &App<Wry>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "resident-open", "Open Code Hangar", true, None::<&str>)?;
    let refresh = MenuItem::with_id(
        app,
        "resident-refresh",
        "Refresh projects now",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "resident-quit", "Exit Code Hangar", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &refresh, &separator, &quit])?;

    let mut builder = TrayIconBuilder::with_id("code-hangar-resident")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Code Hangar — local projects stay ready in the background")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "resident-open" => show_main_window(app),
            "resident-refresh" => app.state::<ResidentControl>().request_refresh(),
            "resident-quit" => {
                app.state::<ResidentControl>().begin_quit();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let should_open = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if should_open {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let _tray = builder.build(app)?;
    Ok(())
}

pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    let window = app.get_webview_window("main").or_else(|| {
        match WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
            .title("Code Hangar")
            .inner_size(1280.0, 820.0)
            .min_inner_size(960.0, 640.0)
            .visible(false)
            .build()
        {
            Ok(window) => Some(window),
            Err(error) => {
                eprintln!("Failed to create the Code Hangar window: {error}");
                None
            }
        }
    });
    if let Some(window) = window {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = app.emit("resident-window-shown", ());
    }
}

pub fn handle_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        let control = window.app_handle().state::<ResidentControl>();
        if !control.is_quitting() {
            api.prevent_close();
            // Closing returns to the native resident service. Destroying the
            // WebView (rather than merely hiding it) cancels deferred UI/catalog
            // hydration and releases the runtime; the next activation recreates
            // `main` from the local bundle.
            let _ = window.destroy();
        }
    }
}

fn start_refresh_loop(app: AppHandle<Wry>, state: AppState, control: ResidentControl) {
    std::thread::spawn(move || {
        // The encrypted DB opens on its own worker. The resident service waits for
        // that gate instead of competing with startup or surfacing transient errors.
        loop {
            if control.stop.load(Ordering::Acquire) {
                return;
            }
            match state.startup_status().state.as_str() {
                "ready" => break,
                "failed" => return,
                _ => std::thread::sleep(Duration::from_millis(250)),
            }
        }

        let mut first_pass = true;
        let mut last_quick = Instant::now()
            .checked_sub(QUICK_REFRESH_INTERVAL)
            .unwrap_or_else(Instant::now);
        let mut last_complete = Instant::now();
        loop {
            if control.stop.load(Ordering::Acquire) {
                return;
            }
            let forced = control.force_refresh.swap(false, Ordering::AcqRel);
            let quick_due = last_quick.elapsed() >= QUICK_REFRESH_INTERVAL;
            if first_pass || forced || quick_due {
                // Startup and 30-second ticks perform only bounded fingerprint
                // probes. They never promote a changing live project to a full
                // root scan; only the six-hour reconciliation or an explicit tray
                // refresh may start the one-worker background inventory.
                let complete_due = forced || last_complete.elapsed() >= COMPLETE_REFRESH_INTERVAL;
                match hangar_api::background_refresh_resident(&state, complete_due) {
                    Ok(Some(job_id)) => {
                        // No paths or file contents travel on the event bus; the UI
                        // receives only the opaque local job id for progress polling.
                        let _ = app.emit("background-scan-started", job_id);
                    }
                    Ok(None) => {}
                    Err(_) => {
                        // A foreground scan may win the race after candidate
                        // selection. The next bounded pass retries naturally.
                    }
                }
                first_pass = false;
                last_quick = Instant::now();
                if complete_due {
                    last_complete = Instant::now();
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_control_coalesces_manual_refresh_requests() {
        let control = ResidentControl::default();
        control.request_refresh();
        control.request_refresh();
        assert!(control.force_refresh.swap(false, Ordering::AcqRel));
        assert!(!control.force_refresh.swap(false, Ordering::AcqRel));
    }

    #[test]
    fn resident_control_remembers_how_the_process_started() {
        assert!(ResidentControl::new(true).started_hidden());
        assert!(!ResidentControl::default().started_hidden());
    }
}
