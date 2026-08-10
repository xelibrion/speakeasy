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
  | { type: "injected"; chars: number }
  | { type: "failed"; error: EngineError };

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

/** A held modifier, or a conventional chord. See `core::config::Hotkey`. */
export type Hotkey =
  | { kind: "modifier"; key: ModifierKey }
  | { kind: "accelerator"; accelerator: string };

/** Hold the key, or tap it twice. See `core::config::ActivationMode`. */
export type ActivationMode = "pushToTalk" | "toggle";

/** Physical screen pixels. See `core::config::WidgetPosition`. */
export type WidgetPosition = { x: number; y: number };

export type Config = {
  hotkey: Hotkey;
  activation: ActivationMode;
  modelPath: string | null;
  language: string | null;
  injectionMode: InjectionMode;
  minRecordingMs: number;
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

export function hotkeyLabel(hotkey: Hotkey): string {
  return hotkey.kind === "modifier" ? MODIFIER_LABELS[hotkey.key] : hotkey.accelerator;
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
export const permissionsGranted = () => invoke<boolean>("permissions_granted");
export const openPermissionSettings = () => invoke<void>("open_permission_settings");

export function onEngineEvent(handler: (event: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>(EVENT, ({ payload }) => handler(payload));
}
