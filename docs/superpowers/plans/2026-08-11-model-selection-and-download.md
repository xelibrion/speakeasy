# Model Selection and Download Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user pick a speech model from a curated catalog or a local file, download it with visible progress, and switch models without restarting the app.

**Architecture:** The catalog and the download orchestration live in `speakeasy-core`; the HTTP transport sits behind a new `ModelFetcher` port implemented by a new `speakeasy-models` crate, exactly as `Transcriber` sits in front of `speakeasy-whisper`. Config stores *desired* state (`ModelChoice`), a new in-memory `ModelStatus` reports *actual* state, and `Engine::ensure_model` reconciles them at startup — which makes first-run auto-download the same code path as a user switching models.

**Tech Stack:** Rust (workspace of 4 crates + Tauri shell), `sha2` for verification, `ureq` for blocking HTTP, TypeScript/Vite for the settings webview, `tauri-plugin-dialog` for the local-file picker.

**Spec:** `docs/superpowers/specs/2026-08-11-model-selection-and-download-design.md`

---

## Orientation for someone new to this codebase

Read these before starting. They are short and they explain the rules this plan obeys.

- `crates/speakeasy-core/src/lib.rs` — core is portable: no tauri, no cpal, no whisper, no OS APIs. The public surface is flat (owned types, no generics in signatures) so it can be bound to Swift later.
- `crates/speakeasy-core/src/ports.rs` — the four traits core needs from the outside world. You are adding a fifth.
- `apps/desktop/src-tauri/src/commands.rs` — its doc comment says a non-trivial command body is a review failure. Every command you add is a one-line delegation.
- `Makefile` — `make check` runs fmt-check, clippy with `-D warnings`, tests, and `make boundary` (which fails if `speakeasy-core` ever gains a tauri dependency). Run `make check` before every commit.

Conventions to match, visible throughout the existing code:

- Doc comments explain *why*, not *what*. Look at `config.rs`'s comment on `WidgetPosition` for the register.
- Test names are sentences: `a_missing_file_loads_the_defaults`, not `test_load`.
- Mutex locks use `.expect("<name> mutex poisoned")`.
- Errors are a flat enum with owned payloads.

---

## File structure

**New files**

| Path | Responsibility |
|---|---|
| `crates/speakeasy-core/src/models/mod.rs` | Module wiring and re-exports |
| `crates/speakeasy-core/src/models/catalog.rs` | `ModelId`, `ModelInfo`, the static `CATALOG`, lookup |
| `crates/speakeasy-core/src/models/manager.rs` | `ModelManager` — file layout, download, verify, swap |
| `crates/speakeasy-core/src/testing.rs` | `TempDir` test helper, shared across modules |
| `crates/speakeasy-models/Cargo.toml` | New crate manifest |
| `crates/speakeasy-models/src/lib.rs` | `HttpFetcher` — the real `ModelFetcher` |
| `crates/speakeasy-models/tests/fetch.rs` | Localhost `TcpListener` test |
| `apps/desktop/src/settings.ts` | Settings window: the tab shell, the permissions banner, and the Models and Dictation tabs |

**Modified files**

| Path | Change |
|---|---|
| `crates/speakeasy-core/src/config.rs` | `ModelChoice` replaces `model_path`; migration in `load` |
| `crates/speakeasy-core/src/ports.rs` | `ModelFetcher` port; `Transcriber::load` |
| `crates/speakeasy-core/src/event.rs` | `ModelStatus`; `Event::ModelStatusChanged` |
| `crates/speakeasy-core/src/error.rs` | `ModelDownload`, `ModelVerification` |
| `crates/speakeasy-core/src/engine.rs` | `Arc<dyn Transcriber>`, model methods, `ensure_model` |
| `crates/speakeasy-core/src/fakes.rs` | `FakeFetcher`; `FakeTranscriber::load` support |
| `crates/speakeasy-core/src/lib.rs` | Module + re-export wiring |
| `crates/speakeasy-whisper/src/lib.rs` | `Transcriber::load` implementation |
| `crates/speakeasy-whisper/tests/transcribe.rs` | Two swap tests |
| `apps/desktop/src-tauri/src/main.rs` | Drop the model-loading branch; call `ensure_model` |
| `apps/desktop/src-tauri/src/commands.rs` | Seven new one-line commands |
| `apps/desktop/src-tauri/src/bridge.rs` | Forward `ModelStatusChanged` to the tray |
| `apps/desktop/src-tauri/src/tray.rs` | Model status line |
| `apps/desktop/src/engine.ts` | Types + invoke wrappers for the new surface |
| `apps/desktop/src/main.ts` | Delegate to `settings.ts` |
| `Cargo.toml` | `sha2`, `ureq`, `speakeasy-models` workspace deps |
| `README.md` | Document the flow; drop the "not built" line |

`models/` is a directory rather than one file because the catalog is data, the manager is behaviour, and they change for different reasons. `settings.ts` is split out of `main.ts` because `main.ts`'s job is choosing which window to mount, and the Models pane alone is bigger than that file is today.

---

## Task 1: `TempDir` test helper

The `TempDir` in `config.rs`'s test module is private to that module, and three more modules are about to need it.

**Files:**
- Create: `crates/speakeasy-core/src/testing.rs`
- Modify: `crates/speakeasy-core/src/lib.rs`, `crates/speakeasy-core/src/config.rs`

- [ ] **Step 1: Create the helper**

```rust
//! Test-only helpers shared across the crate's unit tests.

/// A directory of our own under the system temp dir, removed on drop.
///
/// Named per-test rather than randomly so a leaked directory is traceable to
/// the test that leaked it.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "speakeasy-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir should be creatable");
        Self(path)
    }

    pub fn join(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
```

The thread id is in the name because `cargo test` runs tests in parallel and the process id alone is shared between them.

- [ ] **Step 2: Wire it into `lib.rs`**

Add next to the other module declarations:

```rust
#[cfg(test)]
mod testing;
```

- [ ] **Step 3: Delete the private `TempDir` from `config.rs` tests and use the shared one**

In `config.rs`'s `mod tests`, delete the `struct TempDir` and its two impls, and add `use crate::testing::TempDir;`. Replace `dir.0.join("config.json")` with `dir.join("config.json")` throughout.

- [ ] **Step 4: Verify nothing broke**

Run: `cargo test -p speakeasy-core`
Expected: PASS, same test count as before.

- [ ] **Step 5: Commit**

```bash
git add crates/speakeasy-core/src/testing.rs crates/speakeasy-core/src/lib.rs crates/speakeasy-core/src/config.rs
git commit -m "Share the TempDir test helper across modules"
```

---

## Task 2: The catalog

**Files:**
- Create: `crates/speakeasy-core/src/models/mod.rs`, `crates/speakeasy-core/src/models/catalog.rs`
- Modify: `crates/speakeasy-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Put these at the bottom of `crates/speakeasy-core/src/models/catalog.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_model_is_in_the_catalog() {
        assert!(find(&ModelId::default()).is_some());
    }

    #[test]
    fn an_unknown_id_is_not_an_entry() {
        assert!(find(&ModelId::new("no-such-model")).is_none());
    }

    #[test]
    fn ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for info in CATALOG {
            assert!(seen.insert(&info.id), "duplicate catalog id {:?}", info.id);
        }
    }

    /// The pane lists these top to bottom and the order doubles as a rough
    /// speed-versus-accuracy ordering, so it is worth asserting rather than
    /// leaving to whoever edits the array next.
    #[test]
    fn the_catalog_is_ordered_smallest_first() {
        let sizes: Vec<u64> = CATALOG.iter().map(|info| info.bytes).collect();
        let mut sorted = sizes.clone();
        sorted.sort_unstable();
        assert_eq!(sizes, sorted);
    }

    /// A typo in a digest turns every download into a verification failure, and
    /// the only symptom is a retry loop. Catch the shape here.
    #[test]
    fn every_digest_is_a_sha256() {
        for info in CATALOG {
            assert_eq!(info.sha256.len(), 64, "{:?} digest is not 64 chars", info.id);
            assert!(
                info.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
                "{:?} digest must be lowercase hex",
                info.id
            );
        }
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p speakeasy-core catalog`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write `catalog.rs`**

```rust
//! The curated list of downloadable models.
//!
//! Static data rather than something fetched at runtime: knowing the size and
//! digest up front is what lets the settings pane say "466 MB" before the user
//! commits, and what makes a truncated or tampered download detectable here
//! rather than surfacing later as a confusing whisper.cpp load error.

use serde::{Deserialize, Serialize};

/// A catalog entry's identifier, e.g. `"base.en"`.
///
/// A string newtype rather than an enum on purpose. This value is persisted in
/// the user's config, and `Config::load` treats malformed JSON as an error — so
/// an enum would mean a config written by a newer build with an unknown id
/// fails to parse and takes every other setting down with it. A string degrades
/// to "not in my catalog", which is recoverable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelId(String);

impl ModelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The file this model occupies inside the models directory.
    pub fn file_name(&self) -> String {
        format!("ggml-{}.bin", self.0)
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Small enough to be quick on any machine, accurate enough to dictate with.
impl Default for ModelId {
    fn default() -> Self {
        Self::new("base.en")
    }
}

/// One downloadable model.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: ModelId,
    /// Shown in the settings pane.
    pub label: &'static str,
    /// One line on who this model is for.
    pub note: &'static str,
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the file at `url`.
    pub sha256: &'static str,
    pub url: &'static str,
    pub english_only: bool,
}

/// Ordered smallest to largest, which doubles as fastest to most accurate.
pub static CATALOG: &[ModelInfo] = &[
    // TODO(implementer): every `bytes` and `sha256` below must be replaced with
    // the real values from the published files before this ships. Do NOT guess
    // them — see Step 4.
];

/// The entry for `id`, or `None` if the catalog does not have it.
pub fn find(id: &ModelId) -> Option<&'static ModelInfo> {
    CATALOG.iter().find(|info| &info.id == id)
}
```

- [ ] **Step 4: Fill in the catalog entries with real values**

This is the one step in the plan that needs the network. Do not invent digests.

For each of `tiny.en`, `base.en`, `small.en`, `medium.en`, `small`, `large-v3-turbo`, fetch the file from the whisper.cpp ggml distribution and record its true size and digest:

```bash
mkdir -p /tmp/speakeasy-catalog && cd /tmp/speakeasy-catalog
for m in tiny.en base.en small.en medium.en small large-v3-turbo; do
  curl -fL -o "ggml-$m.bin" \
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$m.bin"
  echo "$m  bytes=$(stat -f%z "ggml-$m.bin")  sha256=$(shasum -a 256 "ggml-$m.bin" | cut -d' ' -f1)"
