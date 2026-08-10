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
use speakeasy_whisper::WhisperTranscriber;

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
        config.model_path = Some(model);
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
    // Loading weights takes seconds and can fail; an unloaded transcriber is a
    // valid state that reports ModelNotLoaded, so a bad path is not fatal.
    let transcriber = match &config.model_path {
        Some(path) => match WhisperTranscriber::load(std::path::Path::new(path)) {
            Ok(transcriber) => transcriber,
            Err(err) => {
                tracing::warn!(%err, "could not load the model; dictation is disabled");
                WhisperTranscriber::unloaded()
            }
        },
        None => {
            tracing::warn!("no model configured; set SPEAKEASY_MODEL or modelPath in config");
            WhisperTranscriber::unloaded()
        }
    };
    let sink = speakeasy_platform::text_sink(&config);

    let engine = Engine::new(Box::new(audio), Box::new(transcriber), sink, config.clone())
        .persisting_to(settings);

    tauri::Builder::default()
        .manage(engine.clone())
        .setup(move |app| {
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::state,
            commands::config,
            commands::set_config,
            commands::audio_level,
            commands::audio_spectrum,
            commands::cancel,
            commands::permissions_granted,
            commands::open_permission_settings,
        ])
        .run(tauri::generate_context!())
        .expect("tauri application should run");
}
