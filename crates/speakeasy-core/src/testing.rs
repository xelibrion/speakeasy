//! Test-only helpers shared across the crate's unit tests.

/// A directory of our own under the system temp dir, removed on drop.
///
/// Named after the test that made it rather than randomly, so a directory left
/// behind by a panicking test is traceable to the test that leaked it.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        // The thread id is in the name because `cargo test` runs tests in
        // parallel within one process, so the pid alone is shared between them.
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
