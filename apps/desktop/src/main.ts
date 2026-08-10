/**
 * Entry point for both windows.
 *
 * One bundle serves the settings window and the floating widget; which one to
 * render is decided by the window label. That avoids a multi-page Vite build
 * for what is currently two small views.
 */
import { getCurrentWindow } from "@tauri-apps/api/window";

import "./styles.css";
import { mountSettings } from "./settings";
import { mountWidget } from "./widget";

function root(): HTMLElement {
  const app = document.querySelector<HTMLElement>("#app");
  if (!app) throw new Error("#app missing from index.html");
  return app;
}

if (getCurrentWindow().label === "widget") {
  mountWidget(root());
} else {
  void mountSettings(root());
}
