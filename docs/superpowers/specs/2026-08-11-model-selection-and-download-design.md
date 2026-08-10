# Model selection and download

Date: 2026-08-11
Status: approved, not yet implemented

## Problem

Speakeasy has no way to choose a speech model. `Config::model_path` is a raw
string that the user must fill in by hand or via `SPEAKEASY_MODEL`, weights are
loaded exactly once during `main`, and there is no way to change them without
restarting. A fresh install starts with `model_path: None`, so the first press
of the hotkey fails with `ModelNotLoaded` and nothing in the app explains where
a model comes from. The README already lists "model download and first-run
flow" as unbuilt.

This design adds a curated catalog of downloadable models, a local-file escape
hatch, download progress visible from the menu bar, and switching models
without a restart.

## Goals

- Pick a model from a curated catalog and have the app fetch it.
- Point at a `.bin` already on disk.
- Auto-download a sensible default on first run, so a fresh install dictates
  without the user making a decision.
- Switch models at runtime with no restart.
- Show download progress in the tray and in settings.
- Keep `speakeasy-core` free of a network stack, and keep every new code path
  testable offline.

## Non-goals

- Resume-from-offset on an interrupted download. A partial file is deleted and
  a retry starts over. Range requests mean trusting a partial file across
  restarts and hashing to find out, and the payoff is limited to the two
  largest catalog entries.
- Concurrent downloads, mirrors, or proxy configuration.
- Checking whether a published model file has been updated.
- The full quantized-variant matrix. A handful of representative models only.
- Auto-selecting a model from the `language` setting.
- Editors for the other settings (hotkey, activation, injection mode,
  language). They render read-only; making them editable is separate work.

## Architecture

Three options were considered for where downloading lives.

**Shell owns downloading.** Tauri fetches, writes the file, tells the engine to
load it. Core barely changes, but the catalog, progress, verification, retry
and cancel all live in the shell, so a second shell reimplements every bit of
it. Progress is the worst part: it would originate in the shell and either be
fed back into core or emitted as a separate webview event, forking the event
stream so the tray has two sources of truth. This is the logic-escaping-the-core
failure that `commands.rs` warns about in its own doc comment.

**Core owns everything including HTTP.** The simplest call graph, and progress
is trivial because core owns the read loop. But `speakeasy-core` currently
depends on serde and thiserror only; adding `ureq` or `reqwest` puts TLS into
the crate that is supposed to be pure logic. Worse, core's tests run offline and
finish instantly today, and a download path baked into core needs a local test
server, an `#[ignore]`d network test, or no test at all — for a flow whose
interesting branches (hash mismatch, cancel mid-download, a swap that fails)
are exactly the ones worth testing.

**Catalog and orchestration in core, transport behind a port.** Chosen. This is
the shape the workspace already uses twice: `Transcriber` is a port with
`speakeasy-whisper` behind it, `AudioSource` is a port with `speakeasy-platform`
behind it. `make boundary` already enforces in CI that core's dependencies are
a design decision rather than an accident. Progress crosses the port as a
callback and is re-emitted by core as an ordinary event, which is invisible to
every consumer.

The cost is one new crate, `speakeasy-models`: a `Cargo.toml` and one file
implementing one trait. That is cheaper than the feature-gating the second
option would need to stay testable.

## Config

`model_path: Option<String>` is replaced by a two-shape choice:

```rust
pub model: ModelChoice,

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ModelChoice {
    Catalog { id: ModelId },
    Local { path: String },
}
```

`ModelId` is a `String` newtype, not an enum. An enum means a config written by
a newer build with an unknown model id fails to deserialize, and `Config::load`
treats malformed JSON as an error — so the app would fall back to defaults and
drop every other setting the user had. A string id degrades to "that model is
not in my catalog", which is recoverable.

The default is `Catalog { id: "base.en" }`.

### Desired state versus actual state

`config.model` is **desired state**: what the user picked. It is persisted, and
written the moment they choose. It does not necessarily name a model that is
installed — on a fresh install it names `base.en` before anything is on disk.

`ModelStatus` (below) is **actual state**: what is loaded or downloading right
now. It lives in memory, is never persisted, and is rebuilt at launch.

`ensure_model()` reconciles the two. First run stops being a special case: a
fresh config wants `base.en`, nothing is installed, so it downloads. Identical
machinery to a user switching models — there is no separate first-run path to
write or test.

Three consequences:

- **Quitting mid-download resumes on the next launch.** Config already names
  the wanted model, so `ensure_model` picks the download back up (from zero).
  The user's intent survives a restart, which it would not if config only
  recorded successful loads.
- **Cancel reverts.** Cancelling writes `config.model` back to whatever is
  currently loaded. Cancel means "never mind", and without the revert the app
  would resume the abandoned download on the next launch.
