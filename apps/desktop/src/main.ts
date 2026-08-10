/**
 * Placeholder settings UI.
 *
 * Enough to prove the IPC round-trip and the event stream work end to end.
 * Replace with a real settings pane; keep the rule that this layer only renders
 * what the core reports and never decides anything itself.
 */
import {
  getConfig,
  getState,
  hotkeyLabel,
  onEngineEvent,
  openPermissionSettings,
  permissionsGranted,
  type EngineEvent,
} from "./engine";

function root(): HTMLElement {
  const app = document.querySelector<HTMLElement>("#app");
  if (!app) throw new Error("#app missing from index.html");
  return app;
}

const log = (line: string) => {
  const entry = document.createElement("div");
  entry.textContent = line;
  root().append(entry);
};

async function main() {
  const [state, config, granted] = await Promise.all([
    getState(),
    getConfig(),
    permissionsGranted(),
  ]);

  log(`state: ${state.state}`);
  log(`hold to talk: ${hotkeyLabel(config.hotkey)}`);
  log(`model: ${config.modelPath ?? "(none loaded)"}`);
  log(`permissions granted: ${granted}`);

  if (!granted) {
    const button = document.createElement("button");
    button.textContent = "Open permission settings";
    button.addEventListener("click", () => void openPermissionSettings());
    root().append(button);
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
        log(`error: ${event.error.kind} ${event.error.message ?? ""}`);
        break;
      case "audioLevel":
        break;
    }
  });
}

void main();
