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
pub mod ports;

#[cfg(feature = "fakes")]
pub mod fakes;

pub use config::Config;
pub use engine::Engine;
pub use error::{Error, Result};
pub use event::{Event, State};
pub use ports::{AudioSource, HotkeyCallback, HotkeyEvent, HotkeyListener, TextSink, Transcriber};

/// Sample rate every [`Transcriber`] implementation expects.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
