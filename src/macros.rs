//! Macro bindings with TeX's scoping. The first pass records every change in
//! source order; a reader asks for the bindings visible at any offset.
//!
//! Local assignments save the previous binding in the innermost group, once per
//! group, and leaving the group restores it. A global assignment marks the
//! binding global, so leaving groups keeps it. Each change creates a version;
//! earlier versions stay readable, so contexts and checkpoints share state.

use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

const UNDEFINED: u32 = u32::MAX;
const POISONED: u32 = u32::MAX - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingKind {
    /// `\newcommand` and its relatives.
    Command,
    /// `\def` and its relatives, with TeX parameter text.
    Def,
    /// `\let` to a command the document does not define, such as a KaTeX
    /// built-in: the body names it.
    Let,
    /// `\newenvironment`: the body is the begin code, `end` the end code.
    Environment,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Binding {
    /// `\name` for commands, `name` for environments.
    pub name: String,
    pub kind: BindingKind,
    /// The number of parameters, including an optional first one.
    pub params: u8,
    /// The default of an optional first parameter.
    pub default: Option<String>,
    /// For `\def`: the parameter text, such as `#1#2` or `#1.`.
    pub parameter_text: Option<String>,
    pub body: String,
    /// For environments: the end code.
    pub end: Option<String>,
}

impl Binding {
    /// A definition KaTeX can replay with persistence enabled, for bindings that
    /// a plain macro string cannot express. Environments return `None`.
    pub fn katex_definition(&self) -> Option<String> {
        Some(match self.kind {
            BindingKind::Environment => return None,
            BindingKind::Def => format!(
                "\\gdef{}{}{{{}}}",
                self.name,
                self.parameter_text.as_deref().unwrap_or_default(),
                self.body
            ),
            BindingKind::Let => format!("\\global\\let{}{}", self.name, self.body),
            BindingKind::Command => {
                let mut out = format!("\\newcommand{{{}}}", self.name);
                if self.params > 0 {
                    out.push_str(&format!("[{}]", self.params));
                }
                if let Some(default) = &self.default {
                    out.push_str(&format!("[{default}]"));
                }
                out.push_str(&format!("{{{}}}", self.body));
                out
            }
        })
    }

    /// The body as a plain macro string, when parameters are only `#1`..`#9`
    /// without an optional default: KaTeX derives the argument count from it.
    pub fn simple_body(&self) -> Option<&str> {
        match self.kind {
            BindingKind::Command if self.default.is_none() => Some(&self.body),
            BindingKind::Def
                if self
                    .parameter_text
                    .as_deref()
                    .is_none_or(|text| is_simple_parameters(text, self.params)) =>
            {
                Some(&self.body)
            }
            BindingKind::Let => Some(&self.body),
            _ => None,
        }
    }

    fn hash128(&self) -> u128 {
        hash128(self)
    }
}

fn is_simple_parameters(text: &str, params: u8) -> bool {
    let expected: String = (1..=params).map(|n| format!("#{n}")).collect();
    text == expected
}

fn hash128(value: &impl Hash) -> u128 {
    let mut low = DefaultHasher::new();
    0u8.hash(&mut low);
    value.hash(&mut low);
    let mut high = DefaultHasher::new();
    1u8.hash(&mut high);
    value.hash(&mut high);
    (u128::from(high.finish()) << 64) | u128::from(low.finish())
}

#[derive(Clone, Copy, Debug)]
struct Change {
    offset: usize,
    version: u32,
    hash: u128,
    poisoned: u32,
}

#[derive(Clone, Copy, Debug)]
struct Current {
    binding: u32,
    global: bool,
}

/// What a lookup found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lookup<'a> {
    Undefined,
    /// A definition that could not be applied: content using it must fall back.
    Unknown,
    Bound(&'a Binding),
}

#[derive(Debug, Default)]
pub(crate) struct Store {
    names: HashMap<Box<str>, u32>,
    histories: Vec<Vec<(u32, u32)>>,
    bindings: Vec<Binding>,
    changes: Vec<Change>,
    /// Offset after which the assignment budget was exhausted.
    overflow: Option<usize>,
    // State of the first pass.
    current: Vec<Current>,
    saves: Vec<Vec<(u32, Current)>>,
    version: u32,
    hash: u128,
    poisoned: u32,
    assignments: usize,
    bytes: usize,
    max_assignments: usize,
    max_bytes: usize,
}

/// The result of an assignment during the first pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Assigned {
    Applied,
    /// The binding became unknown: malformed, or over a size limit.
    Poisoned,
    /// The assignment budget is exhausted; later contexts are incomplete.
    Overflow,
}

impl Store {
    pub fn new(max_assignments: usize, max_bytes: usize) -> Self {
        Self {
            max_assignments,
            max_bytes,
            ..Self::default()
        }
    }