- **A failed download does not revert, but does fall back.** Config keeps
  naming the wanted model, so a retry is one click and the next launch tries
  again. But `ensure_model` will not leave dictation dead: if the desired model
  cannot be had and another model is installed, it loads that one, with
  `ModelStatus::Failed` explaining why. Transcribing on an unexpected model
  while saying so beats not transcribing.

Nothing else about the model is persisted. The on-disk path is derivable from
the id plus the models directory, and storing both is how they drift.

### Migration

`model_path` stays in the struct as a deprecated `Option<String>`.
`Config::load` normalizes: if `model_path` is set and `model` is absent, it
becomes `Local { path }`, and `model_path` is cleared on the next save. Existing
configs keep working. `SPEAKEASY_MODEL` keeps overriding at startup and maps to
`Local` too, which is what `tests/transcribe.rs` and manual pipeline checks
want.

## Catalog

Static data in `core::models`:

```rust
pub struct ModelInfo {
    pub id: ModelId,
    pub label: &'static str,      // "Base (English)"
    pub note: &'static str,       // "Fast, accurate enough for dictation."
    pub bytes: u64,
    pub sha256: &'static str,
    pub url: &'static str,
    pub english_only: bool,
}
```

Roughly six entries from the whisper.cpp ggml releases: `tiny.en`, `base.en`,
`small.en`, `medium.en`, plus multilingual `small` and `large-v3-turbo` so the
existing `language` setting has something to be useful with.

`bytes` and `sha256` are catalog data rather than discovered from the response.
That is what lets the UI say "466 MB" before the user commits, and what makes a
truncated or tampered download detectable instead of surfacing later as a
confusing whisper.cpp load error.

**Open item for implementation:** the exact byte counts and SHA-256 digests must
be taken from the real published files. Do not guess them — a wrong digest turns
every download into a verification failure.

## Ports

A fifth port alongside the existing four:

```rust
pub trait ModelFetcher: Send + Sync {
    /// Stream `url` into `destination`, calling `progress(received, total)`.
    /// Must return promptly once `cancel` is set.
    fn fetch(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()>;
}
```

Blocking, like `Transcriber::transcribe` — same contract, same "call it off the
UI thread" rule. Cancellation is a shared flag rather than a returned handle so
the port stays a single function; the engine owns the flag.

`Transcriber` gains one method:

```rust
fn load(&self, path: &Path) -> Result<()>;
```

`WhisperTranscriber` implements it by building the new `WhisperContext` *before*
taking the lock, then swapping it into the `Mutex<Option<WhisperContext>>` it
already holds. That ordering matters: a model that fails to load leaves the
previous one working, so a bad swap costs the user nothing. `is_ready()` needs
no change.

## ModelManager

Lives in `core::models` rather than on `Engine`. `engine.rs` is already 564
lines, and this is a self-contained unit with its own tests.

```rust
fn path_for(&self, choice: &ModelChoice) -> Option<PathBuf>
fn installed(&self) -> Vec<ModelId>
fn install(&self, id: &ModelId) -> Result<PathBuf>   // blocking
fn cancel(&self)
fn remove(&self, id: &ModelId) -> Result<()>
```

`install` is: download to `ggml-<id>.bin.part` → verify SHA-256 → atomic rename
→ `transcriber.load`. A hash mismatch deletes the partial file and reports
failure; it never renames.

Because the swap is the last step, **the currently loaded model keeps working
for the whole download**. Switching models never costs a dictation, and a failed
switch is a no-op rather than a broken app.

One download at a time. A second `install` while one is in flight is refused
rather than queued or silently replacing the first; the UI has an explicit
Cancel button, which is clearer than a click that quietly abandons a 1.5 GB
transfer.

Storage is `<config dir>/models/`, a sibling of `config.json`, so it survives
reinstalling the app. The repo's existing `models/` directory stays a dev-only
thing. The engine learns the location through a chained `storing_models_in(dir)`,
matching the existing `persisting_to`.

## Status and events

```rust
pub enum ModelStatus {
    Absent,
    Downloading { id: ModelId, received: u64, total: u64 },
    Ready { id: ModelId },
    Failed { id: ModelId, error: Error },
}
```

Deliberately **not** folded into `State`. `State` is the dictation cycle, and
the two are genuinely concurrent — the user can be recording on the current
model while the next one downloads. A single enum would make those mutually
exclusive and would corrupt what the widget means.

One new event, mirroring `StateChanged`:

```rust
ModelStatusChanged { status: ModelStatus }
```

Not three separate progress/ready/failed events. The status enum already carries
everything, the throttle below applies naturally to its `Downloading` variant,
and every consumer has one thing to switch on.

