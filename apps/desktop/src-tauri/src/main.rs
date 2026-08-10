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

use speakeasy_core::{Config, Engine};
use speakeasy_platform::CpalAudioSource;
use speakeasy_whisper::WhisperTranscriber;
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "speakeasy=info".into()),
        )
        .init();

    let config = Config::default();

    let audio = CpalAudioSource::new().expect("audio source should be constructible");
    let transcriber = WhisperTranscriber::unloaded();
    let sink = speakeasy_platform::text_sink(&config);

    let engine = Engine::new(Box::new(audio), Box::new(transcriber), sink, config.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(engine.clone())
        .setup(move |app| {
            // Core events → webview events. The only place the two worlds meet.
            bridge::forward_events(app.handle().clone(), engine.clone());
            hotkey::register(app.handle().clone(), engine.clone(), &config.hotkey)?;
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