    fn id(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.names.get(name) {
            return id;
        }
        let id = self.histories.len() as u32;
        self.names.insert(name.into(), id);
        self.histories.push(Vec::new());
        self.current.push(Current {
            binding: UNDEFINED,
            global: false,
        });
        id
    }

    fn binding_hash(&self, id: u32, binding: u32) -> u128 {
        match binding {
            UNDEFINED => 0,
            POISONED => hash128(&(id, "unknown")),
            index => self.bindings[index as usize].hash128(),
        }
    }

    fn set(&mut self, id: u32, value: Current, offset: usize) {
        let old = self.current[id as usize];
        self.hash = self
            .hash
            .wrapping_sub(self.binding_hash(id, old.binding))
            .wrapping_add(self.binding_hash(id, value.binding));
        self.poisoned = self.poisoned - u32::from(old.binding == POISONED)
            + u32::from(value.binding == POISONED);
        self.current[id as usize] = value;
        self.version += 1;
        self.histories[id as usize].push((self.version, value.binding));
        self.changes.push(Change {
            offset,
            version: self.version,
            hash: self.hash,
            poisoned: self.poisoned,
        });
    }

    pub fn enter(&mut self) {
        self.saves.push(Vec::new());
    }

    pub fn leave(&mut self, offset: usize) {
        let Some(saved) = self.saves.pop() else {
            return;
        };
        for (id, previous) in saved.into_iter().rev() {
            if !self.current[id as usize].global {
                self.set(id, previous, offset);
            }
        }
    }

    /// Applies `binding` to `name`; `None` makes the name unknown.
    pub fn assign(
        &mut self,
        name: &str,
        binding: Option<Binding>,
        global: bool,
        offset: usize,
    ) -> Assigned {
        if self.overflow.is_some() {
            return Assigned::Overflow;
        }
        self.assignments += 1;
        let size = binding.as_ref().map_or(0, |b| {
            b.body.len() + b.end.as_ref().map_or(0, String::len) + b.name.len()
        });
        if self.assignments > self.max_assignments || self.bytes + size > self.max_bytes {
            self.overflow = Some(offset);
            return Assigned::Overflow;
        }
        self.bytes += size;
        let id = self.id(name);
        let index = match binding {
            Some(binding) => {
                self.bindings.push(binding);
                (self.bindings.len() - 1) as u32
            }
            None => POISONED,
        };
        if !global
            && let Some(frame) = self.saves.last_mut()
            && !frame.iter().any(|(saved, _)| *saved == id)
        {
            frame.push((id, self.current[id as usize]));
        }
        self.set(
            id,
            Current {
                binding: index,
                global,
            },
            offset,
        );
        if index == POISONED {
            Assigned::Poisoned
        } else {
            Assigned::Applied
        }
    }

    fn resolve(&self, binding: u32) -> Lookup<'_> {
        match binding {
            UNDEFINED => Lookup::Undefined,
            POISONED => Lookup::Unknown,
            index => Lookup::Bound(&self.bindings[index as usize]),
        }
    }

    /// The binding during the first pass.
    pub fn current(&self, name: &str) -> Lookup<'_> {
        match self.names.get(name) {
            Some(&id) => self.resolve(self.current[id as usize].binding),
            None => Lookup::Undefined,
        }
    }

    fn change_at(&self, offset: usize) -> Option<&Change> {
        let index = self.changes.partition_point(|c| c.offset <= offset);
        index.checked_sub(1).map(|i| &self.changes[i])
    }

    pub fn version_at(&self, offset: usize) -> u32 {
        self.change_at(offset).map_or(0, |c| c.version)
    }

    pub fn lookup(&self, name: &str, version: u32) -> Lookup<'_> {
        let Some(&id) = self.names.get(name) else {
            return Lookup::Undefined;
        };
        let history = &self.histories[id as usize];
        let index = history.partition_point(|(v, _)| *v <= version);
        match index.checked_sub(1) {
            Some(i) => self.resolve(history[i].1),
            None => Lookup::Undefined,
        }
    }

    /// Releases first-pass state before the store is shared with readers.
    pub fn finish(&mut self) {
        self.current = Vec::new();
        self.saves = Vec::new();
    }
}

/// The macro bindings visible at a position, for rendering formulas.
#[derive(Clone)]
pub struct MacroContext {
    store: Option<Arc<Store>>,
    version: u32,
    hash: u128,
    incomplete: bool,
}

impl std::fmt::Debug for MacroContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacroContext")
            .field("hash", &format_args!("{:032x}", self.hash))
            .field("incomplete", &self.incomplete)
            .finish()
    }
}

impl PartialEq for MacroContext {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.incomplete == other.incomplete
    }
}

impl MacroContext {
    pub(crate) fn empty() -> Self {
        Self {
            store: None,
            version: 0,
            hash: 0,
            incomplete: false,
        }
    }

