#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Tauri shell for Speakeasy.
//!
//! This binary owns the tray, the windows, and the IPC surface — nothing else.
//! All application behaviour lives in `speakeasy-core`, which has no idea Tauri
//! exists. Replacing this crate with a SwiftUI app is therefore a matter of
//! writing a new shell against the same `Engine`, not a rewrite.

mod bridge;
mod commands;
mod hotkey;
mod tray;
mod widget;

use speakeasy_core::{Config, Engine};
use speakeasy_platform::CpalAudioSource;
use speakeasy_whisper::{WhisperSegmenter, WhisperTranscriber};

/// Config file location. Under the OS config directory rather than next to the
/// binary, so it survives reinstalling the app.
fn config_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                std::path::PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
            })
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("com.speakeasy.app").join("config.json")
}

/// Downloaded weights. A sibling of `config.json`, so they survive reinstalling
/// the app and are found again by the next version.
fn models_dir(config: &std::path::Path) -> std::path::PathBuf {
    config
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("models")
}

/// Settings file, with the model path still overridable from the environment
/// so the pipeline can be exercised without touching the file.
///
/// A file that cannot be read falls back to defaults rather than refusing to
/// start: a menu-bar app that dies on a bad config leaves the user no UI to fix
/// it with.
fn load_config(path: &std::path::Path) -> Config {
    let mut config = Config::load(path).unwrap_or_else(|err| {
        tracing::warn!(%err, "could not read the config; starting from defaults");
        Config::default()
    });
    if let Ok(model) = std::env::var("SPEAKEASY_MODEL") {
        config.model = speakeasy_core::ModelChoice::Local { path: model };
    }
    config
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "speakeasy=info".into()),
        )
        .init();

    let settings = config_path();
    let config = load_config(&settings);

    let audio = CpalAudioSource::new().expect("audio source should be constructible");
    // Always unloaded. Which weights to use, whether they are on disk, and
    // whether to fetch them are all `Engine::ensure_model`'s decision — so
    // every shell gets identical first-run behaviour without writing it twice.
    let transcriber = std::sync::Arc::new(WhisperTranscriber::unloaded());
    let sink = speakeasy_platform::text_sink(&config);

    let models = models_dir(&settings);
    let engine = Engine::new(
        Box::new(audio),
        transcriber,
        sink,
        Box::new(speakeasy_models::HttpFetcher::new()),
        models.clone(),
        config.clone(),
    )
    .persisting_to(settings);

    // A detector that will not load costs the user their live preview, not
    // their dictation — so this warns and carries on rather than failing to
    // start.
    let engine = match WhisperSegmenter::new(&models) {
        Ok(segmenter) => engine.with_segmenter(std::sync::Arc::new(segmenter)),
        Err(err) => {
            tracing::warn!(%err, "previews are off: the voice detector would not load");
            engine
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(engine.clone())
        .setup(move |app| {
            // Accessory, not Regular: no Dock icon, no app switcher entry, and
            // — the part that matters — the app never activates.
            //
            // Everything about this tool assumes the user's focus stays where
            // it was. An activating app takes the focus the transcript is aimed
            // at, so the text lands in the wrong window, and the status panel
            // starts eating keystrokes meant for the app underneath.
            //
            // `LSUIElement` in Info.plist says the same thing, but only once
            // the app is bundled; `tauri dev` runs the bare binary, where it
            // has no effect at all. Setting the policy here covers both.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Before anything tries to use the grant. Asking is what registers
            // the app in the Accessibility list, so without this the settings
            // pane the banner sends people to has no Speakeasy row to switch
            // on — which reads as the app being broken rather than unpermitted.
            // Two signals, logged together because they can disagree: the AX
            // API caches per process, while the tap probe reports what is
            // actually possible right now. `trusted=false can_tap=true` is that
            // cache being stale; both false is a grant that really is missing.
            let trusted = speakeasy_platform::request_permissions();
            let can_tap = speakeasy_platform::permissions_granted();
            tracing::info!(trusted, can_tap, "accessibility at startup");

            // An unsigned app launched from a DMG runs from a randomised
            // read-only path, and TCC keys the grant to that path — so the
            // permission is granted to an instance that will not exist next
            // launch, and the app asks again forever. Worth saying out loud,
            // because from the user's side it looks like the grant simply does
            // not work.
            if let Ok(exe) = std::env::current_exe() {
                if exe.to_string_lossy().contains("AppTranslocation") {
                    tracing::warn!(
                        path = %exe.display(),
                        "running translocated: copy Speakeasy.app to /Applications and \
                         clear com.apple.quarantine, or the Accessibility grant cannot stick"
                    );
                }
            }
            if !can_tap {
                tracing::warn!("Accessibility has not been granted; the hotkey will not bind");
            }

            // Core events → webview events. The only place the two worlds meet.
            bridge::forward_events(app.handle().clone(), engine.clone());
            tray::attach(app.handle(), &engine)?;
            widget::remember_moves(app.handle(), &engine);
            // A failure here is usually a missing Accessibility grant. Log it
            // and keep running: the tray still works, and the settings window
            // is where the user is told what to fix.
            if let Err(err) = hotkey::register(engine.clone(), &config.hotkey) {
                tracing::warn!(%err, "push-to-talk is not bound");
            }
            // Last, and after the bridge and tray are up: on a fresh install
            // this starts a download immediately, and the progress has to have
            // somewhere to land.
            engine.ensure_model();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::state,
            commands::config,
            commands::set_config,
            commands::audio_level,
            commands::audio_spectrum,
            commands::cancel,
            commands::model_catalog,
            commands::model_status,
            commands::installed_models,
            commands::install_model,
            commands::select_model,
            commands::cancel_model_download,
            commands::remove_model,
            commands::set_hotkey,
            commands::capture_hotkey,
            commands::cancel_hotkey_capture,
            commands::bind_hotkey,
            commands::permissions_granted,
            commands::open_permission_settings,
        ])
        .run(tauri::generate_context!())
        .expect("tauri application should run");
}
