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

/// Config source, until a settings file exists: defaults, with the model path
/// overridable from the environment so the pipeline can be exercised without a
/// settings UI.
fn load_config() -> Config {
    let mut config = Config::default();
    if let Ok(path) = std::env::var("SPEAKEASY_MODEL") {
        config.model_path = Some(path);
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

    let config = load_config();

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

    let engine = Engine::new(Box::new(audio), Box::new(transcriber), sink, config.clone());

    tauri::Builder::default()
        .manage(engine.clone())
        .setup(move |app| {
            // Core events → webview events. The only place the two worlds meet.
            bridge::forward_events(app.handle().clone(), engine.clone());
            tray::attach(app.handle(), &engine)?;
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
            commands::cancel,
            commands::permissions_granted,
            commands::open_permission_settings,
        ])
        .run(tauri::generate_context!())
        .expect("tauri application should run");
}
