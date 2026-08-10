/**
 * Entry point for both windows.
 *
 * One bundle serves the settings window and the floating widget; which one to
 * render is decided by the window label. That avoids a multi-page Vite build
 * for what is currently two small views.
 */
import { getCurrentWindow } from "@tauri-apps/api/window";

import "./styles.css";
import {
  activationLabel,
  errorLabel,
  getConfig,
  getState,
  onEngineEvent,
  openPermissionSettings,
  permissionsGranted,
  type EngineEvent,
} from "./engine";
import { mountWidget } from "./widget";

function root(): HTMLElement {
  const app = document.querySelector<HTMLElement>("#app");
  if (!app) throw new Error("#app missing from index.html");
  return app;
}

/**
 * Placeholder settings pane.
 *
 * Enough to prove the IPC round-trip and the event stream work end to end.
 * Keep the rule that this layer only renders what the core reports and never
 * decides anything itself.
 */
async function mountSettings(app: HTMLElement) {
  app.className = "settings";

  const log = (line: string) => {
    const entry = document.createElement("div");
    entry.textContent = line;
    app.append(entry);
  };

  const [state, config, granted] = await Promise.all([
    getState(),
    getConfig(),
    permissionsGranted(),
  ]);

  log(`state: ${state.state}`);
  log(`hotkey: ${activationLabel(config)}`);
  log(`model: ${config.modelPath ?? "(none loaded)"}`);
  log(`permissions granted: ${granted}`);

  if (!granted) {
    const button = document.createElement("button");
    button.textContent = "Open permission settings";
    button.addEventListener("click", () => void openPermissionSettings());
    app.append(button);
  }

  await onEngineEvent((event: EngineEvent) => {
    switch (event.type) {
      case "stateChanged":
        log(`→ ${event.state.state}`);
        break;
      case "transcript":
        log(`transcript: ${event.text}`);
        break;
      case "injected":
        log(`injected ${event.chars} chars`);
        break;
      case "failed":
        log(`error: ${errorLabel(event.error)} (${event.error.kind})`);
        break;
      case "audioLevel":
        break;
    }
  });
}

if (getCurrentWindow().label === "widget") {
  mountWidget(root());
} else {
  void mountSettings(root());
}
