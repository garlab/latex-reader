//! Bounded diagnostics: the first occurrences keep positions; later ones only
//! increment per-name counts.

use std::{collections::BTreeMap, ops::Range};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DiagnosticKind {
    UnknownCommand,
    UnknownEnvironment,
    UnbalancedGroup,
    MismatchedEnd,
    UnterminatedMath,
    UnterminatedEnvironment,
    UnsupportedLabelPlacement,
    UnknownCounter,
    MalformedDefinition,
    /// A resource limit was exceeded; the name says which.
    LimitExceeded,
    /// Indexing stopped at the checkpoint cap; later content is source.
    IndexStopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub name: Option<String>,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    first: Vec<Diagnostic>,
    counts: BTreeMap<(DiagnosticKind, String), usize>,
    total: usize,
    capacity: usize,
}

impl Diagnostics {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }

    pub(crate) fn push(&mut self, kind: DiagnosticKind, name: Option<&str>, range: Range<usize>) {
        self.total += 1;
        let key = (kind, name.unwrap_or_default().to_owned());
        if let Some(count) = self.counts.get_mut(&key) {
            *count += 1;
            return;
        }
        if self.counts.len() < self.capacity.max(1) * 4 {
            self.counts.insert(key, 1);
        }
        if self.first.len() < self.capacity {
            self.first.push(Diagnostic {
                kind,
                name: name.map(str::to_owned),
                range,
            });
        }
    }

    /// The first diagnostics, in source order, with their positions.
    pub fn first(&self) -> &[Diagnostic] {
        &self.first
    }

    /// Occurrences per kind and name, including those without positions.
    pub fn counts(&self) -> impl Iterator<Item = (DiagnosticKind, &str, usize)> {
        self.counts
            .iter()
            .map(|((kind, name), count)| (*kind, name.as_str(), *count))
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}
