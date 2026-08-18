//! IPC surface.
//!
//! Every body here is a one-line delegation to the engine, on purpose. The
//! moment a command grows a branch or holds state, that logic has escaped the
//! core and the SwiftUI migration gets more expensive. Treat a non-trivial
//! command body as a review failure.

use speakeasy_core::{
    Config, Engine, Hotkey, KeyboardBucket, ModelChoice, ModelId, ModelInfo, ModelStatus, State,
    Transcript,
};
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
pub fn recoverable_transcript(engine: TauriState<'_, Engine>) -> Option<String> {
    engine.recoverable_transcript()
}

#[tauri::command]
pub fn copy_transcript(engine: TauriState<'_, Engine>) -> Result<bool, String> {
    engine.copy_transcript().map_err(|err| err.to_string())
}

#[tauri::command]
pub fn history(engine: TauriState<'_, Engine>) -> Vec<Transcript> {
    engine.history()
}

#[tauri::command]
pub fn clear_history(engine: TauriState<'_, Engine>) {
    engine.clear_history();
}

#[tauri::command]
pub fn copy_history_entry(engine: TauriState<'_, Engine>, index: usize) -> Result<(), String> {
    engine
        .copy_history_entry(index)
        .map_err(|err| err.to_string())
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
pub fn set_hotkey(engine: TauriState<'_, Engine>, bucket: KeyboardBucket, hotkey: Hotkey) {
    engine.set_hotkey(bucket, hotkey);
}

#[tauri::command]
pub fn active_keyboard_bucket(engine: TauriState<'_, Engine>) -> KeyboardBucket {
    engine.active_bucket()
}

/// Returns as soon as the keyboard is armed, not when a key arrives. What was
/// pressed comes back as a `HotkeyChanged` event like any other rebinding.
///
/// `bucket` names the row that started the capture — see `hotkey::capture`
/// for why that must not be inferred from `Engine::active_bucket`.
#[tauri::command]
pub fn capture_hotkey(
    engine: TauriState<'_, Engine>,
    bucket: KeyboardBucket,
) -> Result<(), String> {
    crate::hotkey::capture(engine.inner().clone(), bucket).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn cancel_hotkey_capture() {
    crate::hotkey::cancel_capture();
}

/// Bind the configured hotkey again.
///
/// For the grant arriving late: registration at startup fails without
/// Accessibility, and nothing retries it, so a user who grants the permission
/// and comes back would have a settings window claiming everything is fine
/// above a key that does nothing.
#[tauri::command]
pub fn bind_hotkey(engine: TauriState<'_, Engine>) -> Result<(), String> {
    let bucket = engine.active_bucket();
    let hotkey = engine.config().hotkeys.get(bucket).clone();
    crate::hotkey::bind_if_unbound(engine.inner().clone(), &hotkey).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn permissions_granted() -> bool {
    speakeasy_platform::permissions_granted()
}

#[tauri::command]
pub fn open_permission_settings() {
    speakeasy_platform::open_permission_settings();
}
