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
  activeKeyboardBucket,
  cancel,
  copyTranscript,
  errorLabel,
  getAudioSpectrum,
  getConfig,
  getRecoverableTranscript,
  getState,
  onEngineEvent,
  stopHint,
  SPECTRUM_BANDS,
  type Config,
  type KeyboardBucket,
  type State,
} from "./engine";

const LABELS: Record<State["state"], string> = {
  idle: "Ready",
  recording: "Listening…",
  transcribing: "Transcribing…",
  injecting: "Inserting…",
  // Not "failed": the text may well have arrived. All that is certain is that
  // nothing focused looked able to take it.
  unconfirmed: "Couldn't confirm the text was inserted",
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
      <div class="rescue">
        <button class="copy">Copy</button>
        <button class="dismiss secondary">Dismiss</button>
      </div>
    </div>
  `;

  const pill = root.querySelector<HTMLElement>(".pill")!;
  const label = root.querySelector<HTMLElement>(".label")!;
  const hint = root.querySelector<HTMLElement>(".hint")!;
  const preview = root.querySelector<HTMLElement>(".preview")!;
  const rescue = root.querySelector<HTMLElement>(".rescue")!;
  const copyButton = root.querySelector<HTMLButtonElement>(".copy")!;
  const dismissButton = root.querySelector<HTMLButtonElement>(".dismiss")!;
  const bars = Array.from(root.querySelectorAll<HTMLElement>(".bar"));

  // Clicks land even though the panel refuses key status — mouse events go to
  // the window under the cursor regardless; it is only keyboard focus that is
  // refused. That also rules out dismissing with Escape: nothing here will ever
  // receive a keystroke, so the way out has to be a button.
  copyButton.addEventListener("click", () => {
    void (async () => {
      const copied = await copyTranscript().catch((err: unknown) => {
        console.warn("speakeasy: could not copy the transcript", err);
        return false;
      });
      // The pill stays up. Copying is not an acknowledgement that the text
      // arrived somewhere useful, and closing on it would take the second
      // chance away from anyone who pasted into the wrong window.
      copyButton.textContent = copied ? "Copied" : "Nothing to copy";
    })();
  });
  dismissButton.addEventListener("click", () => void cancel());

  let recording = false;
  // The hotkey and the activation mode can both change at runtime, so the hint
  // is rebuilt from config rather than baked in at mount.
  let config: Config | null = null;
  let bucket: KeyboardBucket = "builtIn";

  const currentHint = () => (config ? stopHint(config.hotkeys[bucket], config.activation) : "");

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
    hint.textContent = recording ? currentHint() : "";
    if (!recording) for (const bar of bars) bar.style.transform = "scaleY(0)";
    // Cleared on the way out of recording, so a discarded tap or a cancel
    // leaves no text behind to be mistaken for something that was inserted.
    if (!recording) preview.textContent = "";

    // Last, and deliberately after that clear: on a failure this puts the
    // stranded transcript back into the same box, which is the one case where
    // text surviving the end of a recording is the point.
    //
    // Offered whenever the failure left a transcript behind, which is what
    // separates a recoverable failure from a total one — secure input caught
    // before recording has nothing to copy, secure input that arrived
    // mid-dictation has a whole paragraph.
    void showRescue(state);
  };

  /**
   * Show the rescue row on any failure, and the Copy button within it only
   * when the failure left a transcript stranded.
   *
   * Dismiss must stay reachable even with nothing to copy — a check caught
   * before recording started (secure input, say) leaves the pill up with no
   * other way off screen, since the panel never takes keyboard focus and so
   * can never be dismissed with Escape.
   *
   * Asks the engine rather than remembering the last `transcript` event: the
   * widget is a renderer, and a copy of the text kept here would be a second
   * source of truth for the one string in this app that cannot be regenerated.
   */
  async function showRescue(state: State): Promise<void> {
    if (state.state !== "failed" && state.state !== "unconfirmed") {
      rescue.hidden = true;
      return;
    }

    rescue.hidden = false;
    const text = await getRecoverableTranscript().catch(() => null);
    // `.preview:empty` already collapses the box when there is nothing to
    // show; `render` clears it on the way into every non-recording state.
    copyButton.hidden = text === null;
    if (text === null) return;

    copyButton.textContent = "Copy";
    // Showing what would be copied, so the button is not a leap of faith.
    preview.textContent = text;
    preview.scrollTop = 0;
  }

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
    [config, bucket] = await Promise.all([getConfig(), activeKeyboardBucket()]);
    if (recording) hint.textContent = currentHint();
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
