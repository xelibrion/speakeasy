/**
 * Typed mirror of the core's public API.
 *
 * These types are hand-written against `speakeasy-core`'s serde
 * representations. If the core's `Event`, `State` or `Config` change, this file
 * changes with them — consider generating it (ts-rs / specta) once the shapes
 * settle.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type State =
  | { state: "idle" }
  | { state: "recording" }
  | { state: "transcribing" }
  | { state: "injecting" }
  | { state: "failed"; error: EngineError };

/**
 * Serde tags for `core::error::Error`. Variants with a payload carry it in
 * `message`; the rest arrive as a bare `kind`, which is why the UI cannot rely
 * on `message` being present.
 */
export type ErrorKind =
  | "AudioUnavailable"
  | "MicrophonePermissionDenied"
  | "InputPermissionDenied"
  | "SecureInputActive"
  | "ModelNotLoaded"
  | "ModelDownload"
  | "ModelVerification"
  | "Transcription"
  | "Injection"
  | "Hotkey"
  | "Other";

export type EngineError = {
  kind: ErrorKind;
  message?: string;
};

/**
 * What to show the user for a failure.
 *
 * Keyed on kind rather than using `message`, because the payloads are developer
 * strings ("could not load /path/…: …") and the widget is a one-line pill. The
 * full message still reaches the console and the tracing log.
 */
const ERROR_LABELS: Record<ErrorKind, string> = {
  AudioUnavailable: "No microphone available",
  MicrophonePermissionDenied: "Microphone access denied",
  InputPermissionDenied: "Accessibility access denied",
  SecureInputActive: "A password field is blocking input",
  ModelNotLoaded: "No speech model loaded",
  ModelDownload: "Could not download the model",
  ModelVerification: "The download was corrupted — try again",
  Transcription: "Transcription failed",
  Injection: "Could not insert the text",
  Hotkey: "Hotkey is not bound",
  Other: "Something went wrong",
};

export function errorLabel(error: EngineError): string {
  // `Other` has nothing but its payload, so prefer that when it is there.
  if (error.kind === "Other" && error.message) return error.message;
  return ERROR_LABELS[error.kind] ?? ERROR_LABELS.Other;
}

export type EngineEvent =
  | { type: "stateChanged"; state: State }
  | { type: "audioLevel"; level: number }
  | { type: "transcript"; text: string }
  /** Advisory text while the user is still speaking. Never what gets injected. */
  | { type: "transcriptPreview"; text: string }
  | { type: "injected"; chars: number }
  | { type: "failed"; error: EngineError }
  | { type: "hotkeyChanged"; hotkey: Hotkey }
  | { type: "modelStatusChanged"; status: ModelStatus };

/** A catalog id such as `"base.en"`. See `core::models::ModelId`. */
export type ModelId = string;

/** Where the weights come from. See `core::config::ModelChoice`. */
export type ModelChoice = { kind: "catalog"; id: ModelId } | { kind: "local"; path: string };

/** One downloadable model. See `core::models::ModelInfo`. */
export type ModelInfo = {
  id: ModelId;
  label: string;
  note: string;
  bytes: number;
  sha256: string;
  url: string;
  englishOnly: boolean;
};

/**
 * What the model subsystem is doing. See `core::event::ModelStatus`.
 *
 * Separate from `State` because the two run concurrently: a download can be in
 * flight while the user dictates on the model they already have.
 */
export type ModelStatus =
  | { status: "absent" }
  | { status: "downloading"; id: ModelId; received: number; total: number }
  | { status: "ready"; label: string }
  | { status: "failed"; error: EngineError };

export type InjectionMode = "synthetic" | "paste";

export type ModifierKey =
  | "leftOption"
  | "rightOption"
  | "leftCommand"
  | "rightCommand"
  | "leftControl"
  | "rightControl"
  | "leftShift"
  | "rightShift"
  | "fn";

/** Side-agnostic, unlike `ModifierKey`. See `core::hotkey::ChordMods`. */
export type ChordMods = {
  command: boolean;
  option: boolean;
  control: boolean;
  shift: boolean;
};

/** A non-modifier key. See `core::hotkey::KeyCode`. */
export type KeyCode = string;

/** A held modifier, or a conventional chord. See `core::config::Hotkey`. */
export type Hotkey =
  | { kind: "modifier"; key: ModifierKey }
  | { kind: "chord"; mods: ChordMods; key: KeyCode };

/** Hold the key, or tap it twice. See `core::config::ActivationMode`. */
export type ActivationMode = "pushToTalk" | "toggle";

/** Physical screen pixels. See `core::config::WidgetPosition`. */
export type WidgetPosition = { x: number; y: number };

export type Config = {
  hotkey: Hotkey;
  activation: ActivationMode;
  model: ModelChoice;
  language: string | null;
  injectionMode: InjectionMode;
  minRecordingMs: number;
  transcriptPreview: boolean;
  widgetPosition: WidgetPosition | null;
};

