mod text_sink;

use speakeasy_core::config::InjectionMode;
use speakeasy_core::TextSink;

pub use text_sink::MacTextSink;

pub fn text_sink(mode: InjectionMode) -> Box<dyn TextSink> {
    Box::new(MacTextSink::new(mode))
}

/// Microphone (AVCaptureDevice) plus Accessibility (AXIsProcessTrusted).
/// Both are separate TCC grants and either can be revoked at any time.
pub fn permissions_granted() -> bool {
    // TODO: AXIsProcessTrustedWithOptions with kAXTrustedCheckOptionPrompt
    // false, and AVCaptureDevice.authorizationStatus(for: .audio).
    false
}

pub fn open_permission_settings() {
    // TODO: open
    // x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}
