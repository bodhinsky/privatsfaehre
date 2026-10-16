//! Labeled dataset and evaluation.
//!
//! # Format
//!
//! One JSON object per line ([`Record`]). Offsets in the dataset are
//! **Unicode scalar (character) offsets**, not bytes, so the files can be
//! edited with tools in other languages and spreadsheets; every span also
//! stores its `text`, which is checked on load.
//!
//! Files are append-only logs: a later line with the same `id` replaces an
//! earlier one ([`load`]).
//!
//! Also supported: an inline markup format for hand-written samples
//! ([`markup`]) and a span-per-row CSV for spreadsheet review ([`csv_io`]).

pub mod csv_io;
pub mod eval;
mod format;
pub mod markup;

pub use format::*;