const MODIFIER_LABELS: Record<ModifierKey, string> = {
  leftOption: "Left Option",
  rightOption: "Right Option",
  leftCommand: "Left Command",
  rightCommand: "Right Command",
  leftControl: "Left Control",
  rightControl: "Right Control",
  leftShift: "Left Shift",
  rightShift: "Right Shift",
  fn: "Fn",
};

/** How a key name is written. Mirrors `core::hotkey::KeyCode::label`. */
const KEY_LABELS: Record<string, string> = {
  space: "Space",
  return: "Return",
  escape: "Escape",
  tab: "Tab",
  delete: "Delete",
  left: "←",
  right: "→",
  up: "↑",
  down: "↓",
};

/**
 * Mirrors `Hotkey::label` in Rust, which renders the same binding for the tray.
 * The two must agree: seeing one name in the menu bar and another in settings
 * reads as two different bindings.
 */
export function hotkeyLabel(hotkey: Hotkey): string {
  if (hotkey.kind === "modifier") return MODIFIER_LABELS[hotkey.key];

  // macOS order, regardless of how the chord was typed.
  const symbols =
    (hotkey.mods.control ? "⌃" : "") +
    (hotkey.mods.option ? "⌥" : "") +
    (hotkey.mods.shift ? "⇧" : "") +
    (hotkey.mods.command ? "⌘" : "");

  const key = hotkey.key;
  // Digits serialise as `digit4`; letters and function keys are already their
  // own names once upper-cased.
  const name =
    KEY_LABELS[key] ?? (key.startsWith("digit") ? key.slice(5) : key.toUpperCase());

  return symbols + name;
}

/** How to describe the binding to the user, mode included. */
export function activationLabel(config: Config): string {
  const key = hotkeyLabel(config.hotkey);
  return config.activation === "toggle" ? `tap ${key} to start and stop` : `hold ${key} to talk`;
}

/**
 * What to tell the user while recording.
 *
 * Toggle mode needs this: nothing about a widget that appeared on its own says
 * how to make it stop. Hold mode gets the matching line for symmetry, though
 * there the answer is already in the user's fingers.
 */
export function stopHint(config: Config): string {
  const key = hotkeyLabel(config.hotkey);
  return config.activation === "toggle" ? `Tap ${key} to stop` : `Release ${key} to insert`;
}

/** Bands in `getAudioSpectrum`. Mirrors `core::SPECTRUM_BANDS`. */
export const SPECTRUM_BANDS = 12;

const EVENT = "speakeasy://event";

export const getState = () => invoke<State>("state");
export const getConfig = () => invoke<Config>("config");
export const setConfig = (config: Config) => invoke<void>("set_config", { config });
export const getAudioLevel = () => invoke<number>("audio_level");
/** `SPECTRUM_BANDS` values, low frequency first. See `core::ports::AudioSource`. */
export const getAudioSpectrum = () => invoke<number[]>("audio_spectrum");
export const cancel = () => invoke<void>("cancel");
export const setHotkey = (hotkey: Hotkey) => invoke<void>("set_hotkey", { hotkey });

/**
 * Arms the keyboard and returns; it does not resolve with the key.
 *
 * What was pressed comes back as a `hotkeyChanged` event, the same one a
 * rebinding from anywhere else emits — so the pane has one thing to listen to.
 */
export const captureHotkey = () => invoke<void>("capture_hotkey");
export const cancelHotkeyCapture = () => invoke<void>("cancel_hotkey_capture");

/** Retry the binding, for a grant that arrived after startup. */
export const bindHotkey = () => invoke<void>("bind_hotkey");

export const permissionsGranted = () => invoke<boolean>("permissions_granted");
export const openPermissionSettings = () => invoke<void>("open_permission_settings");

export const getModelCatalog = () => invoke<ModelInfo[]>("model_catalog");
export const getModelStatus = () => invoke<ModelStatus>("model_status");
export const getInstalledModels = () => invoke<ModelId[]>("installed_models");
export const installModel = (id: ModelId) => invoke<void>("install_model", { id });
export const selectModel = (choice: ModelChoice) => invoke<void>("select_model", { choice });
export const cancelModelDownload = () => invoke<void>("cancel_model_download");
export const removeModel = (id: ModelId) => invoke<void>("remove_model", { id });

/**
 * "488 MB" — decimal megabytes, which is how these models are advertised and
 * what makes the number match what the user reads elsewhere.
 */
export function formatBytes(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

/** Whether `choice` names this catalog entry. */
export function isSelected(choice: ModelChoice, id: ModelId): boolean {
  return choice.kind === "catalog" && choice.id === id;
}

export function onEngineEvent(handler: (event: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>(EVENT, ({ payload }) => handler(payload));
}
