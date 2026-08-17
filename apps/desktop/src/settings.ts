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
  activeKeyboardBucket,
  bindHotkey,
  cancelHotkeyCapture,
  cancelModelDownload,
  clearHistory,
  copyHistoryEntry,
  captureHotkey,
  hotkeyLabel,
  errorLabel,
  formatBytes,
  getConfig,
  setConfig,
  getHistory,
  getInstalledModels,
  getModelCatalog,
  getModelStatus,
  installModel,
  isSelected,
  onEngineEvent,
  openPermissionSettings,
  permissionsGranted,
  relativeTime,
  removeModel,
  selectModel,
  type Config,
  type Hotkey,
  type KeyboardBucket,
  type ModelId,
  type ModelInfo,
  type ModelStatus,
  type Transcript,
} from "./engine";

type Tab = {
  id: string;
  label: string;
  /**
   * Raw SVG markup, 16x16, single `currentColor` stroke. No icon asset
   * pipeline exists yet, so glyphs are inline rather than imported SF
   * Symbols; every value here is authored by us, never user input, so
   * setting it via `innerHTML` is safe.
   */
  icon: string;
  mount: (panel: HTMLElement) => void | Promise<void>;
};

/**
 * Adding a tab is one entry here plus its mount function.
 *
 * Data-driven so the shell never has to know what the tabs are — which is what
 * keeps this file from growing a branch per pane.
 */
