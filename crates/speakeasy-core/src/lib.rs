//! Shell-agnostic engine for Speakeasy.
//!
//! Everything here is portable: no tauri, no cpal, no whisper, no OS APIs.
//! Platform capabilities arrive through the traits in [`ports`], and the shell
//! (Tauri today, SwiftUI later) observes progress through the event stream
//! returned by [`Engine::subscribe`].
//!
//! # API shape
//!
//! The public surface is deliberately flat — owned types, no generics or
//! lifetimes in signatures, errors as plain enums. That is what `uniffi` and
//! `swift-bridge` can express, so binding this crate to Swift later is a
//! mechanical step rather than a redesign.

pub mod config;
pub mod engine;
pub mod error;
pub mod event;
pub mod hotkey;
pub mod models;
pub mod ports;
pub mod preview;

#[cfg(feature = "fakes")]
pub mod fakes;

#[cfg(test)]
mod testing;

pub use config::{
    ActivationMode, Config, Hotkey, InjectionMode, ModelChoice, ModifierKey, WidgetPosition,
};
pub use engine::{Engine, HotkeyAction};
pub use error::{Error, Result};
pub use event::{Event, ModelStatus, State};
pub use hotkey::{ChordMods, KeyCode};
pub use models::{ModelId, ModelInfo};
pub use ports::{
    AudioSource, CaptureCallback, HotkeyCallback, HotkeyEvent, HotkeyListener, SpeechSegment,
    SpeechSegmenter, TextSink, Transcriber,
};

/// Sample rate every [`Transcriber`] implementation expects.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Number of bands in [`AudioSource::spectrum`].
///
/// Fixed rather than a parameter so the shell and the platform layer cannot
/// disagree about the length of the slice they are passing around. Twelve reads
/// as an equaliser at the width the widget has to spare.
pub const SPECTRUM_BANDS: usize = 12;
