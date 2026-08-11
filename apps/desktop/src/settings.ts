/**
 * The settings window.
 *
 * A tab shell plus the panes that fill it. Tabs rather than one long scroll
 * because the Models pane is already six rows and a picker, and more tabs are
 * coming; stacking them would make the window's length depend on how much is in
 * each pane.
 *
 * Keep the rule this layer has always followed: render what the core reports,
 * decide nothing. Every button here is one `invoke` and no local state.
 */
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";

import {
  bindHotkey,
  cancelHotkeyCapture,
  cancelModelDownload,
  captureHotkey,
  hotkeyLabel,
  errorLabel,
  formatBytes,
  getConfig,
  getInstalledModels,
  getModelCatalog,
  getModelStatus,
  installModel,
  isSelected,
  onEngineEvent,
  openPermissionSettings,
  permissionsGranted,
  removeModel,
  selectModel,
  type Config,
  type Hotkey,
  type ModelId,
  type ModelInfo,
  type ModelStatus,
} from "./engine";

type Tab = {
  id: string;
  label: string;
  mount: (panel: HTMLElement) => void | Promise<void>;
};

/**
 * Adding a tab is one entry here plus its mount function.
 *
 * Data-driven so the shell never has to know what the tabs are — which is what
 * keeps this file from growing a branch per pane.
 */
const TABS: Tab[] = [
  { id: "models", label: "Models", mount: mountModels },
  { id: "dictation", label: "Dictation", mount: mountDictation },
];

/**
 * Torn down before the next tab mounts, so a background pane is not still
 * re-rendering on every download progress event.
 */
let releasePanel: (() => void) | undefined;

export async function mountSettings(app: HTMLElement): Promise<void> {
  app.className = "settings";
  app.replaceChildren();

  const banner = element("div", "banner");
  const tablist = element("nav", "tabs");
  tablist.setAttribute("role", "tablist");
  const panel = element("section", "panel");
  panel.setAttribute("role", "tabpanel");
  app.append(banner, tablist, panel);

  await renderPermissions(banner);

  // The window is created hidden at startup, so this webview loads before the
  // user has had any chance to grant anything — and showing the window again
  // does not remount. Without re-checking, the banner answers a question asked
  // once, at launch, and goes on claiming the grant is missing forever.
  //
  // Focus is the right moment: it is what happens on the way back from System
  // Settings, which is where the banner's own button sends people.
  void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (focused) void renderPermissions(banner);
  });

  // Paired rather than kept in parallel arrays, so the selected state cannot
  // drift out of step with the tab it describes.
  const entries = TABS.map((tab) => {
    const button = element("button", "tab");
    button.textContent = tab.label;
    button.setAttribute("role", "tab");
    button.addEventListener("click", () => void show(tab));
    return { tab, button };
  });
  tablist.append(...entries.map(({ button }) => button));

  async function show(tab: Tab): Promise<void> {
    releasePanel?.();
    releasePanel = undefined;

    for (const entry of entries) {
      entry.button.setAttribute("aria-selected", String(entry.tab.id === tab.id));
    }

    panel.replaceChildren();
    await tab.mount(panel);
  }

  // The selected tab is deliberately not persisted: `Config` is behavioural
  // state that syncs to disk on every change, and which tab someone last
  // clicked is neither.
  const first = entries[0];
  if (first) await show(first.tab);
}

/**
 * A banner rather than a tab, and only when something is wrong.
 *
 * A missing Accessibility grant is the likeliest reason a fresh install does
 * nothing at all, so it belongs above everything else — not behind a tab the
 * user has no reason to open.
 */
async function renderPermissions(banner: HTMLElement): Promise<void> {
  banner.replaceChildren();
  if (await permissionsGranted()) {
    // The grant may have only just arrived, in which case the binding failed
    // at startup and nothing has retried it. Clearing the banner without this
    // would report success above a key that still does nothing.
    await bindHotkey().catch((err: unknown) => console.warn("could not bind the hotkey", err));
    return;
  }

  const text = element("span");
  text.textContent = "Speakeasy needs Accessibility access to type for you.";

  const button = element("button");
  button.textContent = "Open permission settings";
  button.addEventListener("click", () => void openPermissionSettings());

  banner.append(text, button);
}

async function mountDictation(panel: HTMLElement): Promise<void> {
  const config = await getConfig();

  panel.append(
    await hotkeyRow(config),
    row("Insertion", config.injectionMode === "paste" ? "Paste" : "Type each character"),
    row("Language", config.language ?? "Auto-detect"),
  );

  const note = element("p", "note");
  note.textContent = "Insertion and language are not editable yet. Edit config.json to change them.";
  panel.append(note);
}

/**
 * The binding, and a button that reads the next key pressed.
 *
 * No validity check here on purpose: whether a key can be bound is the
 * platform's answer, and a second opinion in the webview is exactly the kind of
 * drift this codebase is arranged to avoid. The pane renders what comes back.
 */
