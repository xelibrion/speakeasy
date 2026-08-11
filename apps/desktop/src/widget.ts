/**
 * The floating status pill.
 *
 * Shown while an utterance is in flight. Rust decides when it is visible and
 * where it sits; this only renders what the engine reports.
 *
 * The spectrum polls rather than listening, because the engine emits state
 * transitions but not a continuous signal — polling at animation rate keeps the
 * event stream cheap.
 */
import {
  errorLabel,
  getAudioSpectrum,
  getConfig,
  getState,
  onEngineEvent,
  stopHint,
  SPECTRUM_BANDS,
  type Config,
  type State,
} from "./engine";

const LABELS: Record<State["state"], string> = {
  idle: "Ready",
  recording: "Listening…",
  transcribing: "Transcribing…",
  injecting: "Inserting…",
  failed: "Something went wrong",
};

export function mountWidget(root: HTMLElement) {
  root.className = "widget";
  // The whole pill is the drag handle: it has no controls to compete with, and
  // a smaller grip would be hard to hit on a window this size.
  root.innerHTML = `
    <div class="pill" data-tauri-drag-region>
      <div class="head">
        <div class="dot"></div>
        <div class="label">Ready</div>
      </div>
      <div class="bars">
        ${Array.from({ length: SPECTRUM_BANDS }, () => `<div class="bar"></div>`).join("")}
      </div>
      <div class="hint"></div>
      <div class="preview"></div>
    </div>
  `;

  const pill = root.querySelector<HTMLElement>(".pill")!;
  const label = root.querySelector<HTMLElement>(".label")!;
  const hint = root.querySelector<HTMLElement>(".hint")!;
  const preview = root.querySelector<HTMLElement>(".preview")!;
  const bars = Array.from(root.querySelectorAll<HTMLElement>(".bar"));

  let recording = false;
  // The hotkey and the activation mode can both change at runtime, so the hint
  // is rebuilt from config rather than baked in at mount.
  let config: Config | null = null;

  const render = (state: State) => {
    pill.dataset.state = state.state;

    if (state.state === "failed") {
      label.textContent = errorLabel(state.error);
      // The pill has room for one short line; the developer detail goes here.
      console.warn("speakeasy:", state.error.kind, state.error.message ?? "");
    } else {
      label.textContent = LABELS[state.state];
    }

    recording = state.state === "recording";
    hint.textContent = recording && config ? stopHint(config) : "";
    if (!recording) for (const bar of bars) bar.style.transform = "scaleY(0)";
    // Cleared on the way out of recording, so a discarded tap or a cancel
    // leaves no text behind to be mistaken for something that was inserted.
    if (!recording) preview.textContent = "";
  };

  // rAF pauses when the window is hidden, so this costs nothing while idle.
  const tick = async () => {
    if (recording) {
      const bands = await getAudioSpectrum();
      bars.forEach((bar, band) => {
        // A floor so silence still reads as an idle equaliser rather than a
        // row of invisible bars.
        const value = Math.max(0.06, Math.min(1, bands[band] ?? 0));
        bar.style.transform = `scaleY(${value.toFixed(3)})`;
      });
    }
    requestAnimationFrame(() => void tick());
  };

  const refreshConfig = async () => {
    config = await getConfig();
    if (recording) hint.textContent = stopHint(config);
  };

  void refreshConfig();
  void getState().then(render);
  void onEngineEvent((event) => {
    if (event.type === "transcriptPreview") {
      preview.textContent = event.text;
      // The box is clamped to three lines and the newest words are at the
      // bottom, so it has to be scrolled down to show them.
      preview.scrollTop = preview.scrollHeight;
      return;
    }
    if (event.type !== "stateChanged") return;
    // Cheap, and only once per dictation: catches a hotkey or mode changed in
    // settings since the widget was last up.
    if (event.state.state === "recording") void refreshConfig();
    render(event.state);
  });
  void tick();
}
