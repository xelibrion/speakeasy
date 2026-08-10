/**
 * The floating status pill.
 *
 * Shown while an utterance is in flight. Rust decides when it is visible and
 * where it sits; this only renders what the engine reports.
 *
 * The level meter polls rather than listening, because the engine emits state
 * transitions but not a continuous level — polling at animation rate keeps the
 * event stream cheap.
 */
import { errorLabel, getAudioLevel, getState, onEngineEvent, type State } from "./engine";

const LABELS: Record<State["state"], string> = {
  idle: "Ready",
  recording: "Listening…",
  transcribing: "Transcribing…",
  injecting: "Inserting…",
  failed: "Something went wrong",
};

export function mountWidget(root: HTMLElement) {
  root.className = "widget";
  root.innerHTML = `
    <div class="pill">
      <div class="dot"></div>
      <div class="body">
        <div class="label">Ready</div>
        <div class="meter"><div class="fill"></div></div>
      </div>
    </div>
  `;

  const pill = root.querySelector<HTMLElement>(".pill")!;
  const label = root.querySelector<HTMLElement>(".label")!;
  const fill = root.querySelector<HTMLElement>(".fill")!;

  let recording = false;

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
    if (!recording) fill.style.transform = "scaleX(0)";
  };

  // rAF pauses when the window is hidden, so this costs nothing while idle.
  const tick = async () => {
    if (recording) {
      const level = await getAudioLevel();
      // Speech rarely exceeds ~0.3 RMS; scale so the meter uses its range.
      fill.style.transform = `scaleX(${Math.min(1, level * 3).toFixed(3)})`;
    }
    requestAnimationFrame(() => void tick());
  };

  void getState().then(render);
  void onEngineEvent((event) => {
    if (event.type === "stateChanged") render(event.state);
  });
  void tick();
}
