//! IPC surface.
//!
//! Every body here is a one-line delegation to the engine, on purpose. The
//! moment a command grows a branch or holds state, that logic has escaped the
//! core and the SwiftUI migration gets more expensive. Treat a non-trivial
//! command body as a review failure.

use speakeasy_core::{Config, Engine, State};
use tauri::State as TauriState;

#[tauri::command]
pub fn state(engine: TauriState<'_, Engine>) -> State {
    engine.state()
}

#[tauri::command]
pub fn config(engine: TauriState<'_, Engine>) -> Config {
    engine.config()
}

#[tauri::command]
pub fn set_config(engine: TauriState<'_, Engine>, config: Config) {
    engine.set_config(config);
}

#[tauri::command]
pub fn audio_level(engine: TauriState<'_, Engine>) -> f32 {
    engine.audio_level()
}

#[tauri::command]
pub fn audio_spectrum(engine: TauriState<'_, Engine>) -> Vec<f32> {
    engine.audio_spectrum()
}

#[tauri::command]
pub fn cancel(engine: TauriState<'_, Engine>) {
    engine.cancel();
}

#[tauri::command]
pub fn permissions_granted() -> bool {
    speakeasy_platform::permissions_granted()
}

#[tauri::command]
pub fn open_permission_settings() {
    speakeasy_platform::open_permission_settings();
}