async function hotkeyRow(config: Config): Promise<HTMLElement> {
  const wrapper = element("div", "setting-block");
  const line = element("div", "setting");
  const name = element("span", "setting-label");
  name.textContent = "Hotkey";
  const value = element("span", "setting-value");
  const action = document.createElement("button");
  line.append(name, value, action);

  const problem = element("div", "setting-error");
  wrapper.append(line, problem);

  let listening = false;

  const draw = (hotkey: Hotkey) => {
    value.textContent = listening ? "Press a key…" : hotkeyLabel(hotkey);
    action.textContent = listening ? "Cancel" : "Change";
  };

  let current = config.hotkey;
  draw(current);

  action.addEventListener("click", () => {
    void (async () => {
      problem.textContent = "";
      if (listening) {
        await cancelHotkeyCapture();
        listening = false;
        draw(current);
        return;
      }

      listening = true;
      draw(current);
      try {
        await captureHotkey();
      } catch (err) {
        // The likeliest cause is a missing Accessibility grant, which the
        // banner above already explains how to fix.
        listening = false;
        draw(current);
        problem.textContent = String(err);
      }
    })();
  });

  // The capture reports through the ordinary rebinding event, so this is also
  // what keeps the row honest if the binding changes from anywhere else.
  const unlisten = await onEngineEvent((event) => {
    if (event.type !== "hotkeyChanged") return;
    listening = false;
    current = event.hotkey;
    draw(current);
  });

  // A capture swallows keystrokes. Leaving one armed behind a tab the user has
  // navigated away from would half-deaden the keyboard with nothing on screen
  // to explain it.
  releasePanel = () => {
    unlisten();
    if (listening) void cancelHotkeyCapture();
  };

  return wrapper;
}

async function mountModels(panel: HTMLElement): Promise<void> {
  const [catalog, config, installed, status] = await Promise.all([
    getModelCatalog(),
    getConfig(),
    getInstalledModels(),
    getModelStatus(),
  ]);

  const state = { config, installed: new Set(installed), status };
  const list = element("div", "models");
  const rows = new Map<ModelId, HTMLElement>();

  for (const info of catalog) {
    const row = element("div", "model");
    rows.set(info.id, row);
    list.append(row);
  }
  panel.append(list, localFileRow());

  const draw = (info: ModelInfo) => {
    const row = rows.get(info.id);
    if (row) renderModel(row, info, state);
  };
  catalog.forEach(draw);

  const unlisten = await onEngineEvent((event) => {
    if (event.type !== "modelStatusChanged") return;
    state.status = event.status;

    // Redraw only the rows whose control can have changed, rather than the
    // whole list: a full re-render on every progress tick would fight the
    // user's scroll position and lose focus from the button they are on.
    void refreshAfterStatus(state, catalog, draw);
  });
  releasePanel = unlisten;

  function localFileRow(): HTMLElement {
    const row = element("div", "model local");
    const label = element("div", "model-label");
    label.textContent = "Use a local file…";

    const note = element("div", "model-note");
    note.textContent =
      state.config.model.kind === "local" ? state.config.model.path : "A ggml .bin you already have";
    row.append(label, note);

    const button = element("button");
    button.textContent = "Choose…";
    button.addEventListener("click", () => {
      void (async () => {
        const picked = await open({
          multiple: false,
          directory: false,
          filters: [{ name: "Model", extensions: ["bin"] }],
        });
        if (typeof picked !== "string") return;
        await selectModel({ kind: "local", path: picked });
        state.config = await getConfig();
        note.textContent = picked;
        catalog.forEach(draw);
      })();
    });

    row.append(button);
    return row;
  }
}

/** State shared by every model row, so a redraw needs no extra IPC. */
type ModelsState = {
  config: Config;
  installed: Set<ModelId>;
  status: ModelStatus;
};

/**
 * A download that ended changed both what is installed and what is selected,
 * neither of which the status event carries.
 */
async function refreshAfterStatus(
  state: ModelsState,
  catalog: ModelInfo[],
  draw: (info: ModelInfo) => void,
): Promise<void> {
  if (state.status.status !== "downloading") {
    const [config, installed] = await Promise.all([getConfig(), getInstalledModels()]);
    state.config = config;
    state.installed = new Set(installed);
  }
  catalog.forEach(draw);
}

function renderModel(row: HTMLElement, info: ModelInfo, state: ModelsState): void {
  row.replaceChildren();

  const label = element("div", "model-label");
  label.textContent = info.englishOnly ? info.label : `${info.label} · multilingual`;

  const note = element("div", "model-note");
  note.textContent = `${info.note} · ${formatBytes(info.bytes)}`;

  row.append(label, note);

  const downloading = state.status.status === "downloading" && state.status.id === info.id;
  const failed = state.status.status === "failed" && isSelected(state.config.model, info.id);

  if (downloading && state.status.status === "downloading") {
    const bar = document.createElement("progress");
    bar.max = state.status.total || 1;
    bar.value = state.status.received;
    row.append(bar, button("Cancel", () => void cancelModelDownload()));
    return;
  }

  if (failed && state.status.status === "failed") {
    const problem = element("div", "model-error");
    problem.textContent = errorLabel(state.status.error);
    row.append(problem, button("Retry", () => void installModel(info.id)));
    return;
  }

  if (!state.installed.has(info.id)) {
    row.append(button("Download", () => void installModel(info.id)));
    return;
  }

  if (isSelected(state.config.model, info.id)) {
    const inUse = button("In use", () => {});
    inUse.disabled = true;
    row.append(inUse);
    return;
  }

  row.append(
    button("Use", () => void selectModel({ kind: "catalog", id: info.id })),
    button("Remove", () => void removeModel(info.id), "secondary"),
  );
}

function button(text: string, onClick: () => void, className = ""): HTMLButtonElement {
  const element = document.createElement("button");
  element.textContent = text;
  if (className) element.className = className;
  element.addEventListener("click", onClick);
  return element;
}

function row(label: string, value: string): HTMLElement {
  const row = element("div", "setting");
  const name = element("span", "setting-label");
  name.textContent = label;
  const detail = element("span", "setting-value");
  detail.textContent = value;
  row.append(name, detail);
  return row;
}

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = "",
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  return node;
}
