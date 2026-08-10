//! IPC surface.
//!
//! Every body here is a one-line delegation to the engine, on purpose. The
//! moment a command grows a branch or holds state, that logic has escaped the
//! core and the SwiftUI migration gets more expensive. Treat a non-trivial
//! command body as a review failure.

use speakeasy_core::{Config, Engine, ModelChoice, ModelId, ModelInfo, ModelStatus, State};
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
pub fn model_catalog(engine: TauriState<'_, Engine>) -> Vec<ModelInfo> {
    engine.catalog().to_vec()
}

#[tauri::command]
pub fn model_status(engine: TauriState<'_, Engine>) -> ModelStatus {
    engine.model_status()
}

#[tauri::command]
pub fn installed_models(engine: TauriState<'_, Engine>) -> Vec<ModelId> {
    engine.installed_models()
}

#[tauri::command]
pub fn install_model(engine: TauriState<'_, Engine>, id: ModelId) {
    engine.install_model(id);
}

#[tauri::command]
pub fn select_model(engine: TauriState<'_, Engine>, choice: ModelChoice) {
    engine.select_model(choice);
}

#[tauri::command]
pub fn cancel_model_download(engine: TauriState<'_, Engine>) {
    engine.cancel_model_download();
}

/// The one command here that can fail, because deleting a file can.
#[tauri::command]
pub fn remove_model(engine: TauriState<'_, Engine>, id: ModelId) -> Result<(), String> {
    engine.remove_model(&id).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn permissions_granted() -> bool {
    speakeasy_platform::permissions_granted()
}

#[tauri::command]
pub fn open_permission_settings() {
    speakeasy_platform::open_permission_settings();
}
