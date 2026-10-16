//! Detection engine for the clipboard guardian.
//!
//! The crate is pure Rust, performs no I/O and has no network capability. It
//! contains:
//!
//! * [`EntityType`]: the closed set of things we detect, each with a fixed
//!   [`Disposition`] (redact or block). GDPR Art. 9/10 categories are always
//!   [`Disposition::Block`]; this is decided at compile time and cannot be
//!   configured.
//! * [`Recognizer`]: the trait every detector implements. Built-in regex and
//!   checksum recognizers live in [`recognizers`].
//! * [`ner`]: a pluggable trait for local NER models (GLiNER via ONNX Runtime
//!   is the recommended backend, see `ARCHITECTURE.md`).
//! * [`Engine`]: runs all recognizers, applies allow/deny lists and thresholds
//!   and resolves overlapping spans.
//!
//! All offsets in this crate are **byte offsets** into the UTF-8 input.

mod checksum;
mod engine;
mod entity;
pub mod ner;
mod recognizer;
pub mod recognizers;
mod util;

pub use engine::{Analysis, DenyEntry, Engine, EngineConfig, SPECIAL_CATEGORY_THRESHOLD, Verdict};
pub use entity::{Disposition, EntityType, ParseEntityError};
pub use recognizer::{DetectError, Detection, Recognizer};
