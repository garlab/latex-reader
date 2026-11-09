//! Fast, approximate reading of LaTeX documents for previews.
//!
//! [`index`] reads a document once and returns an [`Index`]: macro bindings
//! with TeX's scoping, equation and section numbers, labels, an outline, and
//! checkpoints. Events then come from the start of the document, or from any
//! checkpoint, with the same results:
//!
//! ```
//! use std::sync::atomic::AtomicBool;
//! use latex_preview::{Event, Options, Tag};
//!
//! let source = r"\section{Intro} Let $x \in \mathbb{R}$.";
//! let index = latex_preview::index(source, &Options::default(), &AtomicBool::new(false)).unwrap();
//! let events: Vec<_> = index.events(source).map(|s| s.event).collect();
//! assert!(matches!(&events[0], Event::Start(Tag::Heading { level: 2, number: Some(n) }) if n == "1"));
//! ```
//!
//! The crate does not render or read files. Formulas come out as TeX prepared
//! for KaTeX, with a [`MacroContext`] that identifies the macro bindings
//! visible where each formula starts.

mod commands;
mod diagnostics;
mod event;
mod index;
mod lexer;
mod macros;
mod math;
mod numbering;
mod parser;
mod text;

pub use diagnostics::{Diagnostic, DiagnosticKind, Diagnostics};
pub use event::*;
pub use index::{Cancelled, Checkpoint, Events, Index, Limits, Options, OutlineEntry, index};
pub use macros::{Binding, BindingKind, MacroContext};
