//! The first pass and event windows.

use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};

use crate::{
    diagnostics::{DiagnosticKind, Diagnostics},
    event::{Event, Spanned, Tag, TagKind},
    macros::Store,
    numbering::Registry,
    parser::{Ctx, Parser, Phase, State},
};

/// Resource limits. Exceeding one keeps the affected region as source, with a
/// diagnostic, and marks state that depends on it as uncertain.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Macro and environment assignments, including redefinitions.
    pub max_assignments: usize,
    /// Total size of definition bodies.
    pub max_definition_bytes: usize,
    pub max_nesting: usize,
    pub max_expansion_depth: usize,
    /// Text-mode macro expansions per run.
    pub max_expansion_steps: usize,
    /// Size of one expansion.
    pub max_expansion_bytes: usize,
    /// Size of one formula.
    pub max_math: usize,
    pub max_labels: usize,
    /// Diagnostics kept with positions.
    pub max_diagnostics: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_assignments: 16_384,
            max_definition_bytes: 1 << 20,
            max_nesting: 256,
            max_expansion_depth: 32,
            max_expansion_steps: 1_000_000,
            max_expansion_bytes: 64 * 1024,
            max_math: 64 * 1024,
            max_labels: 65_536,
            max_diagnostics: 256,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// An opaque revision supplied by the host, returned by [`Index::revision`].
    pub revision: u64,
    /// Source bytes between checkpoints, at the next safe boundary.
    pub checkpoint_spacing: usize,
    /// Indexing stops at this many checkpoints; later content is source.
    pub max_checkpoints: usize,
    pub limits: Limits,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            revision: 0,
            checkpoint_spacing: 32 * 1024,
            max_checkpoints: 1 << 20,
            limits: Limits::default(),
        }
    }
}

/// The first pass or a window was cancelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// A point where reading can resume, with the complete reader state.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub(crate) offset: usize,
    pub(crate) state: State,
}