**Progress events are throttled to at most one per 250 ms.** `bridge.rs` does a
`run_on_main_thread` hop per event it forwards to the tray; a 1.6 GB download
emitting per chunk would hammer the main thread with thousands of redundant
updates. The throttle lives in `ModelManager` so every shell inherits it, and
its interval is injectable so tests do not have to sleep.

## Engine surface

```rust
fn catalog(&self) -> &'static [ModelInfo]
fn installed_models(&self) -> Vec<ModelId>
fn model_status(&self) -> ModelStatus
fn install_model(&self, id: ModelId)          // returns immediately
fn select_model(&self, choice: ModelChoice)   // no download
fn cancel_model_download(&self)
fn remove_model(&self, id: &ModelId) -> Result<()>
fn ensure_model(&self)                        // startup reconciliation
```

`install_model` spawns its own thread and returns immediately, unlike
`finish_recording`. There is no return value worth waiting for — every outcome
arrives as `ModelStatusChanged` — and this keeps the IPC commands to the
one-line delegations `commands.rs` demands. `Engine` is already `Arc`-backed and
`Clone`, so the spawn is free.

`select_model` covers the no-download cases: a local file, or a catalog model
already on disk.

`ensure_model` is the whole first-run story and it lives in core, not in
`main.rs`: resolve `config.model`; if the file is on disk, load it; if it is a
catalog entry that is not, start downloading it; if that cannot be had, fall
back to any installed model. `main.rs` calls it once during setup.

That also lets `main.rs` shed its current model-loading branch — the shell
constructs `WhisperTranscriber::unloaded()` unconditionally and lets core
decide, which simplifies code that is already awkward.

## Errors

Two new variants:

- `ModelDownload(String)` — transport failure.
- `ModelVerification { id }` — the bytes arrived but the digest did not match.
  Separate because it has a specific remedy (retry) that a generic network
  failure does not.

Cancelling is not an error: status returns to `Ready` if a model was already
loaded, `Absent` if not.

Both kinds need entries in the TypeScript `ERROR_LABELS` map in `engine.ts`.

## Shell

**IPC** adds `model_catalog`, `model_status`, `installed_models`,
`install_model`, `select_model`, `cancel_model_download`, `remove_model` — all
one-line delegations.

**Tray** gains a line fed by `ModelStatusChanged`, forwarded through `bridge.rs`
with the same `run_on_main_thread` hop `StateChanged` already uses. When status
is `Downloading` or `Failed` that line wins; otherwise the tray shows the
dictation state as it does today. Downloading is the more urgent and more
transient thing to say, and while it is happening the dictation state is almost
always `Idle` anyway.

The widget is untouched. Its whole identity is "you are dictating right now",
and reusing it for downloads dilutes that.

**Settings window.** The debug log is removed. In its place a plain window frame
with sections; only **Models** is interactive. Hotkey, activation, injection
mode and language render as read-only rows, so the window is honest about what
it currently does and the sections exist as slots for later work. Nothing is
lost by dropping the log — the event stream is still visible in the console and
in tracing, which is where anyone debugging it looks.

The Models pane is one row per catalog entry, ordered smallest to largest (which
doubles as a rough speed-versus-accuracy ordering). Each row shows label, note,
size, and exactly one control depending on state: **Download** / progress bar +
**Cancel** / **Use** / **In use** / **Remove**. Below them, a **Use a local
file…** row with a picker.

That picker needs `tauri-plugin-dialog`, a new dependency. It is shell-side
only; core never sees a file dialog.

## Testing

A `FakeFetcher` in `core::fakes`, configurable to emit a given number of
progress ticks, to succeed, to fail, to deliver bytes that do not match the
hash, or to block until cancelled. It covers the flow end to end with no
network:

- install writes the file and swaps the transcriber
- a hash mismatch leaves no file behind and the previous model still loaded
- cancel restores the previous status *and* reverts `config.model`
- a second install while one is in flight is refused
- `ensure_model` downloads when the desired model is absent
- `ensure_model` falls back to an installed model when the desired one fails
- config migration: legacy `modelPath` becomes `Local`, and an unknown catalog
  id still loads the rest of the config
- progress is throttled to one event per interval (injectable interval — a
  timing-sensitive test that sleeps is a flaky test)

`speakeasy-whisper` gets two tests: `load()` swaps the context, and a failed
`load()` leaves the previous model working. Both need real weights, so they
follow the `#[ignore]` + `SPEAKEASY_MODEL` gating `tests/transcribe.rs` already
uses.

`speakeasy-models` gets one test against a `TcpListener` on localhost serving
fixed bytes — enough to prove the streaming, progress and cancel paths without
depending on Hugging Face being reachable.

No test in the suite hits the network.