done
```

If that URL shape has moved, find the current one before proceeding — do not fall back to a mirror without checking what it serves. Then write the entries, e.g.:

```rust
ModelInfo {
    id: ModelId(String::from("base.en")),  // use a const fn or lazy init as needed
    label: "Base (English)",
    note: "The default. Fast, and accurate enough for dictation.",
    bytes: 0,          // <- real value
    sha256: "",        // <- real value
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
    english_only: true,
},
```

`ModelId` wraps a `String`, which cannot be built in a `static`. Either make `CATALOG` a `LazyLock<Vec<ModelInfo>>` (std, no new dependency) or change `ModelInfo::id` to `&'static str` and build a `ModelId` at the lookup boundary. Prefer `LazyLock` — it keeps one type for ids everywhere.

Suggested notes for the other five: *tiny.en* "Fastest, least accurate. For slow machines."; *small.en* "Noticeably better than Base; a few seconds slower."; *medium.en* "High accuracy. Slow on machines without much memory."; *small* "Multilingual. Use with a language set."; *large-v3-turbo* "Most accurate, multilingual. Large download."

- [ ] **Step 5: Write `models/mod.rs` and wire `lib.rs`**

`models/mod.rs`:

```rust
//! Model catalog, storage, and download.

pub mod catalog;

pub use catalog::{find, ModelId, ModelInfo, CATALOG};
```

In `lib.rs`, add `pub mod models;` and extend the re-export line:

```rust
pub use models::{ModelId, ModelInfo};
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p speakeasy-core catalog`
Expected: PASS, five tests.

- [ ] **Step 7: Commit**

```bash
git add crates/speakeasy-core/src/models crates/speakeasy-core/src/lib.rs
git commit -m "Add the model catalog"
```

---

## Task 3: `ModelChoice` in config, with migration

**Files:**
- Modify: `crates/speakeasy-core/src/config.rs`, `crates/speakeasy-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to `config.rs`'s `mod tests`:

```rust
#[test]
fn the_default_config_wants_the_default_catalog_model() {
    assert_eq!(
        Config::default().model,
        ModelChoice::Catalog { id: ModelId::default() }
    );
}