impl Checkpoint {
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Tags open at the checkpoint, outermost first. A window's events
    /// continue inside them.
    pub fn open_tags(&self) -> Vec<TagKind> {
        self.state.open_tags()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineEntry {
    pub level: u8,
    pub number: Option<String>,
    pub title: String,
    pub range: Range<usize>,
}

pub(crate) struct Builder {
    pub options: Options,
    pub store: Store,
    pub labels: HashMap<String, Arc<str>>,
    pub bibliography: HashMap<String, String>,
    pub registry: Registry,
    pub title: [Option<String>; 3],
    pub diagnostics: Diagnostics,
    outline: Vec<OutlineEntry>,
    heading: Option<OutlineEntry>,
    checkpoints: Vec<Checkpoint>,
    pub last_checkpoint: usize,
    stopped: Option<usize>,
}

impl Builder {
    fn new(options: &Options) -> Self {
        let limits = &options.limits;
        Self {
            store: Store::new(limits.max_assignments, limits.max_definition_bytes),
            labels: HashMap::new(),
            bibliography: HashMap::new(),
            registry: Registry::new(false),
            title: [None, None, None],
            diagnostics: Diagnostics::new(limits.max_diagnostics),
            outline: Vec::new(),
            heading: None,
            checkpoints: Vec::new(),
            last_checkpoint: 0,
            stopped: None,
            options: options.clone(),
        }
    }

    pub fn capturing(&self) -> bool {
        self.heading.is_some()
    }

    pub fn observe(&mut self, event: &Event<'_>, range: &Range<usize>) {
        match event {
            Event::Start(Tag::Heading { level, number }) => {
                self.heading = Some(OutlineEntry {
                    level: *level,
                    number: number.clone(),
                    title: String::new(),
                    range: range.clone(),
                });
            }
            Event::Text(text) => {
                if let Some(heading) = &mut self.heading {
                    heading.title.push_str(text);
                }
            }
            Event::End(TagKind::Heading) => {
                if let Some(mut heading) = self.heading.take() {
                    heading.range.end = heading.range.end.max(range.end);
                    heading.title = heading.title.trim().to_owned();
                    self.outline.push(heading);
                }
            }
            _ => {}
        }
    }

    /// Records a checkpoint; `false` when the cap stops indexing here.
    pub fn record(&mut self, offset: usize, state: State) -> bool {
        if self.checkpoints.len() >= self.options.max_checkpoints {
            self.stopped = Some(offset);
            self.diagnostics
                .push(DiagnosticKind::IndexStopped, None, offset..offset);
            return false;
        }
        self.checkpoints.push(Checkpoint { offset, state });
        self.last_checkpoint = offset;
        true
    }
}

/// The result of the first pass: checkpoints, macro bindings, labels, and the
/// outline. Read events from the start with [`Index::events`], or from any
/// checkpoint with [`Index::window`].
pub struct Index {
    pub(crate) options: Options,
    pub(crate) checkpoints: Vec<Checkpoint>,
    pub(crate) store: Arc<Store>,
    pub(crate) labels: HashMap<String, Arc<str>>,
    pub(crate) bibliography: HashMap<String, String>,
    pub(crate) registry: Registry,
    pub(crate) title: [Option<String>; 3],
    diagnostics: Diagnostics,
    outline: Vec<OutlineEntry>,
    stopped: Option<usize>,
    len: usize,
}

fn has_document(source: &str) -> bool {
    source.match_indices("\\begin{document}").any(|(at, _)| {
        let line = &source[source[..at].rfind('\n').map_or(0, |i| i + 1)..at];
        !line.contains('%')
    })
}

/// Reads `source` once, building the index. Polls `cancel` every 64 KiB.
pub fn index(source: &str, options: &Options, cancel: &AtomicBool) -> Result<Index, Cancelled> {
    let mut builder = Builder::new(options);
    let state = State::new(has_document(source));
    builder.checkpoints.push(Checkpoint {
        offset: 0,
        state: state.clone(),
    });
    let mut parser = Parser::new(
        source,
        Ctx::Build(Box::new(builder)),
        0,
        source.len(),
        state,
        Some(cancel),
    );
    while parser.step() {}
    if parser.cancelled {
        return Err(Cancelled);
    }
    let Ctx::Build(builder) = parser.ctx else {
        unreachable!("the first pass builds")
    };
    let mut builder = *builder;
    builder.store.finish();
    Ok(Index {
        options: builder.options,
        checkpoints: builder.checkpoints,
        store: Arc::new(builder.store),
        labels: builder.labels,
        bibliography: builder.bibliography,
        registry: builder.registry,
        title: builder.title,
        diagnostics: builder.diagnostics,
        outline: builder.outline,
        stopped: builder.stopped,
        len: source.len(),
    })
}

impl Index {
    pub fn revision(&self) -> u64 {
        self.options.revision
    }

    /// The length of the indexed source, to check a snapshot matches.
    pub fn source_len(&self) -> usize {
        self.len
    }

    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// The last checkpoint at or before `offset`.
    pub fn checkpoint_before(&self, offset: usize) -> usize {
        self.checkpoints
            .partition_point(|c| c.offset <= offset)
            .saturating_sub(1)
    }

    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    pub fn outline(&self) -> &[OutlineEntry] {
        &self.outline
    }

    /// The document has a `\title`, so `\maketitle` produces a title block.
    pub fn has_title(&self) -> bool {
        self.title[0].is_some()
    }

    /// The document defines chapters, so sections are one level deeper.
    pub fn has_chapters(&self) -> bool {
        self.registry.chapters
    }

    /// The offset where indexing stopped at the checkpoint cap.
    pub fn stopped(&self) -> Option<usize> {
        self.stopped
    }

    /// What `\ref` gives for a label.
    pub fn label(&self, name: &str) -> Option<&str> {
        self.labels.get(name).map(|v| &**v)
    }

    fn end_of(&self, checkpoint: usize) -> usize {
        self.checkpoints
            .get(checkpoint + 1)
            .map(|c| c.offset)
            .or(self.stopped)
            .unwrap_or(self.len)
    }

    /// Events for the whole document.
    pub fn events<'a>(&'a self, source: &'a str) -> Events<'a> {
        self.read(source, 0, self.stopped.unwrap_or(self.len))
    }

    /// Events from checkpoint `checkpoint` up to the next one. Concatenating
    /// every window gives the same events as [`Index::events`].
    pub fn window<'a>(&'a self, source: &'a str, checkpoint: usize) -> Events<'a> {
        let checkpoint = checkpoint.min(self.checkpoints.len() - 1);
        self.read(source, checkpoint, self.end_of(checkpoint))
    }

    fn read<'a>(&'a self, source: &'a str, checkpoint: usize, end: usize) -> Events<'a> {
        assert_eq!(
            source.len(),
            self.len,
            "the source must be the indexed revision"
        );
        let start = &self.checkpoints[checkpoint];
        let tail = (self.stopped == Some(end)).then_some(end..self.len);
        Events {
            parser: Parser::new(
                source,
                Ctx::Read(self),
                start.offset,
                end,
                start.state.clone(),
                None,
            ),
            source,
            tail,
            finished: false,
        }
    }
}

/// An iterator over events. Stops early, with [`Events::cancelled`] set, when
/// its cancellation flag is raised.
pub struct Events<'a> {
    parser: Parser<'a>,
    source: &'a str,
    /// Source past the checkpoint cap, shown as is.
    tail: Option<Range<usize>>,
    finished: bool,
}

impl<'a> Events<'a> {
    pub fn with_cancel(mut self, cancel: &'a AtomicBool) -> Self {
        self.parser.set_cancel(cancel);
        self
    }

    pub fn cancelled(&self) -> bool {
        self.parser.cancelled
    }
}

const RAW_CHUNK: usize = 64 * 1024;

impl<'a> Iterator for Events<'a> {
    type Item = Spanned<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(event) = self.parser.out.pop_front() {
                return Some(event);
            }
            if !self.finished {
                if !self.parser.step() {
                    // Drain the events of the last step before the tail.
                    self.finished = true;
                }
                continue;
            }
            if self.parser.cancelled || self.parser.st.phase == Phase::Preamble {
                return None;
            }
            let tail = self.tail.as_mut()?;
            if tail.start >= tail.end {
                return None;
            }
            let mut end = (tail.start + RAW_CHUNK).min(tail.end);
            while !self.source.is_char_boundary(end) {
                end += 1;
            }
            if end < tail.end
                && let Some(newline) = self.source[tail.start..end].rfind('\n')
            {
                end = tail.start + newline + 1;
            }
            let range = tail.start..end;
            tail.start = end;
            return Some(Spanned {
                event: Event::Raw(std::borrow::Cow::Borrowed(&self.source[range.clone()])),
                range,
            });
        }
    }
}
