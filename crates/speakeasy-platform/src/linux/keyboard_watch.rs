//! No real device-based detection on X11 yet — mirrors `X11Hotkeys::register`
//! in `hotkey.rs`, which is also unimplemented for the same underlying
//! reason (no XInput2 device-enumeration wiring exists in this crate yet).
//! Always reports the built-in bucket, so the rest of the app still has a
//! bucket to bind against.

use speakeasy_core::{KeyboardBucket, KeyboardBucketCallback, KeyboardWatcher, Result};

#[derive(Default)]
pub struct X11KeyboardWatcher;

impl X11KeyboardWatcher {
    pub fn new() -> Self {
        Self
    }
}

impl KeyboardWatcher for X11KeyboardWatcher {
    fn watch(&self, on_change: KeyboardBucketCallback) -> Result<()> {
        on_change(KeyboardBucket::BuiltIn);
        Ok(())
    }
}
