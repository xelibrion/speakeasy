//! Tray menu.
//!
//! The icon itself is declared in `tauri.conf.json`, which is enough to place
//! it in the menu bar but gives it no menu — those have to be built in Rust.
//! This attaches one to the config-created icon (id `main`).
//!
//! Quit matters more than it looks: the app is `LSUIElement`, so there is no
//! Dock icon and no app-switcher entry. Without this menu the only way to stop
//! it is killing the process.
//!
//! A status line reflects the engine's current state. It is disabled — it is a
//! readout, not a command — and `refresh` is called by the event bridge on
//! every transition.

use std::sync::Mutex;

use speakeasy_core::{Engine, ModelStatus, State};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Manager};

/// The status line, kept so the bridge can rewrite its text.
static STATUS_ITEM: Mutex<Option<MenuItem<tauri::Wry>>> = Mutex::new(None);

/// The two things the status line reports on.
///
/// Both are kept because either can change without the other, and the line has
/// to be rendered from both — a download finishing must not reset the dictation
/// state shown, and vice versa.
static LAST_STATE: Mutex<Option<State>> = Mutex::new(None);
static MODEL_STATUS: Mutex<Option<ModelStatus>> = Mutex::new(None);

const TRAY_ID: &str = "main";

pub fn attach(app: &AppHandle, engine: &Engine) -> Result<(), Box<dyn std::error::Error>> {
    *LAST_STATE.lock().expect("last state mutex poisoned") = Some(engine.state());
    *MODEL_STATUS.lock().expect("model status mutex poisoned") = Some(engine.model_status());

    let status = MenuItem::with_id(app, "status", status_text(), false, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Speakeasy", true, Some("CmdOrCtrl+Q"))?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or("tray icon `main` was not created; check app.trayIcon in tauri.conf.json")?;
    tray.set_menu(Some(menu))?;
    // Without this, a left click does nothing and only right click opens the
    // menu — which reads as a broken icon.
    tray.set_show_menu_on_left_click(true)?;

    *STATUS_ITEM.lock().expect("status item mutex poisoned") = Some(status);

    app.on_menu_event(|app, event| match event.id().as_ref() {
        "settings" => show_settings(app),
        "quit" => app.exit(0),
        _ => {}
    });

    Ok(())
}

/// Rewrite the status line after an engine transition.
///
/// Call on the main thread, like everything else that touches AppKit.
pub fn refresh(state: &State) {
    *LAST_STATE.lock().expect("last state mutex poisoned") = Some(state.clone());
    render();
}

/// Rewrite the status line after a model status change.
pub fn refresh_model(status: &ModelStatus) {
    *MODEL_STATUS.lock().expect("model status mutex poisoned") = Some(status.clone());
    render();
}

fn render() {
    if let Some(item) = STATUS_ITEM
        .lock()
        .expect("status item mutex poisoned")
        .as_ref()
    {
        if let Err(err) = item.set_text(status_text()) {
            tracing::warn!(%err, "could not update tray status");
        }
    }
}

/// What the menu bar says.
///
/// The model line wins whenever there is one worth showing: downloading is the
/// more urgent and more transient thing to say, and while it runs the dictation
/// state is almost always `Idle` anyway. A model that is absent or broken also
/// wins, because in that state "Ready" would be a lie.
fn status_text() -> String {
    match MODEL_STATUS
        .lock()
        .expect("model status mutex poisoned")
        .as_ref()
    {
        Some(ModelStatus::Downloading {
            id,
            received,
            total,
        }) if *total > 0 => {
            return format!("Downloading {id}… {}%", received * 100 / total);
        }
        Some(ModelStatus::Downloading { id, .. }) => return format!("Downloading {id}…"),
        Some(ModelStatus::Failed { error }) => return format!("Model error: {error}"),
        Some(ModelStatus::Absent) => return "No model loaded".to_string(),
        Some(ModelStatus::Ready { .. }) | None => {}
    }

    match LAST_STATE
        .lock()
        .expect("last state mutex poisoned")
        .as_ref()
    {
        Some(State::Recording) => "Recording…".to_string(),
        Some(State::Transcribing) => "Transcribing…".to_string(),
        Some(State::Injecting) => "Inserting text…".to_string(),
        Some(State::Failed { error }) => format!("Error: {error}"),
        Some(State::Idle) | None => "Ready".to_string(),
    }
}

fn show_settings(app: &AppHandle) {
    // The window is created hidden (`visible: false`) so the app starts in the
    // menu bar rather than popping a window on launch.
    match app.get_webview_window("settings") {
        Some(window) => {
            if let Err(err) = window.show().and_then(|_| window.set_focus()) {
                tracing::warn!(%err, "could not show settings window");
            }
        }
        None => tracing::warn!("no window labelled `settings`"),
    }
}
