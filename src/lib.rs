//! Reads LaTeX documents into semantic events, as LaTeX would number and
//! reference them, without a TeX engine.
//!
//! [`index`] reads a document once and returns an [`Index`]: macro bindings
//! with TeX's scoping, equation and section numbers, labels, an outline, and
//! checkpoints. Events then come from the start of the document, or from any
//! checkpoint, with the same results:
//!
//! ```
//! use std::sync::atomic::AtomicBool;
//! use latex_reader::{Event, Options, Tag};
//!
//! let source = r"\section{Intro} Let $x \in \mathbb{R}$.";
//! let index = latex_reader::index(source, &Options::default(), &AtomicBool::new(false)).unwrap();
//! let events: Vec<_> = index.events(source).map(|s| s.event).collect();
//! assert!(matches!(&events[0], Event::Start(Tag::Heading { level: 2, number: Some(n) }) if n == "1"));
//! ```
//!
//! The crate does not render or read files. Formulas come out as TeX prepared
//! for KaTeX, with a [`MacroContext`] that identifies the macro bindings
//! visible where each formula starts. [`expand`] splices files named by
//! `\input` and similar commands into the text before indexing, reading them
//! through a callback that keeps the application's path policy.

mod commands;
mod diagnostics;
mod event;
mod index;
mod inputs;
mod lexer;
mod macros;
mod math;
mod numbering;
mod parser;
mod text;

pub use diagnostics::{Diagnostic, DiagnosticKind, Diagnostics};
pub use event::*;
pub use index::{Cancelled, Checkpoint, Events, Index, Limits, Options, OutlineEntry, index};
pub use inputs::{Expanded, InputKind, InputLimits, expand};
pub use macros::{Binding, BindingKind, MacroContext};

/// The README's examples run as documentation tests.
#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDoctests;