const TABS: Tab[] = [
  {
    id: "dictation",
    label: "Dictation",
    icon: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M8 2a2 2 0 0 1 2 2v4a2 2 0 1 1-4 0V4a2 2 0 0 1 2-2Z"/><path d="M4 8a4 4 0 0 0 8 0"/><path d="M8 12v2"/></svg>`,
    mount: mountDictation,
  },
  {
    id: "history",
    label: "History",
    icon: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M2 4h12M2 8h12M2 12h8"/></svg>`,
    mount: mountHistory,
  },
  {
    id: "models",
    label: "Models",
    icon: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><rect x="2" y="2" width="5" height="5" rx="1"/><rect x="9" y="2" width="5" height="5" rx="1"/><rect x="2" y="9" width="5" height="5" rx="1"/><rect x="9" y="9" width="5" height="5" rx="1"/></svg>`,
    mount: mountModels,
  },
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
    button.innerHTML = tab.icon;
    const text = document.createElement("span");
    text.textContent = tab.label;
    button.append(text);
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
  const activeBucket = await activeKeyboardBucket();

  const builtIn = await hotkeyRow(config, "builtIn", "Built-in keyboard", activeBucket);
  const external = await hotkeyRow(config, "external", "External keyboard", activeBucket);

  const primary = element("div", "card");
  primary.append(
    builtIn.element,
    external.element,
    toggleRow("Live preview", config.transcriptPreview, (value) => {
      void setConfig({ ...config, transcriptPreview: value });
    }),
  );

  const groupLabel = element("div", "grouplabel");
  groupLabel.textContent = "Text insertion";

  const secondary = element("div", "card");
  secondary.append(
    row("Insertion", config.injectionMode === "paste" ? "Paste" : "Type each character"),
    row("Language", config.language ?? "Auto-detect"),
  );

  panel.append(primary, groupLabel, secondary);

  const note = element("p", "note");
  note.textContent = "Insertion and language are not editable yet. Edit config.json to change them.";
  panel.append(note);

  releasePanel = () => {
    builtIn.release();
    external.release();
  };
}

type HotkeyRow = { element: HTMLElement; release: () => void };

/**
 * One binding — built-in or external — and a button that reads the next key
 * pressed.
 *
 * No validity check here on purpose: whether a key can be bound is the
 * platform's answer, and a second opinion in the webview is exactly the kind of
 * drift this codebase is arranged to avoid. The pane renders what comes back.
 *
 * `initialActive` is a one-time snapshot for the first paint; the row keeps
 * itself honest afterward via `activeBucketChanged`, the same way it already
 * does for `hotkeyChanged`.
 */
async function hotkeyRow(
  config: Config,
  bucket: KeyboardBucket,
  label: string,
  initialActive: KeyboardBucket,
): Promise<HotkeyRow> {
  const wrapper = element("div", "setting-block");
  const line = element("div", "setting");
  const name = element("span", "setting-label");
  name.textContent = label;
  const active = element("span", "setting-active");
  active.textContent = "Active";
  const value = element("span", "setting-value");
  const action = document.createElement("button");
  line.append(name, active, value, action);

  const problem = element("div", "setting-error");
  wrapper.append(line, problem);

  let listening = false;
  let isActive = bucket === initialActive;

  const draw = (hotkey: Hotkey) => {
    value.textContent = listening ? "Press a key…" : hotkeyLabel(hotkey);
    action.textContent = listening ? "Cancel" : "Change";
    active.hidden = !isActive;
  };

  let current = config.hotkeys[bucket];
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
  // what keeps the row honest if the binding changes from anywhere else. A
  // capture always writes into whichever bucket was active when it started
  // (see the shell's `hotkey::capture`), so only react to a change for this
  // row's own bucket.
  const unlisten = await onEngineEvent((event) => {
    if (event.type === "hotkeyChanged" && event.bucket === bucket) {
      listening = false;
      current = event.hotkey;
      draw(current);
      return;
    }
    if (event.type === "activeBucketChanged") {
      isActive = event.bucket === bucket;
      draw(current);
    }
  });

  // A capture swallows keystrokes. Leaving one armed behind a tab the user has
  // navigated away from would half-deaden the keyboard with nothing on screen
  // to explain it.
  const release = () => {
    unlisten();
    if (listening) void cancelHotkeyCapture();
  };

  return { element: wrapper, release };
}

async function mountModels(panel: HTMLElement): Promise<void> {
  const [catalog, config, installed, status] = await Promise.all([
    getModelCatalog(),
    getConfig(),
    getInstalledModels(),
    getModelStatus(),
  ]);

  const state = { config, installed: new Set(installed), status };
  const list = element("div", "models card");
  const rows = new Map<ModelId, HTMLElement>();

  for (const info of catalog) {
    const row = element("div", "model");
    rows.set(info.id, row);
    list.append(row);
  }

  const localCard = element("div", "card");
  localCard.append(localFileRow());
  panel.append(list, localCard);

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

    const actions = element("div", "model-actions");
    const choose = button("Choose…", () => {
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
    }, "outline");
    actions.append(choose);

    row.append(label, note, actions);
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
  label.textContent = modelLabel(info);

  const note = element("div", "model-note");
  note.textContent = `${info.note} · ${formatBytes(info.bytes)}`;

  const actions = element("div", "model-actions");

  const downloading = state.status.status === "downloading" && state.status.id === info.id;
  const failed = state.status.status === "failed" && isSelected(state.config.model, info.id);

  if (downloading && state.status.status === "downloading") {
    const bar = document.createElement("progress");
    bar.max = state.status.total || 1;
    bar.value = state.status.received;
    // Cancel is a way out, not the row's goal, so it stays visually quiet next
    // to the progress bar rather than competing with it.
    actions.append(bar, button("Cancel", () => void cancelModelDownload(), "ghost"));
    row.append(label, note, actions);
    return;
  }

  if (failed && state.status.status === "failed") {
    const problem = element("div", "model-error");
    problem.textContent = errorLabel(state.status.error);
    // Same tier as Download: retrying is another attempt to fetch, not the
    // more consequential act of switching the active model.
    actions.append(button("Retry", () => void installModel(info.id), "outline"));
    row.append(label, problem, actions);
    return;
  }

  if (!state.installed.has(info.id)) {
    // Outlined rather than solid: fetching a model onto disk does not change
    // what Speakeasy is using yet, so it should not look as weighty as `Use`.
    actions.append(button("Download", () => void installModel(info.id), "outline"));
    row.append(label, note, actions);
    return;
  }

  if (isSelected(state.config.model, info.id)) {
    const inUse = button("In use", () => {});
    inUse.disabled = true;
    actions.append(inUse);
    row.append(label, note, actions);
    return;
  }

  // Stacked rather than side by side: the CTA and its destructive secondary
  // read as two competing buttons when they sit shoulder to shoulder, and
  // stacking makes the size/weight difference between them the thing that
  // separates them instead.
  actions.classList.add("stacked");
  actions.append(
    // The row's one real CTA: switches what Speakeasy transcribes with.
    button("Use", () => void selectModel({ kind: "catalog", id: info.id })),
    button("Remove", () => void removeModel(info.id), "secondary"),
  );
  row.append(label, note, actions);
}

/** "Small (multilingual)" already says it; do not also append "· multilingual". */
function modelLabel(info: ModelInfo): string {
  if (info.englishOnly) return info.label;
  return info.label.toLowerCase().includes("multilingual") ? info.label : `${info.label} · multilingual`;
}

async function mountHistory(panel: HTMLElement): Promise<void> {
  const list = element("div");
  panel.append(list);

  const draw = async () => {
    const entries = await getHistory();
    list.replaceChildren();

    if (entries.length === 0) {
      const empty = element("p", "note");
      empty.textContent =
        "Recent dictations appear here so a failed or misdirected paste is recoverable. Cleared when Speakeasy quits.";
      list.append(empty);
      return;
    }

    const clearRow = element("div", "history-clear");
    clearRow.append(
      button("Clear", () => {
        void (async () => {
          await clearHistory();
          await draw();
        })();
      }, "secondary"),
    );
    list.append(clearRow);

    const card = element("div", "card");
    entries.forEach((entry, index) => card.append(historyRow(entry, index)));
    list.append(card);
  };

  await draw();

  const unlisten = await onEngineEvent((event) => {
    if (event.type !== "historyChanged") return;
    void draw();
  });
  releasePanel = unlisten;
}

function historyRow(entry: Transcript, index: number): HTMLElement {
  const row = element("div", "history-row");

  const text = element("div", "history-text");
  text.textContent = entry.text;
  text.title = entry.text;

  const meta = element("div", "history-meta");
  const time = element("span");
  time.textContent = relativeTime(entry.at);
  meta.append(time);

  // Only a genuine injection failure is marked. `Unconfirmed` deliveries are
  // recorded as delivered — see `Transcript.delivered` — because that
  // detection is known wrong for apps that draw their own text, and this
  // marker would cry wolf on rows that arrived fine.
  if (!entry.delivered) {
    const marker = element("span", "history-undelivered");
    marker.textContent = "Not delivered";
    meta.append(marker);
  }

  meta.append(button("Copy", () => void copyHistoryEntry(index)));

  row.append(text, meta);
  return row;
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

/** A labelled checkbox that writes straight back to config. */
function toggleRow(
  label: string,
  checked: boolean,
  onChange: (value: boolean) => void,
): HTMLElement {
  const line = element("div", "setting");
  const name = element("span", "setting-label");
  name.textContent = label;

  const box = document.createElement("input");
  box.type = "checkbox";
  box.className = "switch";
  box.checked = checked;
  box.addEventListener("change", () => onChange(box.checked));

  line.append(name, box);
  return line;
}

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = "",
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  return node;
}