    pub(crate) fn at(store: &Arc<Store>, offset: usize) -> Self {
        let change = store.change_at(offset);
        let incomplete = change.is_some_and(|c| c.poisoned > 0)
            || store.overflow.is_some_and(|overflow| offset >= overflow);
        Self {
            store: Some(Arc::clone(store)),
            version: change.map_or(0, |c| c.version),
            hash: change.map_or(0, |c| c.hash),
            incomplete,
        }
    }

    /// Identifies the visible bindings: equal bindings give equal hashes
    /// however they were reached. Use it in cache keys.
    pub fn hash(&self) -> u128 {
        self.hash
    }

    /// A definition visible here could not be applied, or the definition
    /// budget was exhausted before this point. Formulas should show source.
    pub fn is_incomplete(&self) -> bool {
        self.incomplete
    }

    pub fn is_empty(&self) -> bool {
        self.hash == 0 && !self.incomplete
    }

    /// The visible command bindings, one per name, in no particular order.
    pub fn bindings(&self) -> impl Iterator<Item = &Binding> {
        let store = self.store.as_deref();
        store
            .into_iter()
            .flat_map(|store| store.names.keys().map(move |name| (store, name)))
            .filter_map(|(store, name)| match store.lookup(name, self.version) {
                Lookup::Bound(binding) if binding.kind != BindingKind::Environment => Some(binding),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, body: &str) -> Binding {
        Binding {
            name: name.into(),
            kind: BindingKind::Command,
            params: 0,
            default: None,
            parameter_text: None,
            body: body.into(),
            end: None,
        }
    }

    fn body(store: &Store, name: &str, offset: usize) -> Option<String> {
        match store.lookup(name, store.version_at(offset)) {
            Lookup::Bound(binding) => Some(binding.body.clone()),
            Lookup::Unknown => Some("?".into()),
            Lookup::Undefined => None,
        }
    }

    #[test]
    fn global_assignments_survive_groups_and_earlier_versions_remain() {
        // \def\x{a}{\gdef\x{b}} and {\def\x{c}\gdef\x{b}}
        let mut store = Store::new(100, 1 << 20);
        store.assign("\\x", Some(command("\\x", "a")), false, 10);
        store.enter();
        store.assign("\\x", Some(command("\\x", "b")), true, 20);
        store.leave(30);
        store.assign("\\y", Some(command("\\y", "a")), false, 40);
        store.enter();
        store.assign("\\y", Some(command("\\y", "c")), false, 50);
        store.assign("\\y", Some(command("\\y", "b")), true, 60);
        store.leave(70);
        assert_eq!(body(&store, "\\x", 15).as_deref(), Some("a"));
        assert_eq!(body(&store, "\\x", 25).as_deref(), Some("b"));
        assert_eq!(body(&store, "\\x", 35).as_deref(), Some("b"));
        assert_eq!(body(&store, "\\y", 55).as_deref(), Some("c"));
        assert_eq!(body(&store, "\\y", 75).as_deref(), Some("b"));
        assert_eq!(body(&store, "\\x", 5), None);
    }

    #[test]
    fn local_definitions_end_with_their_group_and_hashes_follow_content() {
        let mut store = Store::new(100, 1 << 20);
        store.enter();
        store.assign("\\x", Some(command("\\x", "a")), false, 10);
        store.leave(20);
        store.assign("\\x", Some(command("\\x", "a")), false, 30);
        let store = Arc::new(store);
        assert_eq!(body(&store, "\\x", 25), None);
        let inside = MacroContext::at(&store, 15);
        let after = MacroContext::at(&store, 35);
        assert_eq!(inside.hash(), after.hash());
        assert_ne!(inside.hash(), MacroContext::at(&store, 25).hash());
        assert!(MacroContext::at(&store, 25).is_empty());
        assert_eq!(after.bindings().count(), 1);
    }

    #[test]
    fn rejected_definitions_make_contexts_incomplete_until_their_group_ends() {
        let mut store = Store::new(100, 1 << 20);
        store.assign("\\x", Some(command("\\x", "a")), false, 10);
        store.enter();
        assert_eq!(store.assign("\\x", None, false, 20), Assigned::Poisoned);
        store.leave(30);
        let store = Arc::new(store);
        assert!(MacroContext::at(&store, 25).is_incomplete());
        assert_eq!(body(&store, "\\x", 25).as_deref(), Some("?"));
        assert!(!MacroContext::at(&store, 35).is_incomplete());
        assert_eq!(body(&store, "\\x", 35).as_deref(), Some("a"));
    }

    #[test]
    fn the_assignment_budget_marks_later_contexts_incomplete() {
        let mut store = Store::new(1, 1 << 20);
        store.assign("\\x", Some(command("\\x", "a")), false, 10);
        assert_eq!(
            store.assign("\\x", Some(command("\\x", "b")), false, 20),
            Assigned::Overflow
        );
        let store = Arc::new(store);
        assert!(!MacroContext::at(&store, 15).is_incomplete());
        assert!(MacroContext::at(&store, 20).is_incomplete());
    }
}
