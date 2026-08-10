mod text_sink;

use speakeasy_core::config::InjectionMode;
use speakeasy_core::TextSink;

pub use text_sink::X11TextSink;

pub fn text_sink(mode: InjectionMode) -> Box<dyn TextSink> {
    Box::new(X11TextSink::new(mode))
}

/// X11 has no permission model for synthetic input or capture.
pub fn permissions_granted() -> bool {
    true
}

pub fn open_permission_settings() {}
