//! Counters, their reset relations and formats, and theorem declarations.
//! Values live in the parser state, so checkpoints carry them; this registry
//! holds what does not change while reading the body.

use std::collections::HashMap;

use crate::{event::TheoremStyle, text};

pub(crate) const CHAPTER: usize = 1;
pub(crate) const SECTION: usize = 2;
pub(crate) const EQUATION: usize = 7;
pub(crate) const FIGURE: usize = 8;
pub(crate) const TABLE: usize = 9;
pub(crate) const FOOTNOTE: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Arabic,
    RomanUpper,
    AlphUpper,
}

impl Format {
    pub fn apply(self, value: u32) -> String {
        match self {
            Format::Arabic => value.to_string(),
            Format::RomanUpper => text::roman(value, true),
            Format::AlphUpper => text::alph(value, true),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Counter {
    pub within: Option<usize>,
    pub format: Format,
}

#[derive(Clone, Debug)]
pub(crate) struct Theorem {
    pub display: String,
    pub counter: Option<usize>,
    pub style: TheoremStyle,
}

#[derive(Clone, Debug)]
pub(crate) struct Registry {
    pub counters: Vec<Counter>,
    names: HashMap<String, usize>,
    pub theorems: HashMap<String, Theorem>,
    pub style: TheoremStyle,
    pub chapters: bool,
    /// Sectioning levels numbered: 1 section, 2 subsection, and so on.
    pub secnumdepth: i32,
    pub amsmath: bool,
}

impl Registry {
    pub fn new(chapters: bool) -> Self {
        let mut registry = Self {
            counters: Vec::new(),
            names: HashMap::new(),
            theorems: HashMap::new(),
            style: TheoremStyle::Plain,
            chapters,
            secnumdepth: if chapters { 2 } else { 3 },
            amsmath: false,
        };
        let chapter = chapters.then_some(CHAPTER);
        for (name, within, format) in [
            ("part", None, Format::RomanUpper),
            ("chapter", None, Format::Arabic),
            ("section", chapter, Format::Arabic),
            ("subsection", Some(SECTION), Format::Arabic),
            ("subsubsection", Some(3), Format::Arabic),
            ("paragraph", Some(4), Format::Arabic),
            ("subparagraph", Some(5), Format::Arabic),
            ("equation", chapter, Format::Arabic),
            ("figure", chapter, Format::Arabic),
            ("table", chapter, Format::Arabic),
            ("footnote", chapter, Format::Arabic),
        ] {
            registry.add(name, within, format);
        }
        registry
    }

    pub fn add(&mut self, name: &str, within: Option<usize>, format: Format) -> usize {
        if let Some(&id) = self.names.get(name) {
            return id;
        }
        let id = self.counters.len();
        self.counters.push(Counter { within, format });
        self.names.insert(name.to_owned(), id);
        id
    }

    pub fn id(&self, name: &str) -> Option<usize> {
        self.names.get(name).copied()
    }

    /// Counters reset when `id` steps, directly or through another counter.
    pub fn descendants(&self, id: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut pending = vec![id];
        while let Some(parent) = pending.pop() {
            for (child, counter) in self.counters.iter().enumerate() {
                if counter.within == Some(parent) && !out.contains(&child) {
                    out.push(child);
                    pending.push(child);
                }
            }
        }
        out
    }

    /// The printed number, such as `2.3`, or `?` when a skipped region could
    /// have changed the counter or one it is numbered within.
    pub fn format(&self, id: usize, values: &[u32], uncertain: u64, appendix: bool) -> String {
        let mut parts = Vec::new();
        let mut current = Some(id);
        while let Some(counter) = current {
            if counter < 64 && uncertain & (1 << counter) != 0 {
                return "?".into();
            }
            let definition = &self.counters[counter];
            let lettered = appendix
                && ((counter == SECTION && !self.chapters)
                    || (counter == CHAPTER && self.chapters));
            let format = if lettered {
                Format::AlphUpper
            } else {
                definition.format
            };
            parts.push(format.apply(values.get(counter).copied().unwrap_or(0)));
            current = definition.within;
        }
        parts.reverse();
        parts.join(".")
    }
}
