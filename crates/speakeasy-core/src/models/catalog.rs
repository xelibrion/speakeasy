//! The curated list of downloadable models.
//!
//! Static data rather than something fetched at runtime: knowing the size and
//! digest up front is what lets the settings pane say "488 MB" before the user
//! commits, and what makes a truncated or tampered download detectable here
//! rather than surfacing later as a confusing whisper.cpp load error.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// A catalog entry's identifier, e.g. `"base.en"`.
///
/// A string newtype rather than an enum on purpose. This value is persisted in
/// the user's config, and [`crate::Config::load`] treats malformed JSON as an
/// error — so an enum would mean a config written by a newer build with an
/// unknown id fails to parse and takes every other setting down with it. A
/// string degrades to "not in my catalog", which is recoverable.
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
    ///
    /// Matches the upstream naming so a file downloaded by hand, or copied from
    /// a whisper.cpp checkout, is recognised without being renamed.
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

fn entry(
    id: &str,
    label: &'static str,
    note: &'static str,
    bytes: u64,
    sha256: &'static str,
    english_only: bool,
) -> ModelInfo {
    ModelInfo {
        id: ModelId::new(id),
        label,
        note,
        bytes,
        sha256,
        url: Box::leak(
            format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{id}.bin")
                .into_boxed_str(),
        ),
        english_only,
    }
}

/// Ordered smallest to largest, which doubles as fastest to most accurate.
///
/// A `LazyLock` rather than a plain `static` because [`ModelId`] owns a
/// `String`, and one id type everywhere is worth more than a const-friendly
/// representation that would need converting at every boundary.
///
/// Sizes and digests come from the upstream repository's LFS metadata and were
/// verified against a real file. Do not edit them by hand.
pub static CATALOG: LazyLock<Vec<ModelInfo>> = LazyLock::new(|| {
    vec![
        entry(
            "tiny.en",
            "Tiny (English)",
            "Fastest and least accurate. For older or busy machines.",
            77_704_715,
            "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
            true,
        ),
        entry(
            "base.en",
            "Base (English)",
            "The default. Fast, and accurate enough for dictation.",
            147_964_211,
            "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
            true,
        ),
        entry(
            "small",
            "Small (multilingual)",
            "Handles languages other than English. Set a language to match.",
            487_601_967,
            "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
            false,
        ),
        entry(
            "small.en",
            "Small (English)",
            "Noticeably better than Base, at a few seconds more per dictation.",
            487_614_201,
            "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
            true,
        ),
        entry(
            "medium.en",
            "Medium (English)",
            "High accuracy. Slow on machines without memory to spare.",
            1_533_774_781,
            "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
            true,
        ),
        entry(
            "large-v3-turbo",
            "Large v3 Turbo (multilingual)",
            "The most accurate option. Large download, and wants a fast machine.",
            1_624_555_275,
            "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
            false,
        ),
    ]
});

/// The entry for `id`, or `None` if the catalog does not have it.
pub fn find(id: &ModelId) -> Option<&'static ModelInfo> {
    CATALOG.iter().find(|info| &info.id == id)
}

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
        for info in CATALOG.iter() {
            assert!(seen.insert(&info.id), "duplicate catalog id {:?}", info.id);
        }
    }

    /// The pane lists these top to bottom and the order doubles as a rough
    /// speed-versus-accuracy ordering, so it is worth asserting rather than
    /// leaving to whoever edits the list next.
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
    fn every_digest_is_a_lowercase_sha256() {
        for info in CATALOG.iter() {
            assert_eq!(
                info.sha256.len(),
                64,
                "{:?} digest is not 64 characters",
                info.id
            );
            assert!(
                info.sha256
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{:?} digest must be lowercase hex",
                info.id
            );
        }
    }

    /// The manager derives the download destination from the id, so a mismatch
    /// between the file name and the URL would install a model under a name
    /// nothing looks for.
    #[test]
    fn each_url_ends_with_the_file_the_id_names() {
        for info in CATALOG.iter() {
            assert!(
                info.url.ends_with(&info.id.file_name()),
                "{} does not end with {}",
                info.url,
                info.id.file_name()
            );
        }
    }
}
