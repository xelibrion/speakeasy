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

export type EngineError = {
  kind: string;
  message?: string;
};

export type EngineEvent =
  | { type: "stateChanged"; state: State }
  | { type: "audioLevel"; level: number }
  | { type: "transcript"; text: string }
  | { type: "injected"; chars: number }
  | { type: "failed"; error: EngineError };

export type InjectionMode = "synthetic" | "paste";

export type Config = {
  hotkey: string;
  modelPath: string | null;
  language: string | null;
  injectionMode: InjectionMode;
  minRecordingMs: number;
};

const EVENT = "speakeasy://event";

export const getState = () => invoke<State>("state");
export const getConfig = () => invoke<Config>("config");
export const setConfig = (config: Config) => invoke<void>("set_config", { config });
export const getAudioLevel = () => invoke<number>("audio_level");
export const cancel = () => invoke<void>("cancel");
export const permissionsGranted = () => invoke<boolean>("permissions_granted");
export const openPermissionSettings = () => invoke<void>("open_permission_settings");

export function onEngineEvent(handler: (event: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>(EVENT, ({ payload }) => handler(payload));
}
