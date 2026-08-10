//! Checks the fetcher against the real distribution.
//!
//! Ignored by default: it downloads ~78 MB, so it cannot run in a plain
//! `cargo test`. Run it after editing the catalog — it is the only thing that
//! proves the sizes and digests in `speakeasy-core::models::catalog` still
//! describe the files actually being served.
//!
//! ```sh
//! cargo test -p speakeasy-models --test realfetch -- --ignored --nocapture
//! ```

use std::sync::atomic::AtomicBool;

use speakeasy_core::models::CATALOG;
use speakeasy_core::ports::ModelFetcher;
use speakeasy_models::HttpFetcher;

/// The smallest catalog entry, so the check stays cheap. A wrong digest on any
/// entry is almost always a wrong *process*, which this would catch.
const MODEL: &str = "tiny.en";

#[test]
#[ignore = "downloads ~78 MB from the real host"]
fn a_real_catalog_model_downloads_and_matches_its_digest() {
    use sha2::{Digest, Sha256};

    let info = CATALOG
        .iter()
        .find(|info| info.id.as_str() == MODEL)
        .expect("the catalog should have the model this test names");

    let destination = std::env::temp_dir().join(format!("speakeasy-realfetch-{MODEL}.bin"));
    let _ = std::fs::remove_file(&destination);

    HttpFetcher::new()
        .fetch(
            info.url,
            &destination,
            &|received, total| eprintln!("{received}/{total}"),
            &AtomicBool::new(false),
        )
        .expect("the real fetch should succeed");

    let size = std::fs::metadata(&destination)
        .expect("the file should exist")
        .len();
    assert_eq!(size, info.bytes, "the catalog's size is stale");

    let mut file = std::fs::File::open(&destination).expect("the file should open");
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).expect("hashing should succeed");
    assert_eq!(
        format!("{:x}", hasher.finalize()),
        info.sha256,
        "the catalog's digest is stale"
    );

    let _ = std::fs::remove_file(&destination);
}