/// Configs written before the catalog existed carry a bare path. Losing the
/// user's model on upgrade would mean a silent re-download of something they
/// already have.
#[test]
fn a_legacy_model_path_migrates_to_a_local_choice() {
    let dir = TempDir::new("legacy-model");
    let path = dir.join("config.json");
    std::fs::write(&path, r#"{"modelPath": "/models/ggml-base.en.bin"}"#)
        .expect("write should succeed");

    let config = Config::load(&path).expect("a legacy config should load");

    assert_eq!(
        config.model,
        ModelChoice::Local { path: "/models/ggml-base.en.bin".to_string() }
    );
    assert_eq!(config.model_path, None, "the legacy field is consumed");
}

/// A newer build may write a model id this build has never heard of. That must
/// cost the user the model, not every other setting in the file.
#[test]
fn an_unknown_model_id_still_loads_the_rest_of_the_config() {
    let dir = TempDir::new("unknown-model");
    let path = dir.join("config.json");
    std::fs::write(
        &path,
        r#"{"model": {"kind": "catalog", "id": "enormous-v9"}, "minRecordingMs": 400}"#,
    )
    .expect("write should succeed");

    let config = Config::load(&path).expect("an unknown id is not a parse error");

    assert_eq!(config.min_recording_ms, 400);
    assert_eq!(
        config.model,
        ModelChoice::Catalog { id: ModelId::new("enormous-v9") }
    );
}

/// `model` wins if both are somehow present — otherwise a stale `modelPath`
/// from a downgrade-then-upgrade would override a deliberate choice.
#[test]
fn an_explicit_model_beats_a_leftover_model_path() {
    let dir = TempDir::new("both-model-fields");
    let path = dir.join("config.json");
    std::fs::write(
        &path,
        r#"{"model": {"kind": "catalog", "id": "small.en"}, "modelPath": "/old.bin"}"#,
    )
    .expect("write should succeed");

    let config = Config::load(&path).expect("load should succeed");
    assert_eq!(
        config.model,
        ModelChoice::Catalog { id: ModelId::new("small.en") }
    );
}

#[test]
fn a_model_choice_survives_a_save_and_load() {
    let dir = TempDir::new("model-roundtrip");
    let path = dir.join("config.json");

    let config = Config {
        model: ModelChoice::Local { path: "/tmp/custom.bin".to_string() },
        ..Config::default()
    };
    config.save(&path).expect("save should succeed");

    assert_eq!(Config::load(&path).expect("load should succeed"), config);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p speakeasy-core config`
Expected: FAIL — `ModelChoice` and `Config::model` do not exist.

- [ ] **Step 3: Implement**

In `config.rs`, add the import `use crate::models::ModelId;` and replace the `model_path` field:

```rust
    /// Which model the user wants.
    ///
    /// *Desired* state, not actual: on a fresh install this names the default
    /// catalog model before anything has been downloaded. `Engine::ensure_model`
    /// reconciles it against what is on disk, which is what makes first run and
    /// a mid-session switch the same code path.
    pub model: ModelChoice,

    /// Pre-catalog setting, read once and migrated to [`ModelChoice::Local`].
    ///
    /// Kept only so an existing config does not lose its model on upgrade.
    /// `Config::load` clears it, so it disappears from the file on the next
    /// save. Delete this field once no user could still be on such a build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_path: Option<String>,
```

And the enum:

```rust
/// Where the weights come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ModelChoice {
    /// An entry in [`crate::models::CATALOG`], downloaded on demand.
    Catalog { id: ModelId },

    /// A `.bin` the user already has. Never downloaded, never deleted by us.
    Local { path: String },
}

impl Default for ModelChoice {
    fn default() -> Self {
        Self::Catalog { id: ModelId::default() }
    }
}
```

In `Config::default()`, replace `model_path: None` with:

```rust
            model: ModelChoice::default(),
            model_path: None,
```

At the end of `Config::load`, after the `serde_json::from_str`, migrate:

```rust
        let mut config: Self = serde_json::from_str(&text)
            .map_err(|err| Error::Other(format!("{} is not valid config: {err}", path.display())))?;
        config.migrate();
        Ok(config)
```

```rust
impl Config {
    /// Fold deprecated fields into their replacements.
    ///
    /// Only applied on load. A `model_path` alongside an explicit `model` loses:
    /// the explicit field is the newer, deliberate one.
    fn migrate(&mut self) {
        if let Some(path) = self.model_path.take() {
            if self.model == ModelChoice::default() {
                self.model = ModelChoice::Local { path };
            }
        }
    }
}
```

Note the subtlety: serde fills `model` with the default when absent, so "absent" and "explicitly the default" are indistinguishable. Migrating only when `model` equals the default is the right trade — a user who explicitly selected `base.en` *and* has a stale `model_path` gets the local file, which is the same file they were using before the upgrade.

Add to `lib.rs`'s config re-export: `ModelChoice`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p speakeasy-core config`
Expected: PASS. `cargo build --workspace` will now fail in `main.rs` — that is expected and fixed in Task 11.

- [ ] **Step 5: Commit**

```bash
git add crates/speakeasy-core/src/config.rs crates/speakeasy-core/src/lib.rs
git commit -m "Replace the raw model path with a model choice"
```

---

## Task 4: `ModelStatus` and the event

**Files:**
- Modify: `crates/speakeasy-core/src/event.rs`, `crates/speakeasy-core/src/error.rs`, `crates/speakeasy-core/src/lib.rs`

- [ ] **Step 1: Add the error variants**

In `error.rs`, after `ModelNotLoaded`:

```rust
    #[error("could not download the model: {0}")]
    ModelDownload(String),

    /// The bytes arrived but the digest did not match. Its own variant because
    /// the remedy is specific — retry — where a transport failure may not be
    /// worth retrying at all.
    #[error("the downloaded model was corrupted")]
    ModelVerification,
```

- [ ] **Step 2: Add `ModelStatus` and the event**

In `event.rs`, below `State`:

```rust
/// What the model subsystem is doing.
///
/// Deliberately separate from [`State`]. `State` is the dictation cycle, and
/// the two run concurrently — the user can dictate on the current model while
/// the next one downloads. One enum would make those mutually exclusive and
/// would corrupt what the widget means.
///
/// In memory only. The persisted counterpart is `Config::model`, which records
/// what the user *wants*; this records what is actually loaded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ModelStatus {
    /// Nothing loaded and nothing in flight.
    Absent,
    Downloading {
        id: ModelId,
        received: u64,
        total: u64,
    },
    /// Weights are loaded and dictation can run.
    Ready { label: String },
    Failed { error: Error },
}
```

with `use crate::models::ModelId;` at the top.

`Ready` carries a display label rather than a `ModelChoice` because both a catalog id and a local path have to render in the same tray line, and the shell should not be reimplementing that formatting.

Add the variant to `Event`:

```rust
    /// Model download or load progress. Throttled — see `ModelManager`.
    ModelStatusChanged {
        status: ModelStatus,
    },
```

Re-export `ModelStatus` from `lib.rs` alongside `State`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p speakeasy-core`
Expected: clean (warnings about unused variants are fine at this point).

- [ ] **Step 4: Commit**

```bash
git add crates/speakeasy-core/src/event.rs crates/speakeasy-core/src/error.rs crates/speakeasy-core/src/lib.rs
git commit -m "Add model status as its own state machine"
```

---

## Task 5: The `ModelFetcher` port and `Transcriber::load`

**Files:**
- Modify: `crates/speakeasy-core/src/ports.rs`, `crates/speakeasy-core/src/fakes.rs`, `crates/speakeasy-core/src/lib.rs`

- [ ] **Step 1: Add the port**

In `ports.rs`:

```rust
/// Fetches a model file over the network.
///
/// A port rather than an HTTP call in the core for the same reason
/// [`Transcriber`] is one: it keeps `speakeasy-core` free of a TLS stack, and
/// it keeps every download branch — progress, a corrupt file, a cancel
/// mid-transfer — testable offline against [`crate::fakes::FakeFetcher`].
pub trait ModelFetcher: Send + Sync {
    /// Stream `url` into `destination`, calling `progress(received, total)` as
    /// bytes arrive. `total` is 0 if the server did not say.
    ///
    /// Blocking; call it off the UI thread. Must return promptly once `cancel`
    /// is set, and must not leave `destination` in place when it does — the
    /// caller treats a surviving file as a completed download.
    fn fetch(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()>;
}
```

with `use std::path::Path;` and `use std::sync::atomic::AtomicBool;` at the top.

- [ ] **Step 2: Add `load` to `Transcriber`**

```rust
    /// Load weights from `path`, replacing whatever is loaded.
    ///
    /// Expensive and blocking. Implementations must leave the previous model
    /// intact when this fails — a bad swap should cost the user nothing.
    fn load(&self, path: &Path) -> Result<()>;
```

- [ ] **Step 3: Write the fakes**

In `fakes.rs`, extend `FakeTranscriber` with a loaded-path record and a configurable load failure:

```rust
pub struct FakeTranscriber {
    result: Result<String>,
    ready: Mutex<bool>,
    loaded: Mutex<Option<std::path::PathBuf>>,
    load_fails_with: Mutex<Option<Error>>,
}
```

Update the three constructors to fill the new fields (`ready` becomes a `Mutex<bool>`, `loaded: Mutex::new(None)`, `load_fails_with: Mutex::new(None)`), and add:

```rust
impl FakeTranscriber {
    /// Make the next `load` fail, so a test can assert the previous model
    /// survives a bad swap.
    pub fn fail_next_load(&self, error: Error) {
        *self.load_fails_with.lock().expect("load failure mutex poisoned") = Some(error);
    }

    /// The path most recently loaded, if any.
    pub fn loaded(&self) -> Option<std::path::PathBuf> {
        self.loaded.lock().expect("loaded mutex poisoned").clone()
    }
}
```

```rust
    fn load(&self, path: &Path) -> Result<()> {
        if let Some(error) = self
            .load_fails_with
            .lock()
            .expect("load failure mutex poisoned")
            .take()
        {
            return Err(error);
        }
        *self.loaded.lock().expect("loaded mutex poisoned") = Some(path.to_path_buf());
        *self.ready.lock().expect("ready mutex poisoned") = true;
        Ok(())
    }
```

Add an `impl Transcriber for Arc<FakeTranscriber>` mirroring the existing `Arc<FakeSink>` impl, so a test can hold a handle while the engine owns the trait object.

Then the fetcher:

```rust
/// Writes canned bytes to the destination, one progress tick per chunk.
pub struct FakeFetcher {
    /// What to write. The manager hashes this, so a test controls verification
    /// success by controlling these bytes.
    body: Vec<u8>,
    chunks: usize,
    fail_with: Option<Error>,
    /// Set once `fetch` has observed the cancel flag, so a test can assert the
    /// cancel actually reached the transport rather than racing past it.
    observed_cancel: Mutex<bool>,
    calls: Mutex<usize>,
}

impl FakeFetcher {
    pub fn serving(body: impl Into<Vec<u8>>) -> Self {
        Self {
            body: body.into(),
            chunks: 4,
            fail_with: None,
            observed_cancel: Mutex::new(false),
            calls: Mutex::new(0),
        }
    }

    pub fn failing(error: Error) -> Self {
        Self { fail_with: Some(error), ..Self::serving(Vec::new()) }
    }

    /// How many progress ticks a fetch reports. Used by the throttle test.
    pub fn in_chunks(mut self, chunks: usize) -> Self {
        self.chunks = chunks.max(1);
        self
    }

    pub fn observed_cancel(&self) -> bool {
        *self.observed_cancel.lock().expect("cancel mutex poisoned")
    }

    pub fn calls(&self) -> usize {
        *self.calls.lock().expect("calls mutex poisoned")
    }
}

impl ModelFetcher for FakeFetcher {
    fn fetch(
        &self,
        _url: &str,
        destination: &Path,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()> {
        *self.calls.lock().expect("calls mutex poisoned") += 1;

        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }

        let total = self.body.len() as u64;
        let chunk = (self.body.len() / self.chunks).max(1);
        let mut written = Vec::new();

        for piece in self.body.chunks(chunk) {
            if cancel.load(Ordering::Relaxed) {
                *self.observed_cancel.lock().expect("cancel mutex poisoned") = true;
                // The contract: no file survives a cancel.
                let _ = std::fs::remove_file(destination);
                return Ok(());
            }
            written.extend_from_slice(piece);
            progress(written.len() as u64, total);
        }

        std::fs::write(destination, &written)
            .map_err(|err| Error::ModelDownload(format!("fake write failed: {err}")))?;
        Ok(())
    }
}

impl ModelFetcher for Arc<FakeFetcher> {
    fn fetch(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()> {
        self.as_ref().fetch(url, destination, progress, cancel)
    }
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p speakeasy-core --features fakes`
Expected: clean. `speakeasy-whisper` will not compile yet — that is Task 6.

- [ ] **Step 5: Commit**

```bash
git add crates/speakeasy-core/src/ports.rs crates/speakeasy-core/src/fakes.rs crates/speakeasy-core/src/lib.rs
git commit -m "Add the model fetcher port and a loadable transcriber"
```

---

## Task 6: `WhisperTranscriber::load`

**Files:**
- Modify: `crates/speakeasy-whisper/src/lib.rs`, `crates/speakeasy-whisper/tests/transcribe.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/transcribe.rs`:

```rust
#[test]
#[ignore = "needs SPEAKEASY_MODEL"]
fn load_swaps_the_model_in_place() {
    let model = std::env::var("SPEAKEASY_MODEL").expect("set SPEAKEASY_MODEL");

    let transcriber = WhisperTranscriber::unloaded();
    assert!(!transcriber.is_ready(), "starts with no weights");

    transcriber
        .load(Path::new(&model))
        .expect("load should succeed");
    assert!(transcriber.is_ready(), "weights are live without a restart");
}

/// The whole point of building the context before taking the lock: a model
/// that will not load must not cost the user the one that already works.
#[test]
#[ignore = "needs SPEAKEASY_MODEL"]
fn a_failed_load_leaves_the_previous_model_working() {
    let model = std::env::var("SPEAKEASY_MODEL").expect("set SPEAKEASY_MODEL");

    let transcriber = WhisperTranscriber::load(Path::new(&model)).expect("model should load");
    let err = transcriber
        .load(Path::new("/nonexistent/ggml-nope.bin"))
        .expect_err("a missing file should fail to load");

    assert!(transcriber.is_ready(), "the working model survived: {err}");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `SPEAKEASY_MODEL=models/ggml-base.en.bin cargo test -p speakeasy-whisper -- --ignored`
Expected: FAIL to compile — no method named `load`.

- [ ] **Step 3: Implement**

Refactor `WhisperTranscriber::load` (the existing associated function) so the context construction is reusable, then add the trait method:

```rust
impl WhisperTranscriber {
    /// Load weights from disk. Expensive — do it once, off the UI thread.
    pub fn load(model_path: &Path) -> Result<Self> {
        let context = Self::open(model_path)?;
        Ok(Self { context: Mutex::new(Some(context)) })
    }

    /// Build a context without touching any existing one.
    fn open(model_path: &Path) -> Result<WhisperContext> {
        static HOOKS: std::sync::Once = std::sync::Once::new();
        HOOKS.call_once(whisper_rs::install_logging_hooks);

        if !model_path.exists() {
            return Err(Error::Transcription(format!("no model at {}", model_path.display())));
        }

        let path = model_path.to_str().ok_or_else(|| {
            Error::Transcription(format!("model path is not UTF-8: {}", model_path.display()))
        })?;

        let context = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|err| Error::Transcription(format!("could not load {path}: {err}")))?;

        tracing::info!(model = path, "whisper model loaded");
        Ok(context)
    }
}
```

and in `impl Transcriber for WhisperTranscriber`:

```rust
    fn load(&self, path: &Path) -> Result<()> {
        // Built *before* the lock is taken, and deliberately so: on failure we
        // return without ever touching `self`, and a swap that goes wrong costs
        // the user nothing. It also means a slow load does not block an
        // in-flight transcription on the same mutex.
        let context = Self::open(path)?;
        *self.context.lock().expect("whisper context mutex poisoned") = Some(context);
        Ok(())
    }
```

Note the name collision: the inherent `WhisperTranscriber::load(&Path) -> Result<Self>` and the trait `Transcriber::load(&self, &Path)` coexist because one takes `self` and one does not, but a caller with both in scope needs `Transcriber::load(&t, p)`. If clippy or readability complains, rename the inherent one to `WhisperTranscriber::open_at`.

- [ ] **Step 4: Run the tests**

Run: `SPEAKEASY_MODEL=models/ggml-base.en.bin cargo test -p speakeasy-whisper -- --ignored --nocapture`
Expected: PASS, three tests (including the pre-existing one, which needs `SPEAKEASY_WAV`; run it too or expect it to panic on the missing var).

- [ ] **Step 5: Commit**

```bash
git add crates/speakeasy-whisper
git commit -m "Let the whisper transcriber swap models in place"
```

---

## Task 7: `ModelManager` — layout and installed models

No network yet. This task is the file bookkeeping only.

**Files:**
- Create: `crates/speakeasy-core/src/models/manager.rs`
- Modify: `crates/speakeasy-core/src/models/mod.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{FakeFetcher, FakeTranscriber};
    use crate::testing::TempDir;

    fn manager(dir: &TempDir) -> ModelManager {
        ModelManager::new(
            Box::new(FakeFetcher::serving(Vec::new())),
            Arc::new(FakeTranscriber::not_ready()),
            dir.0.clone(),
        )
    }

    #[test]
    fn a_catalog_choice_resolves_into_the_models_directory() {
        let dir = TempDir::new("layout");
        let id = ModelId::new("base.en");

        assert_eq!(
            manager(&dir).path_for(&ModelChoice::Catalog { id: id.clone() }),
            Some(dir.join("ggml-base.en.bin"))
        );
    }

    #[test]
    fn a_local_choice_resolves_to_its_own_path() {
        let dir = TempDir::new("layout-local");

        assert_eq!(
            manager(&dir).path_for(&ModelChoice::Local { path: "/elsewhere/m.bin".into() }),
            Some(std::path::PathBuf::from("/elsewhere/m.bin"))
        );
    }

    #[test]
    fn only_files_that_exist_count_as_installed() {
        let dir = TempDir::new("installed");
        let manager = manager(&dir);
        assert!(manager.installed().is_empty());

        std::fs::write(dir.join("ggml-base.en.bin"), b"weights").expect("write should succeed");
        assert_eq!(manager.installed(), vec![ModelId::new("base.en")]);
    }

    /// A `.part` file is an interrupted download, not a model.
    #[test]
    fn a_partial_download_is_not_installed() {
        let dir = TempDir::new("partial");
        std::fs::write(dir.join("ggml-base.en.bin.part"), b"half").expect("write should succeed");

        assert!(manager(&dir).installed().is_empty());
    }

    #[test]
    fn removing_a_model_deletes_its_file() {
        let dir = TempDir::new("remove");
        let manager = manager(&dir);
        std::fs::write(dir.join("ggml-base.en.bin"), b"weights").expect("write should succeed");

        manager.remove(&ModelId::new("base.en")).expect("remove should succeed");
        assert!(manager.installed().is_empty());
    }

    /// Idempotent so the UI never has to disable the button on a stale list.
    #[test]
    fn removing_a_model_that_is_not_there_is_not_an_error() {
        let dir = TempDir::new("remove-missing");
        assert!(manager(&dir).remove(&ModelId::new("base.en")).is_ok());
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p speakeasy-core --features fakes manager`
Expected: FAIL — no `manager` module.

- [ ] **Step 3: Implement the layout half of `manager.rs`**

```rust
//! Owns the models directory: what is installed, what is downloading, and the
//! swap into the live transcriber.
//!
//! Separate from [`crate::engine::Engine`] because `engine.rs` is already large
//! and this is a self-contained unit with its own state machine and its own
//! tests. The engine holds one and delegates.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::ModelChoice;
use crate::error::{Error, Result};
use crate::event::ModelStatus;
use crate::models::catalog::{self, ModelId, CATALOG};
use crate::ports::{ModelFetcher, Transcriber};

/// The smallest gap between two progress events.
///
/// The Tauri bridge hops to the main thread for every event the tray consumes,
/// so an unthrottled 1.6 GB download would schedule thousands of redundant
/// main-thread updates. Injectable so tests do not have to sleep.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub struct ModelManager {
    fetcher: Box<dyn ModelFetcher>,
    transcriber: Arc<dyn Transcriber>,
    directory: PathBuf,
    status: Mutex<ModelStatus>,
    cancel: AtomicBool,
    /// Held for the whole of an install, so a second one can be refused.
    installing: Mutex<bool>,
    progress_interval: Duration,
    /// Where progress and completion go. Set by the engine.
    on_status: Mutex<Option<Box<dyn Fn(ModelStatus) + Send + Sync>>>,
}

impl ModelManager {
    pub fn new(
        fetcher: Box<dyn ModelFetcher>,
        transcriber: Arc<dyn Transcriber>,
        directory: PathBuf,
    ) -> Self {
        Self {
            fetcher,
            transcriber,
            directory,
            status: Mutex::new(ModelStatus::Absent),
            cancel: AtomicBool::new(false),
            installing: Mutex::new(false),
            progress_interval: PROGRESS_INTERVAL,
            on_status: Mutex::new(None),
        }
    }

    /// Test seam for the throttle. See [`PROGRESS_INTERVAL`].
    #[cfg(test)]
    fn with_progress_interval(mut self, interval: Duration) -> Self {
        self.progress_interval = interval;
        self
    }

    pub fn on_status(&self, sink: Box<dyn Fn(ModelStatus) + Send + Sync>) {
        *self.on_status.lock().expect("status sink mutex poisoned") = Some(sink);
    }

    pub fn status(&self) -> ModelStatus {
        self.status.lock().expect("status mutex poisoned").clone()
    }

    /// Where this choice's weights live. Catalog models sit in our directory;
    /// a local choice is wherever the user put it.
    pub fn path_for(&self, choice: &ModelChoice) -> Option<PathBuf> {
        match choice {
            ModelChoice::Catalog { id } => Some(self.directory.join(id.file_name())),
            ModelChoice::Local { path } => Some(PathBuf::from(path)),
        }
    }

    /// Catalog models whose file is on disk. Ordered as the catalog is.
    pub fn installed(&self) -> Vec<ModelId> {
        CATALOG
            .iter()
            .filter(|info| self.directory.join(info.id.file_name()).is_file())
            .map(|info| info.id.clone())
            .collect()
    }

    /// Delete a downloaded model. Missing is success — the caller wants it
    /// gone, and it is.
    pub fn remove(&self, id: &ModelId) -> Result<()> {
        let path = self.directory.join(id.file_name());
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::Other(format!(
                "could not remove {}: {err}",
                path.display()
            ))),
        }
    }

    fn set_status(&self, next: ModelStatus) {
        *self.status.lock().expect("status mutex poisoned") = next.clone();
        if let Some(sink) = self.on_status.lock().expect("status sink mutex poisoned").as_ref() {
            sink(next);
        }
    }
}
```

`path_for` returns `Option` even though both arms are `Some` today, because a future choice (a bundled model, say) may not have one, and the callers already branch on it.

- [ ] **Step 4: Wire it into `models/mod.rs`**

```rust
pub mod catalog;
pub mod manager;

pub use catalog::{find, ModelId, ModelInfo, CATALOG};
pub use manager::ModelManager;
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p speakeasy-core --features fakes manager`
Expected: PASS, six tests.

- [ ] **Step 6: Commit**

```bash
git add crates/speakeasy-core/src/models
git commit -m "Add the model manager's directory bookkeeping"
```

---

## Task 8: `ModelManager::install` — download, verify, swap

**Files:**
- Modify: `crates/speakeasy-core/src/models/manager.rs`, `Cargo.toml`, `crates/speakeasy-core/Cargo.toml`

- [ ] **Step 1: Add the `sha2` dependency**

In the workspace `Cargo.toml` under `[workspace.dependencies]`:

```toml
# Verification only — hashing is not a network stack, so this does not
# compromise core's "portable, no OS APIs" rule.
sha2 = "0.10"
```

and in `crates/speakeasy-core/Cargo.toml` under `[dependencies]`: `sha2 = { workspace = true }`.

- [ ] **Step 2: Write the failing tests**

These need a catalog entry whose digest matches bytes a fake can serve, which the real catalog cannot give you. Add a test-only entry point that takes a `ModelInfo` directly, and test `install_info` rather than `install`:

```rust
    use crate::models::catalog::ModelInfo;

    /// A catalog entry describing bytes the fake will actually serve.
    fn entry(body: &[u8]) -> ModelInfo {
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(body));
        ModelInfo {
            id: ModelId::new("base.en"),
            label: "Base (English)",
            note: "test entry",
            bytes: body.len() as u64,
            // `ModelInfo.sha256` is `&'static str`; leak in tests only.
            sha256: Box::leak(digest.into_boxed_str()),
            url: "https://example.invalid/ggml-base.en.bin",
            english_only: true,
        }
    }

    fn manager_serving(dir: &TempDir, body: &[u8]) -> (ModelManager, Arc<FakeTranscriber>) {
        let transcriber = Arc::new(FakeTranscriber::not_ready());
        let manager = ModelManager::new(
            Box::new(FakeFetcher::serving(body.to_vec())),
            transcriber.clone(),
            dir.0.clone(),
        );
        (manager, transcriber)
    }

    #[test]
    fn a_successful_install_writes_the_file_and_swaps_the_transcriber() {
        let dir = TempDir::new("install-ok");
        let body = b"pretend weights";
        let (manager, transcriber) = manager_serving(&dir, body);

        let path = manager
            .install_info(&entry(body))
            .expect("install should succeed")
            .expect("install should not report a cancel");

        assert_eq!(path, dir.join("ggml-base.en.bin"));
        assert_eq!(std::fs::read(&path).expect("file should exist"), body);
        assert_eq!(transcriber.loaded(), Some(path), "the new model is live");
        assert!(matches!(manager.status(), ModelStatus::Ready { .. }));
    }

    #[test]
    fn a_digest_mismatch_leaves_no_file_and_reports_verification() {
        let dir = TempDir::new("install-corrupt");
        let (manager, transcriber) = manager_serving(&dir, b"corrupted bytes");

        let err = manager
            .install_info(&entry(b"the expected bytes"))
            .expect_err("a mismatch should fail");

        assert_eq!(err, Error::ModelVerification);
        assert!(manager.installed().is_empty(), "no file was renamed into place");
        assert!(!dir.join("ggml-base.en.bin.part").exists(), "the partial is cleaned up");
        assert_eq!(transcriber.loaded(), None, "nothing was swapped in");
    }

    #[test]
    fn a_transport_failure_surfaces_as_a_download_error() {
        let dir = TempDir::new("install-transport");
        let transcriber = Arc::new(FakeTranscriber::not_ready());
        let manager = ModelManager::new(
            Box::new(FakeFetcher::failing(Error::ModelDownload("offline".into()))),
            transcriber,
            dir.0.clone(),
        );

        let err = manager.install_info(&entry(b"x")).expect_err("should fail");

        assert!(matches!(err, Error::ModelDownload(_)));
        assert!(matches!(manager.status(), ModelStatus::Failed { .. }));
    }

    /// The reason the swap is last: a download that arrives fine but will not
    /// load must not cost the user the model they are already dictating with.
    #[test]
    fn a_failed_swap_leaves_the_previous_model_loaded() {
        let dir = TempDir::new("install-badswap");
        let body = b"pretend weights";
        let (manager, transcriber) = manager_serving(&dir, body);
        transcriber
            .load(std::path::Path::new("/previous/ggml-old.bin"))
            .expect("the previous model loads");
        transcriber.fail_next_load(Error::Transcription("unsupported format".into()));

        let err = manager.install_info(&entry(body)).expect_err("the swap should fail");

        assert!(matches!(err, Error::Transcription(_)));
        assert_eq!(
            transcriber.loaded(),
            Some(std::path::PathBuf::from("/previous/ggml-old.bin")),
            "the working model is still the live one"
        );
    }

    #[test]
    fn a_cancelled_install_reports_no_path_and_leaves_nothing_behind() {
        let dir = TempDir::new("install-cancel");
        let body = b"pretend weights";
        let fetcher = Arc::new(FakeFetcher::serving(body.to_vec()));
        let transcriber = Arc::new(FakeTranscriber::not_ready());
        let manager =
            ModelManager::new(Box::new(fetcher.clone()), transcriber.clone(), dir.0.clone());

        manager.cancel();
        let outcome = manager.install_info(&entry(body)).expect("a cancel is not an error");

        assert_eq!(outcome, None);
        assert!(fetcher.observed_cancel(), "the cancel reached the transport");
        assert!(manager.installed().is_empty());
        assert_eq!(transcriber.loaded(), None);
    }

    /// Abandoning a 1.5 GB transfer because of a stray second click would be
    /// worse than saying no.
    #[test]
    fn a_second_install_is_refused_while_one_is_in_flight() {
        let dir = TempDir::new("install-busy");
        let (manager, _transcriber) = manager_serving(&dir, b"weights");
        *manager.installing.lock().expect("installing mutex poisoned") = true;

        let err = manager.install_info(&entry(b"weights")).expect_err("should refuse");
        assert!(matches!(err, Error::ModelDownload(_)));
    }

    #[test]
    fn progress_events_are_throttled() {
        let dir = TempDir::new("install-throttle");
        let body = vec![7u8; 4096];
        let transcriber = Arc::new(FakeTranscriber::not_ready());
        let seen = Arc::new(Mutex::new(Vec::new()));

        let manager = ModelManager::new(
            Box::new(FakeFetcher::serving(body.clone()).in_chunks(500)),
            transcriber,
            dir.0.clone(),
        )
        .with_progress_interval(Duration::from_secs(3600));

        let recorder = seen.clone();
        manager.on_status(Box::new(move |status| {
            recorder.lock().expect("seen mutex poisoned").push(status);
        }));

        manager.install_info(&entry(&body)).expect("install should succeed");

        let downloads = seen
            .lock()
            .expect("seen mutex poisoned")
            .iter()
            .filter(|status| matches!(status, ModelStatus::Downloading { .. }))
            .count();
        assert_eq!(
            downloads, 1,
            "500 chunks inside one interval must collapse to the opening tick"
        );
    }
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p speakeasy-core --features fakes manager`
Expected: FAIL — no `install_info`.

- [ ] **Step 4: Implement**

```rust
impl ModelManager {
    /// Download, verify and activate a catalog model.
    ///
    /// Blocking. `Ok(None)` means the user cancelled, which is not a failure.
    ///
    /// The order — fetch, verify, rename, swap — is the whole design. The live
    /// model is only replaced once the new one is known-good and loaded, so a
    /// download can run for ten minutes while the user keeps dictating, and
    /// every failure short of the last step is a no-op.
    pub fn install(&self, id: &ModelId) -> Result<Option<PathBuf>> {
        let info = catalog::find(id)
            .ok_or_else(|| Error::ModelDownload(format!("{id} is not a model I know about")))?;
        self.install_info(info)
    }

    fn install_info(&self, info: &ModelInfo) -> Result<Option<PathBuf>> {
        {
            let mut installing = self.installing.lock().expect("installing mutex poisoned");
            if *installing {
                return Err(Error::ModelDownload(
                    "another model is already downloading".to_string(),
                ));
            }
            *installing = true;
        }
        let outcome = self.run_install(info);
        *self.installing.lock().expect("installing mutex poisoned") = false;
        self.cancel.store(false, Ordering::Relaxed);
        outcome
    }

    fn run_install(&self, info: &ModelInfo) -> Result<Option<PathBuf>> {
        std::fs::create_dir_all(&self.directory).map_err(|err| {
            Error::ModelDownload(format!("could not create {}: {err}", self.directory.display()))
        })?;

        let final_path = self.directory.join(info.id.file_name());
        let partial = final_path.with_extension("bin.part");

        self.set_status(ModelStatus::Downloading {
            id: info.id.clone(),
            received: 0,
            total: info.bytes,
        });

        let last = Mutex::new(Instant::now());
        let interval = self.progress_interval;
        let id = info.id.clone();
        let total = info.bytes;
        let progress = |received: u64, reported_total: u64| {
            let mut last = last.lock().expect("progress clock mutex poisoned");
            if last.elapsed() < interval {
                return;
            }
            *last = Instant::now();
            self.set_status(ModelStatus::Downloading {
                id: id.clone(),
                received,
                // Trust the catalog over the server: it is what the size in the
                // UI was drawn from, and a header can be absent or wrong.
                total: if total > 0 { total } else { reported_total },
            });
        };

        if let Err(err) = self.fetcher.fetch(info.url, &partial, &progress, &self.cancel) {
            let _ = std::fs::remove_file(&partial);
            self.set_status(ModelStatus::Failed { error: err.clone() });
            return Err(err);
        }

        if self.cancel.load(Ordering::Relaxed) || !partial.exists() {
            let _ = std::fs::remove_file(&partial);
            self.set_status(self.status_for_loaded());
            return Ok(None);
        }

        if !self.digest_matches(&partial, info.sha256)? {
            let _ = std::fs::remove_file(&partial);
            self.set_status(ModelStatus::Failed { error: Error::ModelVerification });
            return Err(Error::ModelVerification);
        }

        std::fs::rename(&partial, &final_path).map_err(|err| {
            let error =
                Error::ModelDownload(format!("could not install {}: {err}", final_path.display()));
            self.set_status(ModelStatus::Failed { error: error.clone() });
            error
        })?;

        self.activate(&final_path, info.label)?;
        Ok(Some(final_path))
    }

    /// Load `path` and report it as live. Failure leaves the previous model in
    /// place, because that is [`Transcriber::load`]'s contract.
    pub fn activate(&self, path: &Path, label: &str) -> Result<()> {
        match self.transcriber.load(path) {
            Ok(()) => {
                self.set_status(ModelStatus::Ready { label: label.to_string() });
                Ok(())
            }
            Err(err) => {
                self.set_status(ModelStatus::Failed { error: err.clone() });
                Err(err)
            }
        }
    }

    /// Streamed rather than read whole: the largest catalog entry is over a
    /// gigabyte and this runs on every install.
    fn digest_matches(&self, path: &Path, expected: &str) -> Result<bool> {
        use sha2::{Digest, Sha256};

        let mut file = std::fs::File::open(path).map_err(|err| {
            Error::ModelDownload(format!("could not read {}: {err}", path.display()))
        })?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(|err| {
            Error::ModelDownload(format!("could not hash {}: {err}", path.display()))
        })?;

        Ok(format!("{:x}", hasher.finalize()) == expected)
    }

    /// What to report once an install stops without replacing anything.
    fn status_for_loaded(&self) -> ModelStatus {
        match self.status() {
            // Keep whatever was already live; a cancel changes nothing.
            ready @ ModelStatus::Ready { .. } => ready,
            _ if self.transcriber.is_ready() => ModelStatus::Ready { label: "Model".to_string() },
            _ => ModelStatus::Absent,
        }
    }

    /// Ask an in-flight install to stop. Returns immediately; the install
    /// thread notices on its next chunk.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
```

Add `use crate::models::catalog::ModelInfo;` to the imports.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p speakeasy-core --features fakes manager`
Expected: PASS, thirteen tests.

- [ ] **Step 6: Run the full check**

Run: `make lint && cargo test -p speakeasy-core --features fakes`
Expected: clean. Fix any clippy complaint about the closure capturing `self` by restructuring rather than by `#[allow]`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/speakeasy-core
git commit -m "Download, verify and activate models"
```

---

## Task 9: The engine surface

**Files:**
- Modify: `crates/speakeasy-core/src/engine.rs`

- [ ] **Step 1: Write the failing tests**

Add to `engine.rs`'s `mod tests`:

```rust
    /// Desired state is written when the user chooses, not when the download
    /// finishes — that is what makes an interrupted download resume on the
    /// next launch.
    #[test]
    fn selecting_a_local_model_records_it_and_loads_it() {
        let dir = crate::testing::TempDir::new("engine-select-local");
        let weights = dir.join("mine.bin");
        std::fs::write(&weights, b"weights").expect("write should succeed");
        let (engine, _sink) = engine("hello");

        engine.select_model(ModelChoice::Local {
            path: weights.display().to_string(),
        });

        assert_eq!(
            engine.config().model,
            ModelChoice::Local { path: weights.display().to_string() }
        );
        assert!(matches!(engine.model_status(), ModelStatus::Ready { .. }));
    }

    #[test]
    fn ensure_model_loads_a_model_that_is_already_on_disk() {
        let dir = crate::testing::TempDir::new("engine-ensure-present");
        let weights = dir.join("mine.bin");
        std::fs::write(&weights, b"weights").expect("write should succeed");
        let (engine, _sink) = engine("hello");
        engine.set_config(Config {
            model: ModelChoice::Local { path: weights.display().to_string() },
            ..Config::default()
        });

        engine.ensure_model();

        assert!(matches!(engine.model_status(), ModelStatus::Ready { .. }));
    }

    /// Dictation must not be left dead because one model could not be had.
    #[test]
    fn ensure_model_falls_back_to_an_installed_model_when_the_wanted_one_fails() {
        // Build an engine whose models directory has `base.en` installed but
        // whose config wants a model the fetcher will fail to deliver.
        // Assert: status is Failed, but the transcriber is ready.
    }

    #[test]
    fn cancelling_a_download_restores_the_previously_selected_model() {
        // Select a local model, start an install, cancel it, and assert
        // config.model is back to the local choice.
    }

    #[test]
    fn a_status_change_reaches_subscribers() {
        let dir = crate::testing::TempDir::new("engine-status-event");
        let weights = dir.join("mine.bin");
        std::fs::write(&weights, b"weights").expect("write should succeed");
        let (engine, _sink) = engine("hello");
        let events = engine.subscribe();

        engine.select_model(ModelChoice::Local {
            path: weights.display().to_string(),
        });

        assert!(
            events.try_iter().any(|event| matches!(event, Event::ModelStatusChanged { .. })),
            "the shell learns about model changes the same way it learns about state"
        );
    }
```

The two sketched tests must be written out fully during implementation — they are the two behaviours from the spec most likely to be got wrong, so do not skip them.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p speakeasy-core --features fakes engine`
Expected: FAIL — the methods do not exist.

- [ ] **Step 3: Implement**

Change `Inner.transcriber` from `Box<dyn Transcriber>` to `Arc<dyn Transcriber>` so the manager can hold the same instance, and add `models: ModelManager` to `Inner`. `Engine::new` gains a `fetcher: Box<dyn ModelFetcher>` parameter and a models directory; keep the existing four-argument shape working by adding a chained builder instead if that churns too many call sites — but prefer widening `new`, since every caller is being touched anyway and an engine without a fetcher cannot honour `ensure_model`.

Wire the manager's status sink to the engine's `emit` in `new`, so `set_status` becomes `Event::ModelStatusChanged` for free.

Then:

```rust
    pub fn catalog(&self) -> &'static [ModelInfo] { CATALOG }

    pub fn installed_models(&self) -> Vec<ModelId> { self.inner.models.installed() }

    pub fn model_status(&self) -> ModelStatus { self.inner.models.status() }

    /// Start downloading `id` and return immediately.
    ///
    /// Fire-and-forget, unlike [`Engine::finish_recording`]: there is no return
    /// value worth waiting for, since every outcome arrives as
    /// [`Event::ModelStatusChanged`]. That is also what keeps the Tauri
    /// commands to one-line delegations.
    ///
    /// Config is written *before* the download starts, because it records what
    /// the user wants rather than what succeeded — so quitting mid-download
    /// resumes on the next launch.
    pub fn install_model(&self, id: ModelId) {
        let previous = self.config().model;
        self.set_model_choice(ModelChoice::Catalog { id: id.clone() });

        let engine = self.clone();
        std::thread::spawn(move || match engine.inner.models.install(&id) {
            // Cancelled: the user said never mind, so the desired model goes
            // back to what it was. Without this the next launch would resume
            // the transfer they just abandoned.
            Ok(None) => engine.set_model_choice(previous),
            // Failed: config deliberately keeps naming the wanted model, so a
            // retry is one click and the next launch tries again.
            Ok(Some(_)) | Err(_) => {}
        });
    }

    /// Switch to a model that needs no download: a local file, or a catalog
    /// entry already on disk.
    pub fn select_model(&self, choice: ModelChoice) {
        self.set_model_choice(choice.clone());
        let Some(path) = self.inner.models.path_for(&choice) else { return };
        let label = label_for(&choice);
        if let Err(err) = self.inner.models.activate(&path, &label) {
            tracing::warn!(%err, "could not switch models");
        }
    }

    pub fn cancel_model_download(&self) { self.inner.models.cancel(); }

    pub fn remove_model(&self, id: &ModelId) -> Result<()> { self.inner.models.remove(id) }

    /// Reconcile desired state (`config.model`) against what is on disk.
    ///
    /// Called once at startup. First run is not a special case: a fresh config
    /// wants the default catalog model, nothing is installed, so this downloads
    /// it — the same path a mid-session switch takes.
    pub fn ensure_model(&self) {
        let choice = self.config().model;
        match self.inner.models.path_for(&choice) {
            Some(path) if path.is_file() => {
                if self.inner.models.activate(&path, &label_for(&choice)).is_ok() {
                    return;
                }
            }
            _ => {}
        }

        match choice {
            ModelChoice::Catalog { id } => self.install_model(id),
            // A local file we cannot read is nothing we can fix by downloading.
            ModelChoice::Local { .. } => self.fall_back_to_any_installed(),
        }
    }

    /// Load whatever is installed so the hotkey still works.
    ///
    /// Transcribing on a model the user did not pick, while `ModelStatus`
    /// says so, beats not transcribing at all.
    fn fall_back_to_any_installed(&self) { /* first entry of `installed()`, if any */ }

    fn set_model_choice(&self, choice: ModelChoice) {
        self.inner.config.lock().expect("config mutex poisoned").model = choice;
        self.persist();
    }
```

`fall_back_to_any_installed` must also run when a catalog download fails, which means `install_model`'s spawned thread calls it on `Err`. Write it so the fallback never clobbers `ModelStatus::Failed` — the user needs to see why they are not on the model they picked.

`label_for(&ModelChoice)` returns the catalog label, or the file name for a local path.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p speakeasy-core --features fakes`
Expected: PASS, all tests including the pre-existing ones.

- [ ] **Step 5: Commit**

```bash
git add crates/speakeasy-core/src/engine.rs
git commit -m "Give the engine a model surface"
```

---

## Task 10: The `speakeasy-models` crate

**Files:**
- Create: `crates/speakeasy-models/Cargo.toml`, `crates/speakeasy-models/src/lib.rs`, `crates/speakeasy-models/tests/fetch.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Create the crate and register it**

`crates/speakeasy-models/Cargo.toml`:

```toml
[package]
name = "speakeasy-models"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
speakeasy-core = { workspace = true }
ureq = { workspace = true }
tracing = { workspace = true }
```

In the workspace `Cargo.toml`: add `"crates/speakeasy-models"` to `members`, `speakeasy-models = { path = "crates/speakeasy-models" }` and `ureq = "2"` to `[workspace.dependencies]`.

`ureq` rather than `reqwest`: the port is blocking by contract, so an async runtime buys nothing, and `ureq` is a much smaller tree.

- [ ] **Step 2: Write the failing test**

`crates/speakeasy-models/tests/fetch.rs`:

```rust
//! Exercises the real fetcher against a socket on localhost.
//!
//! No network and no dependency on Hugging Face being reachable, which matters
//! because this runs in a plain `cargo test`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use speakeasy_core::ports::ModelFetcher;
use speakeasy_models::HttpFetcher;

/// Serves `body` once and returns the URL to reach it at.
fn serve(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("should bind");
    let port = listener.local_addr().expect("should have an address").port();

    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("should accept");
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&body);
        let _ = stream.flush();
    });

    (format!("http://127.0.0.1:{port}/model.bin"), handle)
}

#[test]
fn fetch_writes_the_body_and_reports_progress() {
    let body = vec![9u8; 64 * 1024];
    let (url, server) = serve(body.clone());
    let destination = std::env::temp_dir().join(format!("speakeasy-fetch-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&destination);

    let ticks = Arc::new(Mutex::new(Vec::new()));
    let recorder = ticks.clone();

    HttpFetcher::new()
        .fetch(
            &url,
            &destination,
            &move |received, total| recorder.lock().expect("ticks mutex poisoned").push((received, total)),
            &AtomicBool::new(false),
        )
        .expect("fetch should succeed");
    server.join().expect("server thread should finish");

    assert_eq!(std::fs::read(&destination).expect("file should exist"), body);

    let ticks = ticks.lock().expect("ticks mutex poisoned");
    assert!(!ticks.is_empty(), "progress was reported");
    assert_eq!(ticks.last().expect("at least one tick").0, body.len() as u64);
    assert_eq!(ticks.last().expect("at least one tick").1, body.len() as u64,
        "Content-Length becomes the total");

    let _ = std::fs::remove_file(&destination);
}

#[test]
fn a_cancelled_fetch_stops_early_and_leaves_no_file() {
    let (url, server) = serve(vec![9u8; 8 * 1024 * 1024]);
    let destination =
        std::env::temp_dir().join(format!("speakeasy-cancel-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&destination);

    let cancel = AtomicBool::new(true);
    HttpFetcher::new()
        .fetch(&url, &destination, &|_, _| {}, &cancel)
        .expect("a cancel is not an error");
    let _ = server.join();

    assert!(!destination.exists(), "no partial file survives a cancel");
}

#[test]
fn a_non_200_response_is_a_download_error() {
    // Serve "HTTP/1.1 404 Not Found" and assert Error::ModelDownload.
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p speakeasy-models`
Expected: FAIL — no `HttpFetcher`.

- [ ] **Step 4: Implement `HttpFetcher`**

```rust
//! [`ModelFetcher`] over HTTP.
//!
//! Blocking on purpose: the port's contract is blocking, so an async runtime
//! would be a dependency with nothing to do. This crate exists so that
//! `speakeasy-core` does not have to carry a TLS stack — see the design doc.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use speakeasy_core::ports::ModelFetcher;
use speakeasy_core::{Error, Result};

/// How much to read before checking the cancel flag. Small enough that a
/// cancel feels immediate, large enough not to syscall per kilobyte.
const CHUNK: usize = 64 * 1024;

#[derive(Default)]
pub struct HttpFetcher;

impl HttpFetcher {
    pub fn new() -> Self {
        Self
    }
}

impl ModelFetcher for HttpFetcher {
    fn fetch(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()> {
        let response = ureq::get(url)
            .call()
            .map_err(|err| Error::ModelDownload(format!("{url}: {err}")))?;

        let total = response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);

        let mut reader = response.into_reader();
        let mut file = std::fs::File::create(destination).map_err(|err| {
            Error::ModelDownload(format!("could not create {}: {err}", destination.display()))
        })?;

        let mut buffer = vec![0u8; CHUNK];
        let mut received = 0u64;

        loop {
            if cancel.load(Ordering::Relaxed) {
                // The port's contract: nothing survives a cancel, because the
                // caller reads a surviving file as a finished download.
                drop(file);
                let _ = std::fs::remove_file(destination);
                return Ok(());
            }

            let read = reader
                .read(&mut buffer)
                .map_err(|err| Error::ModelDownload(format!("{url}: {err}")))?;
            if read == 0 {
                break;
            }

            file.write_all(&buffer[..read]).map_err(|err| {
                Error::ModelDownload(format!("could not write {}: {err}", destination.display()))
            })?;
            received += read as u64;
            progress(received, total);
        }

        file.flush().map_err(|err| {
            Error::ModelDownload(format!("could not flush {}: {err}", destination.display()))
        })?;
        tracing::info!(url, received, "model downloaded");
        Ok(())
    }
}
```

Check `ureq`'s current API before writing this — the 2.x `response.header(..)` / `into_reader()` shape above may have moved in a newer major. Adjust rather than pinning to an old version.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p speakeasy-models`
Expected: PASS, three tests.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/speakeasy-models
git commit -m "Add the HTTP model fetcher"
```

---

## Task 11: Wire the shell

**Files:**
- Modify: `apps/desktop/src-tauri/src/main.rs`, `commands.rs`, `bridge.rs`, `tray.rs`, `apps/desktop/src-tauri/Cargo.toml`

- [ ] **Step 1: Update `main.rs`**

Add `speakeasy-models` to `apps/desktop/src-tauri/Cargo.toml`. Then delete the whole `let transcriber = match &config.model_path { … }` block and replace with:

```rust
    // Always unloaded: which weights to use, whether they are on disk, and
    // whether to fetch them are all `Engine::ensure_model`'s decision. Every
    // shell then gets identical first-run behaviour for free.
    let transcriber = WhisperTranscriber::unloaded();
```

Change `SPEAKEASY_MODEL` handling in `load_config` to set the new field:

```rust
    if let Ok(model) = std::env::var("SPEAKEASY_MODEL") {
        config.model = speakeasy_core::ModelChoice::Local { path: model };
    }
```

Add a models directory beside the config:

```rust
/// Downloaded weights, a sibling of `config.json` so they survive reinstalling.
fn models_dir(config: &std::path::Path) -> std::path::PathBuf {
    config
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("models")
}
```

Build the engine with the fetcher and the directory, and call `ensure_model` in `setup` — after `bridge::forward_events` and `tray::attach`, so the first status event has somewhere to land:

```rust
            // After the bridge and the tray are up: this can start a download
            // immediately on first run, and the progress has to be visible.
            engine.ensure_model();
```

- [ ] **Step 2: Add the commands**

In `commands.rs`, seven one-liners:

```rust
#[tauri::command]
pub fn model_catalog(engine: TauriState<'_, Engine>) -> Vec<ModelInfo> {
    engine.catalog().to_vec()
}

#[tauri::command]
pub fn model_status(engine: TauriState<'_, Engine>) -> ModelStatus {
    engine.model_status()
}

#[tauri::command]
pub fn installed_models(engine: TauriState<'_, Engine>) -> Vec<ModelId> {
    engine.installed_models()
}

#[tauri::command]
pub fn install_model(engine: TauriState<'_, Engine>, id: ModelId) {
    engine.install_model(id);
}

#[tauri::command]
pub fn select_model(engine: TauriState<'_, Engine>, choice: ModelChoice) {
    engine.select_model(choice);
}

#[tauri::command]
pub fn cancel_model_download(engine: TauriState<'_, Engine>) {
    engine.cancel_model_download();
}

#[tauri::command]
pub fn remove_model(engine: TauriState<'_, Engine>, id: ModelId) -> std::result::Result<(), String> {
    engine.remove_model(&id).map_err(|err| err.to_string())
}
```

Register all seven in `generate_handler!`.

- [ ] **Step 3: Forward status to the tray**

In `bridge.rs`, alongside the existing `StateChanged` arm:

```rust
            if let Event::ModelStatusChanged { status } = &event {
                let status = status.clone();
                if let Err(err) = app.run_on_main_thread(move || tray::refresh_model(&status)) {
                    tracing::warn!(%err, "could not schedule the model status update");
                }
            }
```

- [ ] **Step 4: Show it in the tray**

In `tray.rs`, keep the last model status in a `static MODEL_STATUS: Mutex<ModelStatus>` and have `status_text` consult it:

```rust
/// Downloading wins over the dictation state: it is the more urgent and more
/// transient thing to say, and while it runs the dictation state is almost
/// always `Idle` anyway.
fn status_text(state: &State) -> String {
    match &*MODEL_STATUS.lock().expect("model status mutex poisoned") {
        ModelStatus::Downloading { id, received, total } if *total > 0 => {
            return format!("Downloading {id}… {}%", received * 100 / total);
        }
        ModelStatus::Downloading { id, .. } => return format!("Downloading {id}…"),
        ModelStatus::Failed { error } => return format!("Model error: {error}"),
        ModelStatus::Absent => return "No model loaded".to_string(),
        ModelStatus::Ready { .. } => {}
    }
    match state { /* unchanged */ }
}
```

`refresh_model` stores the new status and re-renders using the engine's last known `State`, which means `tray.rs` needs to keep that too — store it in a `static LAST_STATE` set by `refresh`.

- [ ] **Step 5: Build and run**

Run: `make check`
Expected: clean.

Run: `make dev` — with no config, the app should start downloading the default model and the tray should show a rising percentage.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri
git commit -m "Wire model selection through the Tauri shell"
```

---

## Task 12: The settings window

**Files:**
- Create: `apps/desktop/src/settings.ts`
- Modify: `apps/desktop/src/engine.ts`, `apps/desktop/src/main.ts`, `apps/desktop/src/styles.css`, `apps/desktop/package.json`, `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/capabilities/*.json`

- [ ] **Step 1: Extend `engine.ts`**

Replace `modelPath` in `Config` with `model: ModelChoice`, and add:

```ts
export type ModelChoice =
  | { kind: "catalog"; id: string }
  | { kind: "local"; path: string };

export type ModelInfo = {
  id: string;
  label: string;
  note: string;
  bytes: number;
  sha256: string;
  url: string;
  englishOnly: boolean;
};

export type ModelStatus =
  | { status: "absent" }
  | { status: "downloading"; id: string; received: number; total: number }
  | { status: "ready"; label: string }
  | { status: "failed"; error: EngineError };

export const getModelCatalog = () => invoke<ModelInfo[]>("model_catalog");
export const getModelStatus = () => invoke<ModelStatus>("model_status");
export const getInstalledModels = () => invoke<string[]>("installed_models");
export const installModel = (id: string) => invoke<void>("install_model", { id });
export const selectModel = (choice: ModelChoice) => invoke<void>("select_model", { choice });
export const cancelModelDownload = () => invoke<void>("cancel_model_download");
export const removeModel = (id: string) => invoke<void>("remove_model", { id });
```

Add `modelStatusChanged` to `EngineEvent`, and the two new kinds to `ErrorKind` and `ERROR_LABELS`:

```ts
  ModelDownload: "Could not download the model",
  ModelVerification: "The download was corrupted — try again",
```

Add a size formatter, since four call sites want it:

```ts
/** "466 MB" — decimal MB, matching how the models are advertised. */
export function formatBytes(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}
```

- [ ] **Step 2: Add the dialog plugin**

```bash
cd apps/desktop && npm install @tauri-apps/plugin-dialog
```

Add `tauri-plugin-dialog = "2"` to `apps/desktop/src-tauri/Cargo.toml`, register it with `.plugin(tauri_plugin_dialog::init())` in `main.rs`, and add `dialog:allow-open` to the settings window's capability file. Without the capability entry the picker silently does nothing.

- [ ] **Step 3: Write `settings.ts`**

Structure, not full code — follow the existing style in `widget.ts`:

`mountSettings(app)` builds a **tabbed window**, because more tabs are coming — a History tab has its own spec, and the currently read-only settings will grow editors. Getting the shell right once is cheaper than retrofitting navigation around a pane that assumed it was alone.

```ts
type Tab = {
  id: string;
  label: string;
  mount: (panel: HTMLElement) => void | Promise<void>;
};

const TABS: Tab[] = [
  { id: "models", label: "Models", mount: mountModels },
  { id: "dictation", label: "Dictation", mount: mountDictation },
];
```

Data-driven on purpose: adding the History tab later is one entry in this array plus one `mount` function, with no change to the shell.

Shell behaviour:

- A `<nav role="tablist">` of buttons, one per entry, and a single `<section role="tabpanel">` below it. Use the ARIA roles and `aria-selected` — it costs three attributes and it is what makes the tabs keyboard-navigable.
- The first tab is selected on open. Do **not** persist the selected tab to config: `Config` is behavioural state that syncs to disk on every change, and which tab someone last clicked is neither.
- Switching tabs calls the new tab's `mount` on a cleared panel. Each `mount` owns its own event subscription and returns nothing; the shell tears down the previous panel's listeners before mounting the next, so a background tab is not re-rendering on every progress tick.

**Permissions is a banner, not a tab.** `permissionsGranted()` plus the "Open permission settings" button render *above* the tab bar, and only when the grant is missing. **Carried over from the current `main.ts`, not rewritten.** It is the only in-app route to fixing a missing Accessibility grant — the likeliest reason a fresh install does nothing — so it must survive the deletion of the debug log, and burying it in a tab nobody opens would waste it. A banner is right for something that is usually absent and urgent when present.

**The Dictation tab** is read-only rows: `activationLabel(config)`, injection mode, and language (or "Auto-detect" when null). No editors — those settings are not editable yet and the window should be honest about what it does. The tab exists as the slot they will fill.

**The Models tab:**
- The Models section renders one row per `getModelCatalog()` entry: label, `note`, `formatBytes(info.bytes)`, and one control chosen from — **In use** (disabled) when `config.model` is `{kind:"catalog", id}` matching; **Use** when installed but not selected; **Remove** beside it; **Download** when not installed; a `<progress>` plus **Cancel** when `modelStatus` is `downloading` with this id.
- Below the rows, a **Use a local file…** button calling `open({ filters: [{ name: "Model", extensions: ["bin"] }] })` from `@tauri-apps/plugin-dialog`, then `selectModel({ kind: "local", path })`.
- Subscribe with `onEngineEvent`; on `modelStatusChanged`, update only the affected row's control rather than re-rendering the list — a re-render on every progress tick would fight the user's scroll position.
- Show `errorLabel(error)` inline in the row when status is `failed`, with the Download button becoming **Retry**.

- [ ] **Step 4: Point `main.ts` at it**

`main.ts` shrinks back to its stated job: pick a window and mount it.

```ts
import { mountSettings } from "./settings";
import { mountWidget } from "./widget";

if (getCurrentWindow().label === "widget") {
  mountWidget(root());
} else {
  void mountSettings(root());
}
```

Delete the debug-log `mountSettings` and its `log` helper — but move the permissions check and button into `settings.ts` first, rather than losing them with the log. The event stream itself is still visible in the devtools console and in the tracing output, which is where anyone debugging looks.

- [ ] **Step 5: Style it**

Add `.settings` section/row rules to `styles.css` following the existing conventions there. Keep it plain — this is a utility window, not a showcase.

- [ ] **Step 6: Verify by hand**

Run: `make dev`

Check each of these:
- With no models, the app auto-downloads the default and the pane shows a progress bar.
- Cancel mid-download: the row returns to **Download**, no `.part` file remains in the models directory, and `config.json`'s `model` is back to what it was.
- Download a second model, then **Use** it: dictation works immediately, with no restart.
- **Remove** a model that is not in use: the row returns to **Download**.
- Pick a local `.bin`: the row set stays as-is and the read-only summary shows the local path.
- Quit mid-download and relaunch: the download restarts on its own.
- Revoke Accessibility in System Settings and reopen the window: the permissions banner appears above the tabs with a working button.
- Switch to the Dictation tab while a download runs: the tray keeps updating, and switching back to Models shows current progress rather than a stale bar.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop
git commit -m "Build the settings Models pane"
```

---

## Task 13: Documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the README**

- Remove "model download and first-run flow" from the not-built list.
- Document the new config shape, including that `model` is desired state and `ModelStatus` is actual state, and that `SPEAKEASY_MODEL` now maps to `Local`.
- Note the models directory location.
- Add `speakeasy-models` to the crate list with its one-line rationale (keeps the TLS stack out of core, mirrors `speakeasy-whisper`).

- [ ] **Step 2: Full check**

Run: `make check`
Expected: clean, including `make boundary`.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "Document model selection and download"
```

---

## Verification checklist

Before calling this done, confirm each with output in hand — not from memory:

- [ ] `make check` passes (fmt, clippy `-D warnings`, tests, boundary)
- [ ] `cargo test -p speakeasy-core --features fakes` — every model test passes offline
- [ ] `cargo test -p speakeasy-models` passes with no network
- [ ] `SPEAKEASY_MODEL=models/ggml-base.en.bin cargo test -p speakeasy-whisper -- --ignored` passes
- [ ] Every `bytes` and `sha256` in `CATALOG` came from a real file, not a guess
- [ ] A real download of at least two catalog models completes and verifies
- [ ] An existing `config.json` with `modelPath` still works after upgrading
