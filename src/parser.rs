//! The reader. One engine runs the first pass, which builds the index, and
//! event windows, which read it. Both take the same path through the source,
//! so a window that starts at a checkpoint repeats the sequential events.

use std::{
    borrow::Cow,
    collections::VecDeque,
    ops::Range,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    commands::{self, Cmd, EnvClass, Language, MathEnv, Special},
    diagnostics::DiagnosticKind as D,
    event::*,
    index::{Builder, Index},
    lexer::{self, Kind, LexState},
    macros::{Assigned, Binding, BindingKind, Lookup, MacroContext},
    math::{self, Delim},
    numbering::{self, Format, Registry, Theorem},
    text,
};

const CANCEL_INTERVAL: usize = 64 * 1024;
const SYNTHETIC_TITLE: &str = "latexreadertitlepart";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Preamble,
    Body,
    Ended,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Para {
    /// Inline content is allowed: a paragraph, cell, or heading is open.
    open: bool,
    content: bool,
    space: Option<Range<usize>>,
    /// The next paragraph is indented, unless `suppress` is set.
    indent: bool,
    suppress: bool,
    /// Inline content of a cell rather than a paragraph.
    cell: bool,
    /// Inline content of a heading, caption, or similar block.
    block: bool,
    styles: Vec<Style>,
}

#[derive(Clone, Debug)]
struct Table {
    columns: Arc<[Column]>,
    row: bool,
    col: usize,
    span: u16,
    next: Option<(u16, Align)>,
}

#[derive(Clone, Debug)]
enum Env {
    Document,
    List {
        kind: ListKind,
        depth: u8,
        count: u32,
        item: bool,
    },
    Block(TagKind),
    Float(FigureKind),
    Table(Box<Table>),
    Bibliography {
        count: u32,
        item: bool,
    },
    Subequations {
        parent: String,
        count: u32,
    },
    Transparent,
    Unsupported,
    /// A `\newenvironment`, with its end code.
    User(Arc<str>),
}

#[derive(Clone, Copy, Debug)]
struct Inline {
    tag: TagKind,
    bracket: bool,
    unsupported: bool,
}

#[derive(Clone, Debug)]
enum FrameKind {
    Brace,
    Inline(Inline),
    Env(Arc<str>, Env),
}

#[derive(Clone, Debug)]
struct Frame {
    kind: FrameKind,
    styles: Vec<Style>,
    label: Arc<str>,
    para: Option<Box<Para>>,
}

#[derive(Clone, Debug)]
pub(crate) struct Verbatim {
    end: usize,
    marker: usize,
}

/// Everything that affects later events. Checkpoints store a copy.
#[derive(Clone, Debug)]
pub(crate) struct State {
    lex: LexState,
    stack: Vec<Frame>,
    counters: Vec<u32>,
    uncertain: u64,
    label: Arc<str>,
    para: Para,
    pub(crate) phase: Phase,
    appendix: bool,
    verbatim: Option<Verbatim>,
}

impl State {
    pub(crate) fn new(has_document: bool) -> Self {
        Self {
            lex: LexState::default(),
            stack: Vec::new(),
            counters: Vec::new(),
            uncertain: 0,
            label: Arc::from(""),
            para: Para {
                indent: true,
                ..Para::default()
            },
            phase: if has_document {
                Phase::Preamble
            } else {
                Phase::Body
            },
            appendix: false,
            verbatim: None,
        }
    }

    /// Tags open at this point, outermost first.
    pub(crate) fn open_tags(&self) -> Vec<TagKind> {
        let mut out = Vec::new();
        for frame in &self.stack {
            match &frame.kind {
                FrameKind::Env(_, env) => match env {
                    Env::List { item, .. } => {
                        out.push(TagKind::List);
                        if *item {
                            out.push(TagKind::Item);
                        }
                    }
                    Env::Block(tag) => out.push(*tag),
                    Env::Float(_) => out.push(TagKind::Figure),
                    Env::Table(table) => {
                        out.push(TagKind::Table);
                        if table.row {
                            out.push(TagKind::Row);
                        }
                    }
                    Env::Bibliography { item, .. } => {
                        out.push(TagKind::Bibliography);
                        if *item {
                            out.push(TagKind::BibItem);
                        }
                    }
                    Env::Unsupported => out.push(TagKind::Unsupported),
                    _ => {}
                },
                FrameKind::Inline(inline) => out.push(inline.tag),
                FrameKind::Brace => {}
            }
        }
        if self.verbatim.is_some() {
            out.push(TagKind::Code);
        }
        out
    }
}

/// Text from the source or from an expansion.
#[derive(Clone, Debug)]
pub(crate) enum Str<'a> {
    Src(&'a str),
    Shared(Rc<str>, usize, usize),
}

impl<'a> Str<'a> {
    fn as_str(&self) -> &str {
        match self {
            Str::Src(s) => s,
            Str::Shared(text, start, end) => &text[*start..*end],
        }
    }

    fn cow(&self) -> Cow<'a, str> {
        match self {
            Str::Src(s) => Cow::Borrowed(s),
            Str::Shared(..) => Cow::Owned(self.as_str().to_owned()),
        }
    }

    fn trimmed(&self) -> Cow<'a, str> {
        match self {
            Str::Src(s) => Cow::Borrowed(s.trim()),
            Str::Shared(..) => Cow::Owned(self.as_str().trim().to_owned()),
        }
    }
}

struct Input {
    text: Rc<str>,
    pos: usize,
    lex: LexState,
    span: Range<usize>,
    synthetic: bool,
    close_env: bool,
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
    name_end: usize,
    /// 0 for the source, otherwise the expansion's depth.
    depth: usize,
    /// The lexer state before the token, to read it again.
    lex: LexState,
}

#[derive(Default)]
struct TextBuf {
    active: bool,
    start: usize,
    end: usize,
    owned: Option<String>,
}

pub(crate) enum Ctx<'a> {
    Build(Box<Builder>),
    Read(&'a Index),
}

pub(crate) struct Parser<'a> {
    src: &'a str,
    pub(crate) ctx: Ctx<'a>,
    pos: usize,
    /// Where this run stops: the next checkpoint, or the end of the source.
    end: usize,
    pub(crate) st: State,
    inputs: Vec<Input>,
    pub(crate) out: VecDeque<Spanned<'a>>,
    text: TextBuf,
    steps: usize,
    next_cancel: usize,
    cancel: Option<&'a AtomicBool>,
    pub(crate) cancelled: bool,
    done: bool,
    /// The run ends at the end of the document rather than at a checkpoint.
    last: bool,
    /// Read mode: the index of the next checkpoint that splits text.
    next_checkpoint: usize,
}

fn is_rows_env(name: &str) -> bool {
    matches!(
        name.trim_end_matches('*'),
        "align" | "gather" | "flalign" | "alignat" | "eqnarray"
    )
}

impl<'a> Parser<'a> {
    pub(crate) fn new(
        src: &'a str,
        ctx: Ctx<'a>,
        start: usize,
        end: usize,
        state: State,
        cancel: Option<&'a AtomicBool>,
    ) -> Self {
        let last = end >= src.len();
        let next_checkpoint = match &ctx {
            Ctx::Read(index) => index.checkpoints.partition_point(|c| c.offset <= start),
            Ctx::Build(_) => 0,
        };
        Self {
            src,
            ctx,
            pos: start,
            end,
            st: state,
            inputs: Vec::new(),
            out: VecDeque::new(),
            text: TextBuf::default(),
            steps: 0,
            next_cancel: start + CANCEL_INTERVAL,
            cancel,
            cancelled: false,
            done: false,
            last,
            next_checkpoint,
        }
    }

    fn limits(&self) -> &crate::index::Limits {
        match &self.ctx {
            Ctx::Build(builder) => &builder.options.limits,
            Ctx::Read(index) => &index.options.limits,
        }
    }

    fn registry(&self) -> &Registry {
        match &self.ctx {
            Ctx::Build(builder) => &builder.registry,
            Ctx::Read(index) => &index.registry,
        }
    }

    fn building(&mut self) -> Option<&mut Builder> {
        match &mut self.ctx {
            Ctx::Build(builder) => Some(builder),
            Ctx::Read(_) => None,
        }
    }

    fn diagnose(&mut self, kind: D, name: Option<&str>, range: Range<usize>) {
        if let Ctx::Build(builder) = &mut self.ctx {
            builder.diagnostics.push(kind, name, range);
        }
    }

    // ---------------------------------------------------------------- driving

    /// Processes one token. Returns `false` when the run is over.
    pub(crate) fn step(&mut self) -> bool {
        if self.done {
            return false;
        }
        if self.inputs.is_empty() {
            // A run that ends at a checkpoint stops where the first pass
            // recorded it: the first visit of that offset at a safe point.
            let at_end = if self.last {
                self.pos >= self.end
            } else {
                self.pos > self.end || (self.pos == self.end && self.safe())
            };
            if at_end || self.st.phase == Phase::Ended {
                self.finish();
                return false;
            }
            if self.pos >= self.next_cancel {
                self.next_cancel = self.pos + CANCEL_INTERVAL;
                if self.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                    self.cancelled = true;
                    self.done = true;
                    return false;
                }
            }
            self.boundary();
            if self.done {
                return false;
            }
        }
        if self.st.verbatim.is_some() {
            self.verbatim_line();
            return true;
        }
        let token = self.next_token();
        self.dispatch(token);
        true
    }

    /// Top-level token boundary: split text at checkpoints, and record one in
    /// the first pass when enough source has passed at a safe point.
    fn boundary(&mut self) {
        let pos = self.pos;
        match &self.ctx {
            Ctx::Read(index) => {
                if index
                    .checkpoints
                    .get(self.next_checkpoint)
                    .is_some_and(|c| c.offset == pos)
                    && self.safe()
                {
                    self.next_checkpoint += 1;
                    self.flush();
                }
            }
            Ctx::Build(builder) => {
                if pos - builder.last_checkpoint >= builder.options.checkpoint_spacing
                    && self.safe()
                {
                    self.flush();
                    let state = self.st.clone();
                    let Ctx::Build(builder) = &mut self.ctx else {
                        unreachable!()
                    };
                    if !builder.record(pos, state) {
                        self.done = true;
                    }
                }
            }
        }
    }

    fn safe(&self) -> bool {
        if self.st.para.open {
            return false;
        }
        for frame in self.st.stack.iter().rev() {
            match &frame.kind {
                FrameKind::Inline(_) => return false,
                FrameKind::Env(_, Env::Table(table)) if table.row => return false,
                _ => {}
            }
        }
        true
    }

    fn finish(&mut self) {
        self.flush();
        if self.last && self.st.phase == Phase::Body {
            self.close_inline();
            while let Some(frame) = self.st.stack.last() {
                if let FrameKind::Env(name, _) = &frame.kind {
                    let name = name.clone();
                    if &*name != "document" {
                        self.diagnose(D::UnterminatedEnvironment, Some(&name), self.pos..self.pos);
                    }
                }
                let at = self.pos;
                self.close_top(at);
            }
            self.flush();
        }
        self.done = true;
    }

    // ----------------------------------------------------------------- tokens

    fn next_token(&mut self) -> Tok {
        loop {
            if let Some(input) = self.inputs.last_mut() {
                if input.pos >= input.text.len() {
                    let input = self.inputs.pop().unwrap();
                    if input.close_env {
                        self.close_user_env(input.span.end);
                    }
                    continue;
                }
                let lex = input.lex;
                let token = lexer::next(&input.text, input.pos, input.text.len(), &mut input.lex);
                input.pos = token.end;
                return Tok {
                    kind: token.kind,
                    start: token.start,
                    end: token.end,
                    name_end: token.name_end,
                    depth: self.inputs.len(),
                    lex,
                };
            }
            let lex = self.st.lex;
            let token = lexer::next(self.src, self.pos, self.src.len(), &mut self.st.lex);
            self.pos = token.end;
            return Tok {
                kind: token.kind,
                start: token.start,
                end: token.end,
                name_end: token.name_end,
                depth: 0,
                lex,
            };
        }
    }

    /// Reads `token` again on the next step.
    fn unread(&mut self, token: &Tok) {
        if token.depth == 0 {
            self.pos = token.start;
            self.st.lex = token.lex;
        } else if let Some(input) = self.inputs.get_mut(token.depth - 1) {
            input.pos = token.start;
            input.lex = token.lex;
        }
    }

    fn slice(&self, token: &Tok, start: usize, end: usize) -> Str<'a> {
        if token.depth == 0 {
            Str::Src(&self.src[start..end])
        } else {
            Str::Shared(Rc::clone(&self.inputs[token.depth - 1].text), start, end)
        }
    }

    fn span(&self, token: &Tok) -> Range<usize> {
        if token.depth == 0 {
            token.start..token.end
        } else {
            self.inputs[token.depth - 1].span.clone()
        }
    }

    fn pop_exhausted(&mut self) {
        while let Some(input) = self.inputs.last() {
            if input.pos < input.text.len() {
                break;
            }
            let input = self.inputs.pop().unwrap();
            if input.close_env {
                self.close_user_env(input.span.end);
            }
        }
    }

    /// The current text and position, for raw scans.
    fn cursor(&mut self) -> (Option<Rc<str>>, usize) {
        self.pop_exhausted();
        match self.inputs.last() {
            Some(input) => (Some(Rc::clone(&input.text)), input.pos),
            None => (None, self.pos),
        }
    }

    fn commit(&mut self, pos: usize) {
        match self.inputs.last_mut() {
            Some(input) => {
                input.pos = pos;
                input.lex.line_start = false;
            }
            None => {
                self.pos = pos;
                self.st.lex.line_start = false;
            }
        }
    }

    fn make_str(&self, rc: &Option<Rc<str>>, start: usize, end: usize) -> Str<'a> {
        match rc {
            Some(text) => Str::Shared(Rc::clone(text), start, end),
            None => Str::Src(&self.src[start..end]),
        }
    }

    fn make_span(&self, rc: &Option<Rc<str>>, start: usize, end: usize) -> Range<usize> {
        match (rc, self.inputs.last()) {
            (Some(_), Some(input)) => input.span.clone(),
            _ => start..end,
        }
    }

    /// The next byte after spaces, comments, and at most one line end.
    fn peek(&mut self) -> Option<u8> {
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        skip_blank(text, pos).map(|at| text.as_bytes()[at])
    }

    fn peek_immediate(&mut self) -> Option<u8> {
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        text.as_bytes().get(pos).copied()
    }

    /// Consumes `*` if it follows.
    fn star(&mut self) -> bool {
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        match skip_blank(text, pos) {
            Some(at) if text.as_bytes()[at] == b'*' => {
                self.commit(at + 1);
                true
            }
            _ => false,
        }
    }

    /// Reads a delimited argument raw: `{...}` or `[...]`.
    fn raw(&mut self, open: u8) -> Option<(Str<'a>, Range<usize>)> {
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        let at = skip_blank(text, pos)?;
        if text.as_bytes()[at] != open {
            return None;
        }
        let close = if open == b'{' { b'}' } else { b']' };
        let end = math::matching(text, at + 1, text.len(), open, close)?;
        self.commit(end + 1);
        Some((
            self.make_str(&rc, at + 1, end),
            self.make_span(&rc, at, end + 1),
        ))
    }

    fn group(&mut self) -> Option<(Str<'a>, Range<usize>)> {
        self.raw(b'{')
    }

    fn optional(&mut self) -> Option<(Str<'a>, Range<usize>)> {
        self.raw(b'[')
    }

    /// Reads a URL argument: `%` and `#` are literal, as hyperref reads them.
    fn url(&mut self) -> Option<(Str<'a>, Range<usize>)> {
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        let at = skip_blank(text, pos)?;
        let bytes = text.as_bytes();
        if bytes[at] != b'{' {
            return None;
        }
        let mut depth = 0;
        let mut i = at + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 1,
                b'{' => depth += 1,
                b'}' if depth == 0 => {
                    self.commit(i + 1);
                    return Some((
                        self.make_str(&rc, at + 1, i),
                        self.make_span(&rc, at, i + 1),
                    ));
                }
                b'}' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// A macro argument: a group, or a single token.
    fn argument(&mut self) -> Option<(Str<'a>, Range<usize>)> {
        if let Some(group) = self.group() {
            return Some(group);
        }
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        let at = skip_blank(text, pos)?;
        let bytes = text.as_bytes();
        let end = if bytes[at] == b'\\' {
            let mut lex = LexState::default();
            let token = lexer::next(text, at, text.len(), &mut lex);
            token.name_end
        } else if matches!(bytes[at], b'}' | b'%') {
            return None;
        } else {
            at + text[at..].chars().next().map_or(1, char::len_utf8)
        };
        self.commit(end);
        Some((self.make_str(&rc, at, end), self.make_span(&rc, at, end)))
    }

    /// Consumes arguments by pattern, discarding them.
    fn skip_args(&mut self, pattern: &str) {
        for kind in pattern.bytes() {
            match kind {
                b's' => {
                    self.star();
                }
                b'o' => {
                    self.optional();
                }
                _ => {
                    self.argument();
                }
            }
        }
    }

    // ------------------------------------------------------------------- text

    fn push_event(&mut self, event: Event<'a>, range: Range<usize>) {
        if self.st.phase != Phase::Body {
            return;
        }
        match &mut self.ctx {
            Ctx::Read(_) => self.out.push_back(Spanned { event, range }),
            Ctx::Build(builder) => builder.observe(&event, &range),
        }
    }

    fn emit(&mut self, event: Event<'a>, range: Range<usize>) {
        self.flush();
        self.push_event(event, range);
    }

    fn flush(&mut self) {
        if !self.text.active {
            return;
        }
        self.text.active = false;
        let range = self.text.start..self.text.end;
        let text = match self.text.owned.take() {
            Some(owned) => Cow::Owned(owned),
            None => Cow::Borrowed(&self.src[range.clone()]),
        };
        self.push_event(Event::Text(text), range);
    }

    fn wants_text(&self) -> bool {
        if self.st.phase != Phase::Body {
            return false;
        }
        match &self.ctx {
            Ctx::Read(_) => true,
            Ctx::Build(builder) => builder.capturing(),
        }
    }

    fn append(&mut self, text: Cow<'a, str>, range: Range<usize>) {
        let buf = &mut self.text;
        let contiguous = matches!(text, Cow::Borrowed(_))
            && buf.owned.is_none()
            && buf.end == range.start
            && self.src.get(range.clone()) == Some(text.as_ref());
        if !buf.active {
            buf.active = true;
            buf.start = range.start;
            buf.end = range.end;
            buf.owned = match text {
                Cow::Borrowed(s) if self.src.get(range.clone()) == Some(s) => None,
                other => Some(other.into_owned()),
            };
            return;
        }
        if contiguous {
            buf.end = range.end;
            return;
        }
        let owned = buf
            .owned
            .get_or_insert_with(|| self.src[buf.start..buf.end].to_owned());
        owned.push_str(&text);
        buf.start = buf.start.min(range.start);
        buf.end = buf.end.max(range.end);
    }

    /// Pushes inline text, opening a paragraph or cell when needed.
    fn push_text(&mut self, text: Cow<'a, str>, range: Range<usize>) {
        if self.st.phase != Phase::Body || text.is_empty() {
            return;
        }
        self.open_inline(range.start);
        if let Some(space) = self.st.para.space.take()
            && self.wants_text()
        {
            let exact = self.src.get(space.clone()) == Some(" ");
            let space_text = if exact {
                Cow::Borrowed(&self.src[space.clone()])
            } else {
                Cow::Owned(" ".into())
            };
            self.append(space_text, space);
        }
        self.st.para.content = true;
        if self.wants_text() {
            self.append(text, range);
        }
    }

    /// Emits an inline event such as math or a reference.
    fn inline(&mut self, event: Event<'a>, range: Range<usize>) {
        if self.st.phase != Phase::Body {
            return;
        }
        self.open_inline(range.start);
        if let Some(space) = self.st.para.space.take()
            && self.wants_text()
        {
            self.append(Cow::Owned(" ".into()), space);
        }
        self.st.para.content = true;
        self.emit(event, range);
    }

    fn space(&mut self, range: Range<usize>) {
        if self.st.para.open && self.st.para.content {
            self.st.para.space = Some(range);
        }
    }

    // ------------------------------------------------------------ paragraphs

    fn table_frame(&mut self) -> Option<&mut Table> {
        for frame in self.st.stack.iter_mut().rev() {
            match &mut frame.kind {
                FrameKind::Inline(_) => return None,
                FrameKind::Env(_, Env::Table(table)) => return Some(table),
                FrameKind::Env(..) => return None,
                FrameKind::Brace => {}
            }
        }
        None
    }

    fn open_inline(&mut self, at: usize) {
        if self.st.para.open || self.st.phase != Phase::Body {
            return;
        }
        if let Some(table) = self.table_frame() {
            let open_row = !table.row;
            table.row = true;
            let (span, align) = table.next.take().unwrap_or_else(|| {
                (
                    1,
                    table
                        .columns
                        .get(table.col)
                        .map_or(Align::Left, |c| c.align),
                )
            });
            table.span = span;
            if open_row {
                self.emit(Event::Start(Tag::Row), at..at);
            }
            self.emit(Event::Start(Tag::Cell { span, align }), at..at);
            self.st.para = Para {
                open: true,
                cell: true,
                indent: self.st.para.indent,
                ..Para::default()
            };
            return;
        }
        let indent = self.st.para.indent && !self.st.para.suppress;
        self.emit(Event::Start(Tag::Paragraph { indent }), at..at);
        self.st.para.open = true;
        self.st.para.content = false;
        self.st.para.suppress = false;
        self.st.para.indent = true;
        let styles: Vec<Style> = self
            .st
            .stack
            .iter()
            .flat_map(|frame| frame.styles.iter().copied())
            .collect();
        for style in styles {
            self.emit(Event::Start(Tag::Style(style)), at..at);
            self.st.para.styles.push(style);
        }
    }

    /// Ends the current paragraph or cell.
    fn close_inline(&mut self) {
        let para = &self.st.para;
        if !para.open || para.block {
            self.st.para.space = None;
            return;
        }
        self.flush();
        let at = self.pos;
        while let Some(style) = self.st.para.styles.pop() {
            self.emit(Event::End(TagKind::Style), at..at);
            let _ = style;
        }
        if self.st.para.cell {
            self.emit(Event::End(TagKind::Cell), at..at);
            if let Some(table) = self.table_frame() {
                table.col += usize::from(table.span.max(1));
            }
        } else {
            self.emit(Event::End(TagKind::Paragraph), at..at);
        }
        let indent = self.st.para.indent;
        let suppress = self.st.para.suppress;
        self.st.para = Para {
            indent,
            suppress,
            ..Para::default()
        };
    }

    fn line_break(&mut self, range: Range<usize>) {
        if self.st.para.open {
            self.emit(Event::LineBreak, range);
            // Spaces after a line break are dropped, as in TeX.
            self.st.para.space = None;
            self.st.para.content = false;
        }
    }

    // ----------------------------------------------------------------- frames

    fn push_frame(&mut self, kind: FrameKind) -> bool {
        if self.st.stack.len() >= self.limits().max_nesting {
            self.diagnose(D::LimitExceeded, Some("nesting"), self.pos..self.pos);
            return false;
        }
        if let Ctx::Build(builder) = &mut self.ctx {
            builder.store.enter();
        }
        self.st.stack.push(Frame {
            kind,
            styles: Vec::new(),
            label: Arc::clone(&self.st.label),
            para: None,
        });
        true
    }

    fn pop_frame(&mut self, at: usize) -> Option<Frame> {
        let frame = self.st.stack.pop()?;
        self.st.label = Arc::clone(&frame.label);
        if let Ctx::Build(builder) = &mut self.ctx {
            builder.store.leave(at);
        }
        if self.st.para.open && !frame.styles.is_empty() {
            self.flush();
            for _ in &frame.styles {
                if self.st.para.styles.pop().is_some() {
                    self.emit(Event::End(TagKind::Style), at..at);
                }
            }
        }
        Some(frame)
    }

    /// Opens a block that holds inline content, such as a heading.
    fn open_block_inline(&mut self, tag: Tag<'a>, bracket: bool, at: Range<usize>) {
        self.close_inline();
        let kind = tag.kind();
        if !self.push_frame(FrameKind::Inline(Inline {
            tag: kind,
            bracket,
            unsupported: false,
        })) {
            return;
        }
        self.emit(Event::Start(tag), at);
        let saved = std::mem::replace(
            &mut self.st.para,
            Para {
                open: true,
                block: true,
                ..Para::default()
            },
        );
        self.st.stack.last_mut().unwrap().para = Some(Box::new(saved));
    }

    /// Opens an inline container inside a paragraph, such as a footnote.
    fn open_inline_frame(
        &mut self,
        tag: Tag<'a>,
        bracket: bool,
        unsupported: bool,
        at: Range<usize>,
    ) {
        self.inline_start(at.start);
        let kind = tag.kind();
        if !self.push_frame(FrameKind::Inline(Inline {
            tag: kind,
            bracket,
            unsupported,
        })) {
            return;
        }
        self.emit(Event::Start(tag), at);
    }

    fn inline_start(&mut self, at: usize) {
        self.open_inline(at);
        if let Some(space) = self.st.para.space.take()
            && self.wants_text()
        {
            self.append(Cow::Owned(" ".into()), space);
        }
        self.st.para.content = true;
    }

    /// Closes the innermost frame, emitting its end events.
    fn close_top(&mut self, at: usize) {
        let Some(frame) = self.st.stack.last() else {
            return;
        };
        match &frame.kind {
            FrameKind::Brace => {
                self.pop_frame(at);
            }
            FrameKind::Inline(inline) => {
                let inline = *inline;
                self.flush();
                if self.st.para.open {
                    while self.st.para.styles.len()
                        > self.st.stack.last().map_or(0, |_| self.outer_style_count())
                    {
                        self.st.para.styles.pop();
                        self.emit(Event::End(TagKind::Style), at..at);
                    }
                }
                let frame = self.pop_frame(at).unwrap();
                if inline.unsupported {
                    self.emit(Event::End(TagKind::Unsupported), at..at);
                } else {
                    self.emit(Event::End(inline.tag), at..at);
                }
                if let Some(para) = frame.para {
                    self.st.para = *para;
                    if inline.tag == TagKind::Heading {
                        self.st.para.suppress = true;
                    }
                }
            }
            FrameKind::Env(..) => {
                self.close_inline();
                let frame = self.pop_frame(at).unwrap();
                let FrameKind::Env(_, env) = frame.kind else {
                    unreachable!()
                };
                self.end_env(env, at);
            }
        }
    }

    /// Styles opened by frames below the innermost one.
    fn outer_style_count(&self) -> usize {
        let len = self.st.stack.len();
        if len == 0 {
            return 0;
        }
        let inner: usize = self.st.stack[len - 1].styles.len();
        self.st.para.styles.len().saturating_sub(inner)
    }

    fn end_env(&mut self, env: Env, at: usize) {
        let range = at..at;
        match env {
            Env::List { item, .. } => {
                if item {
                    self.emit(Event::End(TagKind::Item), range.clone());
                }
                self.emit(Event::End(TagKind::List), range);
                self.st.para.indent = false;
            }
            Env::Block(tag) => {
                self.emit(Event::End(tag), range);
                self.st.para.indent = false;
            }
            Env::Float(_) => self.emit(Event::End(TagKind::Figure), range),
            Env::Table(table) => {
                if table.row {
                    self.emit(Event::End(TagKind::Row), range.clone());
                }
                self.emit(Event::End(TagKind::Table), range);
            }
            Env::Bibliography { item, .. } => {
                if item {
                    self.emit(Event::End(TagKind::BibItem), range.clone());
                }
                self.emit(Event::End(TagKind::Bibliography), range);
            }
            Env::Unsupported => self.emit(Event::End(TagKind::Unsupported), range),
            Env::Document | Env::Transparent | Env::Subequations { .. } | Env::User(_) => {}
        }
    }

    fn close_user_env(&mut self, at: usize) {
        if let Some(index) = self
            .st
            .stack
            .iter()
            .rposition(|f| matches!(f.kind, FrameKind::Env(_, Env::User(_))))
        {
            while self.st.stack.len() > index + 1 {
                self.close_top(at);
            }
            self.close_top(at);
        }
    }

    // --------------------------------------------------------------- dispatch

    fn dispatch(&mut self, token: Tok) {
        let range = self.span(&token);
        match token.kind {
            Kind::Eof => {}
            Kind::Text => {
                if self.st.phase != Phase::Body {
                    return;
                }
                let raw = self.slice(&token, token.start, token.end);
                let text = match &raw {
                    Str::Src(s) => text::ligatures(s),
                    Str::Shared(..) => Cow::Owned(text::ligatures(raw.as_str()).into_owned()),
                };
                self.push_text(text, range);
            }
            Kind::Space => self.space(range),
            Kind::Par => self.par(range),
            Kind::Word => {
                let name = self.slice(&token, token.start + 1, token.name_end);
                self.word(&token, name, range);
            }
            Kind::Symbol => {
                let name = self.slice(&token, token.start + 1, token.end);
                self.symbol(&token, name, range);
            }
            Kind::Open => {
                self.push_frame(FrameKind::Brace);
            }
            Kind::Close => self.close_group(range),
            Kind::LBracket => self.push_text(Cow::Borrowed("["), range),
            Kind::RBracket => {
                if matches!(
                    self.st.stack.last().map(|f| &f.kind),
                    Some(FrameKind::Inline(Inline { bracket: true, .. }))
                ) {
                    self.close_inline_group(range);
                } else {
                    self.push_text(Cow::Borrowed("]"), range);
                }
            }
            Kind::Math => self.dollar(&token, range),
            Kind::Align => {
                if self.table_frame().is_some() {
                    self.cell_end(range);
                } else {
                    self.push_text(Cow::Borrowed("&"), range);
                }
            }
            Kind::Tilde => self.push_text(Cow::Borrowed("\u{a0}"), range),
            Kind::Param => self.push_text(Cow::Borrowed("#"), range),
            Kind::Super => self.push_text(Cow::Borrowed("^"), range),
            Kind::Sub => self.push_text(Cow::Borrowed("_"), range),
        }
    }

    fn par(&mut self, range: Range<usize>) {
        let in_inline = self
            .st
            .stack
            .iter()
            .any(|f| matches!(f.kind, FrameKind::Inline(_)));
        if in_inline || self.st.para.cell || self.st.para.block {
            self.line_break(range);
        } else {
            self.close_inline();
            if !self.st.para.suppress {
                self.st.para.indent = true;
            }
        }
    }

    fn close_group(&mut self, range: Range<usize>) {
        match self.st.stack.last().map(|f| &f.kind) {
            Some(FrameKind::Brace) => {
                self.pop_frame(range.end);
            }
            Some(FrameKind::Inline(Inline { bracket: false, .. })) => {
                self.close_inline_group(range);
            }
            _ => self.diagnose(D::UnbalancedGroup, Some("}"), range),
        }
    }

    fn close_inline_group(&mut self, range: Range<usize>) {
        let Some(FrameKind::Inline(inline)) = self.st.stack.last().map(|f| f.kind.clone()) else {
            return;
        };
        if inline.unsupported {
            self.emit(
                Event::Raw(Cow::Owned(if inline.bracket { "]" } else { "}" }.into())),
                range.clone(),
            );
            // Adjacent groups are further arguments of the unknown command.
            if let Some(next @ (b'{' | b'[')) = self.peek_immediate() {
                let (rc, pos) = self.cursor();
                self.commit(pos + 1);
                let open = self.make_span(&rc, pos, pos + 1);
                self.emit(Event::Raw(Cow::Owned(char::from(next).to_string())), open);
                if let Some(FrameKind::Inline(top)) = self.st.stack.last_mut().map(|f| &mut f.kind)
                {
                    top.bracket = next == b'[';
                }
                return;
            }
        }
        self.close_top(range.end);
    }

    // --------------------------------------------------------------- commands

    fn word(&mut self, token: &Tok, name: Str<'a>, range: Range<usize>) {
        let synthetic = token.depth > 0 && self.inputs[token.depth - 1].synthetic;
        if synthetic && name.as_str() == SYNTHETIC_TITLE {
            self.title_part(range);
            return;
        }
        let spec = commands::command(name.as_str());
        if let Some(spec) = spec {
            self.command(token, &name, spec, range);
            return;
        }
        let key = format!("\\{}", name.as_str());
        if self.expand(&key, range.clone()) {
            return;
        }
        if let Some(mark) = text::accent_mark(name.as_str()) {
            self.accent(mark, range);
            return;
        }
        self.unknown(&name, range);
    }

    fn symbol(&mut self, token: &Tok, name: Str<'a>, range: Range<usize>) {
        match name.as_str() {
            "\\" => {
                self.star();
                self.optional();
                if self.table_frame().is_some() && !self.in_inline_frame() {
                    self.row_end(range);
                } else {
                    self.line_break(range);
                }
            }
            "(" => self.delimited_math(Delim::Paren, false, range),
            "[" => self.delimited_math(Delim::Bracket, true, range),
            ")" | "]" => self.push_text(Cow::Owned(name.as_str().to_owned()), range),
            other => {
                if let Some(mark) = text::accent_mark(other) {
                    self.accent(mark, range);
                } else if let Some(text) = commands::symbol(other) {
                    if other == "-" || other == "/" || other == "@" || other == "!" {
                        return;
                    }
                    self.push_text(Cow::Borrowed(text), range);
                } else {
                    let _ = token;
                    self.push_text(name.cow(), range);
                }
            }
        }
    }

    fn in_inline_frame(&self) -> bool {
        for frame in self.st.stack.iter().rev() {
            match frame.kind {
                FrameKind::Inline(_) => return true,
                FrameKind::Env(..) => return false,
                FrameKind::Brace => {}
            }
        }
        false
    }

    fn accent(&mut self, mark: char, range: Range<usize>) {
        let base = self
            .argument()
            .map(|(arg, _)| arg.as_str().trim().to_owned())
            .unwrap_or_default();
        let base = match base.as_str() {
            "\\i" => "ı".to_owned(),
            "\\j" => "ȷ".to_owned(),
            _ => base,
        };
        let composed = text::accent(&base, mark);
        self.push_text(Cow::Owned(composed), range);
    }

    fn command(&mut self, token: &Tok, name: &Str<'a>, spec: commands::Spec, range: Range<usize>) {
        let body = self.st.phase == Phase::Body;
        match spec.cmd {
            Cmd::Style(style) => {
                if !body {
                    self.skip_args(spec.args);
                    return;
                }
                if self.peek() == Some(b'{') {
                    self.raw_open_brace();
                    self.open_inline_frame(Tag::Style(style), false, false, range);
                } else if let Some((arg, arg_range)) = self.argument() {
                    self.inline_start(range.start);
                    self.emit(Event::Start(Tag::Style(style)), range.clone());
                    self.push_text(arg.cow(), arg_range);
                    self.emit(Event::End(TagKind::Style), range);
                }
            }
            Cmd::Declare(style) => {
                if let Some(frame) = self.st.stack.last_mut() {
                    frame.styles.push(style);
                    if self.st.para.open && body {
                        self.st.para.styles.push(style);
                        self.emit(Event::Start(Tag::Style(style)), range);
                    }
                }
            }
            Cmd::Transparent => {
                let (skip, last) = spec.args.split_at(spec.args.len().saturating_sub(1));
                self.skip_args(skip);
                if last == "m" && self.peek() == Some(b'{') {
                    self.raw_open_brace();
                    self.push_frame(FrameKind::Brace);
                }
            }
            Cmd::Invisible => self.skip_args(spec.args),
            Cmd::Symbol(symbol) => self.push_text(Cow::Borrowed(symbol), range),
            Cmd::Accent => {
                if let Some(mark) = text::accent_mark(name.as_str()) {
                    self.accent(mark, range);
                }
            }
            Cmd::Section(level) => self.section(level, range),
            Cmd::Hint(hint) => {
                self.skip_args(spec.args);
                if hint == crate::Hint::PageBreak {
                    // `\newpage` ends the paragraph first.
                    self.par(range.clone());
                }
                if body {
                    self.emit(Event::Hint(hint), range);
                }
            }
            Cmd::LineBreak => {
                self.skip_args(spec.args);
                self.line_break(range);
            }
            Cmd::Par => self.par(range),
            Cmd::NoIndent => self.st.para.indent = false,
            Cmd::Special(special) => self.special(token, special, name, range),
        }
    }

    /// Consumes a `{` that `peek` found.
    fn raw_open_brace(&mut self) {
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        if let Some(at) = skip_blank(text, pos) {
            self.commit(at + 1);
        }
    }

    fn unknown(&mut self, name: &Str<'a>, range: Range<usize>) {
        if self.st.phase != Phase::Body {
            return;
        }
        self.diagnose(D::UnknownCommand, Some(name.as_str()), range.clone());
        self.inline_start(range.start);
        self.emit(
            Event::Start(Tag::Unsupported {
                name: name.cow(),
                block: false,
            }),
            range.clone(),
        );
        self.emit(
            Event::Raw(Cow::Owned(format!("\\{}", name.as_str()))),
            range.clone(),
        );
        if let Some(next @ (b'{' | b'[')) = self.peek_immediate() {
            let (rc, pos) = self.cursor();
            self.commit(pos + 1);
            let open = self.make_span(&rc, pos, pos + 1);
            if self.push_frame(FrameKind::Inline(Inline {
                tag: TagKind::Unsupported,
                bracket: next == b'[',
                unsupported: true,
            })) {
                self.emit(Event::Raw(Cow::Owned(char::from(next).to_string())), open);
                return;
            }
        }
        self.emit(Event::End(TagKind::Unsupported), range);
    }

    // ------------------------------------------------------------- expansion

    fn lookup_macro(&self, name: &str, at: usize) -> Option<Binding> {
        let found = match &self.ctx {
            Ctx::Build(builder) => builder.store.current(name),
            Ctx::Read(index) => index.store.lookup(name, index.store.version_at(at)),
        };
        match found {
            Lookup::Bound(binding) => Some(binding.clone()),
            Lookup::Unknown => Some(Binding {
                name: name.to_owned(),
                kind: BindingKind::Let,
                params: 0,
                default: None,
                parameter_text: None,
                body: String::new(),
                end: Some("unknown".into()),
            }),
            Lookup::Undefined => None,
        }
    }

    /// Expands a user macro in text. Returns `false` when `name` is not one.
    fn expand(&mut self, name: &str, range: Range<usize>) -> bool {
        let Some(binding) = self.lookup_macro(name, range.start) else {
            return false;
        };
        if binding.kind == BindingKind::Environment {
            return false;
        }
        if binding.end.as_deref() == Some("unknown") {
            // A definition that could not be applied: show the source.
            self.unknown(&owned_str(name.trim_start_matches('\\')), range);
            return true;
        }
        if binding.kind == BindingKind::Let {
            // An alias of a command the document does not define, often a
            // math command; shown as its target.
            let target = binding.body.trim_start_matches('\\').to_owned();
            match commands::command(&target) {
                Some(spec) if !matches!(spec.cmd, Cmd::Special(_)) => {
                    let token = Tok {
                        kind: Kind::Word,
                        start: 0,
                        end: 0,
                        name_end: 0,
                        depth: 0,
                        lex: LexState::default(),
                    };
                    self.command(
                        &token,
                        &Str::Shared(Rc::from(target.as_str()), 0, target.len()),
                        spec,
                        range,
                    );
                }
                _ => self.unknown(
                    &Str::Shared(Rc::from(target.as_str()), 0, target.len()),
                    range,
                ),
            }
            return true;
        }
        let mut args: Vec<String> = Vec::new();
        let mut params = binding.params;
        if let Some(default) = &binding.default {
            args.push(
                self.optional()
                    .map_or_else(|| default.clone(), |(arg, _)| arg.as_str().to_owned()),
            );
            params = params.saturating_sub(1);
        }
        if binding.kind == BindingKind::Def
            && binding
                .parameter_text
                .as_deref()
                .is_some_and(|t| !t.is_empty() && !binding.simple_body().is_some())
        {
            // Delimited parameters are not expanded in text.
            self.unknown(&owned_str(name.trim_start_matches('\\')), range);
            return true;
        }
        for _ in 0..params {
            args.push(
                self.argument()
                    .map(|(a, _)| a.as_str().to_owned())
                    .unwrap_or_default(),
            );
        }
        let limits = self.limits();
        let (max_depth, max_steps, max_bytes) = (
            limits.max_expansion_depth,
            limits.max_expansion_steps,
            limits.max_expansion_bytes,
        );
        self.steps += 1;
        let expansion = substitute(&binding.body, &args);
        if self.inputs.len() >= max_depth || self.steps > max_steps || expansion.len() > max_bytes {
            self.diagnose(D::LimitExceeded, Some("expansion"), range.clone());
            self.unknown(&owned_str(name.trim_start_matches('\\')), range);
            return true;
        }
        let span = self.inputs.last().map_or(range, |input| input.span.clone());
        self.inputs.push(Input {
            text: Rc::from(expansion),
            pos: 0,
            lex: LexState {
                line_start: false,
                at_letter: self.st.lex.at_letter,
            },
            span,
            synthetic: false,
            close_env: false,
        });
        true
    }

    // -------------------------------------------------------------- structure

    fn section(&mut self, level: u8, range: Range<usize>) {
        let star = self.star();
        self.optional();
        if self.st.phase != Phase::Body {
            self.group();
            return;
        }
        let registry = self.registry();
        let latex_level = i32::from(level) - 1;
        let numbered =
            !star && latex_level <= registry.secnumdepth && (level != 1 || registry.chapters);
        let number = if numbered {
            let id = usize::from(level);
            self.step_counter(id);
            let number = self.format_counter(id);
            self.st.label = Arc::from(number.as_str());
            Some(number)
        } else {
            None
        };
        if self.peek() == Some(b'{') {
            self.raw_open_brace();
            self.open_block_inline(Tag::Heading { level, number }, false, range);
        }
    }

    fn step_counter(&mut self, id: usize) {
        let descendants = self.registry().descendants(id);
        let counters = &mut self.st.counters;
        if counters.len() <= id {
            counters.resize(id + 1, 0);
        }
        counters[id] += 1;
        for child in descendants {
            if child < counters.len() {
                counters[child] = 0;
            }
            if child < 64 {
                self.st.uncertain &= !(1 << child);
            }
        }
    }

    fn set_counter(&mut self, id: usize, value: u32) {
        if self.st.counters.len() <= id {
            self.st.counters.resize(id + 1, 0);
        }
        self.st.counters[id] = value;
        if id < 64 {
            self.st.uncertain &= !(1 << id);
        }
    }

    fn format_counter(&self, id: usize) -> String {
        self.registry()
            .format(id, &self.st.counters, self.st.uncertain, self.st.appendix)
    }

    fn special(&mut self, token: &Tok, special: Special, name: &Str<'a>, range: Range<usize>) {
        let body = self.st.phase == Phase::Body;
        match special {
            Special::Begin => self.begin(range),
            Special::End => self.end(range),
            Special::Item | Special::BibItem => {
                self.item(token, special == Special::BibItem, range)
            }
            Special::Label => {
                if let Some((label, _)) = self.group() {
                    let label = label.as_str().trim().to_owned();
                    let value = Arc::clone(&self.st.label);
                    self.record_label(label, &value, range);
                }
            }
            Special::Ref(kind) => {
                self.star();
                if let Some((label, _)) = self.group() {
                    if !body {
                        return;
                    }
                    let label_text = label.trimmed();
                    let resolved = self.resolve_label(&label_text);
                    self.inline(
                        Event::Reference(Reference {
                            kind,
                            label: label_text,
                            resolved,
                        }),
                        range,
                    );
                }
            }
            Special::Cite => {
                self.star();
                let first = self.optional();
                let second = self.optional();
                let note = second.or(first).map(|(n, _)| note_text(n.as_str()));
                if let Some((keys, _)) = self.group() {
                    if !body {
                        return;
                    }
                    let keys: Vec<Cow<'a, str>> = keys
                        .as_str()
                        .split(',')
                        .map(|k| Cow::<'a, str>::Owned(k.trim().to_owned()))
                        .filter(|k| !k.is_empty())
                        .collect();
                    let resolved = keys.iter().map(|k| self.resolve_citation(k)).collect();
                    self.inline(
                        Event::Citation(Citation {
                            keys,
                            resolved,
                            note: note.filter(|n| !n.is_empty()),
                        }),
                        range,
                    );
                }
            }
            Special::Footnote => {
                let thanks = name.as_str() == "thanks";
                let given = self.optional();
                let number = if thanks {
                    None
                } else if let Some((given, _)) = given {
                    Some(given.as_str().trim().to_owned())
                } else {
                    self.step_counter(numbering::FOOTNOTE);
                    Some(self.format_counter(numbering::FOOTNOTE))
                };
                if self.peek() == Some(b'{') && body {
                    self.raw_open_brace();
                    self.open_inline_frame(Tag::Footnote { number }, false, false, range);
                } else {
                    self.group();
                }
            }
            Special::Href => {
                let url = self.url();
                if !body {
                    self.group();
                    return;
                }
                let url = url.map_or(Cow::Borrowed(""), |(u, _)| u.trimmed());
                if self.peek() == Some(b'{') {
                    self.raw_open_brace();
                    self.open_inline_frame(Tag::Link { url }, false, false, range);
                }
            }
            Special::Url => {
                if let Some((url, url_range)) = self.url()
                    && body
                {
                    let url = url.trimmed();
                    self.inline_start(range.start);
                    self.emit(Event::Start(Tag::Link { url: url.clone() }), range.clone());
                    self.push_text(url, url_range);
                    self.emit(Event::End(TagKind::Link), range);
                }
            }
            Special::IncludeGraphics => {
                self.star();
                let options = self.optional().map(|(o, _)| o.trimmed());
                self.optional();
                if let Some((path, _)) = self.group()
                    && body
                {
                    let path = path.trimmed();
                    self.inline(Event::Image(Image { path, options }), range);
                }
            }
            Special::GraphicsPath => {
                if let Some((dirs, _)) = self.group()
                    && let Some(builder) = self.building()
                {
                    let dirs = dirs.as_str();
                    builder.graphics_path = if dirs.contains('{') {
                        dirs.split(['{', '}'])
                            .map(str::trim)
                            .filter(|d| !d.is_empty())
                            .map(str::to_owned)
                            .collect()
                    } else {
                        vec![dirs.trim().to_owned()]
                    };
                }
            }
            Special::Caption => self.caption(name, range),
            Special::MakeTitle => self.make_title(range),
            Special::Title | Special::Author | Special::Date => {
                self.optional();
                if let Some((text, _)) = self.group()
                    && let Some(builder) = self.building()
                {
                    let slot = match special {
                        Special::Title => 0,
                        Special::Author => 1,
                        _ => 2,
                    };
                    builder.title[slot] = Some(text.as_str().to_owned());
                }
            }
            Special::And => self.line_break(range),
            // Scoped like braces, which is all a reader needs from them.
            Special::Group(true) => {
                self.push_frame(FrameKind::Brace);
            }
            Special::Group(false) => self.close_group(range),
            Special::Verb => self.verb(range),
            Special::LstInline => {
                self.optional();
                if self.peek() == Some(b'{') {
                    if let Some((code, code_range)) = self.group() {
                        self.inline_code(code, code_range, range);
                    }
                } else {
                    self.verb(range);
                }
            }
            Special::MintInline => {
                self.optional();
                self.group();
                if self.peek() == Some(b'{') {
                    if let Some((code, code_range)) = self.group() {
                        self.inline_code(code, code_range, range);
                    }
                } else {
                    self.verb(range);
                }
            }
            Special::Include(kind) => {
                let mut dir = None;
                if kind == IncludeKind::Listing {
                    self.star();
                    self.optional();
                    if name.as_str() == "inputminted" {
                        self.group();
                    }
                } else if name.as_str().contains("import") || name.as_str().ends_with("from") {
                    self.star();
                    dir = self.group().map(|(dir, _)| dir.trimmed());
                }
                if let Some((path, span)) = self.group()
                    && body
                {
                    let path = match dir {
                        Some(dir) => Cow::Owned(format!("{dir}{}", path.trimmed())),
                        None => path.trimmed(),
                    };
                    // The range covers the arguments, so the command shows as written.
                    let range = range.start..span.end.max(range.end);
                    self.emit(Event::Include(Include { kind, path }), range);
                }
            }
            Special::NewTheorem => self.new_theorem(),
            Special::TheoremStyle => {
                if let Some((style, _)) = self.group()
                    && let Some(builder) = self.building()
                {
                    builder.registry.style = match style.as_str().trim() {
                        "definition" => TheoremStyle::Definition,
                        "remark" => TheoremStyle::Remark,
                        _ => TheoremStyle::Plain,
                    };
                }
            }
            Special::SetCounter | Special::AddToCounter => {
                let counter = self.group().map(|(c, _)| c.as_str().trim().to_owned());
                let value = self.group().map(|(v, _)| v.as_str().trim().to_owned());
                if let (Some(counter), Some(value)) = (counter, value) {
                    self.change_counter(&counter, &value, special == Special::AddToCounter, range);
                }
            }
            Special::StepCounter => {
                if let Some((counter, _)) = self.group() {
                    let counter = counter.as_str().trim().to_owned();
                    match self.registry().id(&counter) {
                        Some(id) => {
                            self.step_counter(id);
                            if name.as_str() == "refstepcounter" {
                                self.st.label = Arc::from(self.format_counter(id));
                            }
                        }
                        None => self.diagnose(D::UnknownCounter, Some(&counter), range),
                    }
                }
            }
            Special::NewCounter => {
                let counter = self.group().map(|(c, _)| c.as_str().trim().to_owned());
                let within = self.optional().map(|(w, _)| w.as_str().trim().to_owned());
                if let (Some(counter), Some(builder)) = (counter, self.building()) {
                    let within = within.and_then(|w| builder.registry.id(&w));
                    builder.registry.add(&counter, within, Format::Arabic);
                }
            }
            Special::NumberWithin => {
                self.optional();
                let counter = self.group().map(|(c, _)| c.as_str().trim().to_owned());
                let within = self.group().map(|(w, _)| w.as_str().trim().to_owned());
                if let (Some(counter), Some(within), Some(builder)) =
                    (counter, within, self.building())
                    && let (Some(id), Some(parent)) =
                        (builder.registry.id(&counter), builder.registry.id(&within))
                {
                    builder.registry.counters[id].within = Some(parent);
                }
            }
            Special::Appendix => {
                self.st.appendix = true;
                let top = if self.registry().chapters {
                    numbering::CHAPTER
                } else {
                    numbering::SECTION
                };
                self.set_counter(top, 0);
            }
            Special::DocumentClass => {
                self.optional();
                if let Some((class, _)) = self.group()
                    && let Some(builder) = self.building()
                {
                    let class = class.as_str().trim();
                    let chapters = matches!(
                        class,
                        "report"
                            | "book"
                            | "memoir"
                            | "scrbook"
                            | "scrreprt"
                            | "amsbook"
                            | "thesis"
                            | "ociamthesis"
                            | "ucthesis"
                    );
                    let amsmath = class.starts_with("ams");
                    builder.registry = Registry::new(chapters);
                    builder.registry.amsmath |= amsmath;
                }
            }
            Special::UsePackage => {
                self.optional();
                if let Some((packages, _)) = self.group()
                    && let Some(builder) = self.building()
                    && packages
                        .as_str()
                        .split(',')
                        .any(|p| matches!(p.trim(), "amsmath" | "mathtools" | "empheq"))
                {
                    builder.registry.amsmath = true;
                }
                self.optional();
            }
            Special::MakeAtLetter => self.set_at_letter(true),
            Special::MakeAtOther => self.set_at_letter(false),
            Special::IfFalse => self.skip_conditional(range),
            Special::Multicolumn => {
                let columns = self
                    .group()
                    .and_then(|(n, _)| n.as_str().trim().parse::<u16>().ok());
                let spec = self.group().map(|(s, _)| parse_columns(s.as_str()));
                let align = spec
                    .and_then(|s| s.first().map(|c| c.align))
                    .unwrap_or(Align::Center);
                if let Some(table) = self.table_frame() {
                    table.next = Some((columns.unwrap_or(1).max(1), align));
                }
            }
            Special::Rule => {
                // `\cmidrule(lr){2-3}`, `\cline{2-3}`.
                let (rc, pos) = self.cursor();
                let text: &str = rc.as_deref().unwrap_or(self.src);
                if let Some(at) = skip_blank(text, pos)
                    && text.as_bytes()[at] == b'('
                    && let Some(close) = text[at..].find(')')
                {
                    self.commit(at + close + 1);
                }
                match name.as_str() {
                    "cline" | "cmidrule" => {
                        self.group();
                    }
                    "specialrule" => self.skip_args("mmm"),
                    _ => {}
                }
                let in_row = self.table_frame().map(|t| t.row);
                if in_row == Some(false) && body {
                    self.emit(Event::TableRule, range);
                }
            }
            Special::NewCommand | Special::RenewCommand | Special::ProvideCommand => {
                self.new_command(special, range)
            }
            Special::Def { global } => self.def(global, range),
            Special::Let { global } => self.let_(global, range),
            Special::Global | Special::Prefix => self.prefixed(special == Special::Global, range),
            Special::DeclareMathOperator => {
                let star = self.star();
                let name = self.definition_name();
                let text = self.group();
                match (name, text) {
                    (Some(name), Some((text, _))) => {
                        let body = format!(
                            "\\operatorname{}{{{}}}",
                            if star { "*" } else { "" },
                            text.as_str()
                        );
                        self.define(
                            &name,
                            Some(command_binding(&name, 0, None, body)),
                            false,
                            range,
                        );
                    }
                    (Some(name), None) => self.define(&name, None, false, range),
                    _ => self.diagnose(D::MalformedDefinition, Some("DeclareMathOperator"), range),
                }
            }
            Special::NewEnvironment => self.new_environment(range),
            Special::EnsureMath => {
                if let Some((body, _)) = self.group() {
                    self.math(body, MathEnv::Inline, "", range);
                }
            }
        }
    }

    fn set_at_letter(&mut self, value: bool) {
        match self.inputs.last_mut() {
            Some(input) => input.lex.at_letter = value,
            None => self.st.lex.at_letter = value,
        }
        self.st.lex.at_letter = value;
    }

    fn record_label(&mut self, label: String, value: &str, range: Range<usize>) {
        if let Ctx::Build(builder) = &mut self.ctx {
            if builder.labels.len() >= builder.options.limits.max_labels {
                builder
                    .diagnostics
                    .push(D::LimitExceeded, Some("labels"), range);
                return;
            }
            builder.labels.insert(label, Arc::from(value));
        }
    }

    fn resolve_label(&self, label: &str) -> Option<String> {
        match &self.ctx {
            Ctx::Read(index) => index
                .labels
                .get(label)
                .map(|v| v.to_string())
                .filter(|v| !v.is_empty()),
            Ctx::Build(_) => None,
        }
    }

    fn resolve_citation(&self, key: &str) -> Option<String> {
        match &self.ctx {
            Ctx::Read(index) => index.bibliography.get(key).cloned(),
            Ctx::Build(_) => None,
        }
    }

    fn change_counter(&mut self, counter: &str, value: &str, add: bool, range: Range<usize>) {
        if counter == "secnumdepth" {
            if let (Ok(depth), Some(builder)) = (value.parse::<i32>(), self.building()) {
                builder.registry.secnumdepth = depth;
            }
            return;
        }
        if counter == "tocdepth" {
            return;
        }
        let Some(id) = self.registry().id(counter) else {
            self.diagnose(D::UnknownCounter, Some(counter), range);
            return;
        };
        let parsed = value.parse::<i64>().ok().or_else(|| {
            let inner = value.strip_prefix("\\value{")?.strip_suffix('}')?;
            let other = self.registry().id(inner.trim())?;
            Some(i64::from(self.st.counters.get(other).copied().unwrap_or(0)))
        });
        let Some(parsed) = parsed else {
            self.diagnose(D::UnknownCounter, Some(value), range);
            return;
        };
        let current = i64::from(self.st.counters.get(id).copied().unwrap_or(0));
        let next = if add { current + parsed } else { parsed };
        self.set_counter(id, next.clamp(0, i64::from(u32::MAX)) as u32);
    }

    fn skip_conditional(&mut self, range: Range<usize>) {
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        let mut lex = LexState {
            line_start: false,
            at_letter: self.st.lex.at_letter,
        };
        let mut at = pos;
        let mut depth = 1;
        loop {
            let token = lexer::next(text, at, text.len(), &mut lex);
            if token.kind == Kind::Eof {
                self.diagnose(D::UnbalancedGroup, Some("iffalse"), range);
                return;
            }
            at = token.end;
            if token.kind == Kind::Word {
                let word = &text[token.start + 1..token.name_end];
                if word == "fi" {
                    depth -= 1;
                    if depth == 0 {
                        self.commit(at);
                        return;
                    }
                } else if word.starts_with("if") && word != "iff" && word != "ifthenelse" {
                    depth += 1;
                }
            }
        }
    }

    // ------------------------------------------------------------ definitions

    /// `{\name}` or `\name`.
    fn definition_name(&mut self) -> Option<String> {
        if let Some((name, _)) = self.group() {
            let name = name.as_str().trim();
            return name.starts_with('\\').then(|| name.to_owned());
        }
        let token = self.next_token();
        match token.kind {
            Kind::Word | Kind::Symbol => {
                let end = if token.kind == Kind::Word {
                    token.name_end
                } else {
                    token.end
                };
                Some(self.slice(&token, token.start, end).as_str().to_owned())
            }
            _ => {
                self.unread(&token);
                None
            }
        }
    }

    fn define(&mut self, name: &str, binding: Option<Binding>, global: bool, range: Range<usize>) {
        let Ctx::Build(builder) = &mut self.ctx else {
            return;
        };
        if binding.is_none() {
            builder
                .diagnostics
                .push(D::MalformedDefinition, Some(name), range.clone());
        }
        let offset = if self.inputs.is_empty() {
            self.pos
        } else {
            range.end
        };
        if builder.store.assign(name, binding, global, offset) == Assigned::Overflow {
            builder
                .diagnostics
                .push(D::LimitExceeded, Some("definitions"), range);
        }
    }

    fn new_command(&mut self, special: Special, range: Range<usize>) {
        self.star();
        let Some(name) = self.definition_name() else {
            self.diagnose(D::MalformedDefinition, Some("newcommand"), range);
            return;
        };
        let params = self
            .optional()
            .and_then(|(n, _)| n.as_str().trim().parse::<u8>().ok())
            .unwrap_or(0)
            .min(9);
        let default = self.optional().map(|(d, _)| d.as_str().to_owned());
        let Some((body, _)) = self.argument() else {
            self.define(&name, None, false, range);
            return;
        };
        if special == Special::ProvideCommand
            && let Ctx::Build(builder) = &self.ctx
            && builder.store.current(&name) != Lookup::Undefined
        {
            return;
        }
        let binding = command_binding(&name, params, default, body.as_str().to_owned());
        self.define(&name, Some(binding), false, range);
    }

    fn def(&mut self, global: bool, range: Range<usize>) {
        let Some(name) = self.definition_name() else {
            self.diagnose(D::MalformedDefinition, Some("def"), range);
            return;
        };
        // The parameter text runs up to the body's opening brace.
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        let Some(open) = text[pos..].find('{').map(|i| pos + i) else {
            self.define(&name, None, global, range);
            return;
        };
        let parameter_text = text[pos..open].trim().to_owned();
        self.commit(open);
        let Some((body, _)) = self.group() else {
            self.define(&name, None, global, range);
            return;
        };
        let params = parameter_text
            .bytes()
            .filter(u8::is_ascii_digit)
            .max()
            .map_or(0, |d| d - b'0');
        let binding = Binding {
            name: name.clone(),
            kind: BindingKind::Def,
            params,
            default: None,
            parameter_text: Some(parameter_text),
            body: body.as_str().to_owned(),
            end: None,
        };
        self.define(&name, Some(binding), global, range);
    }

    fn let_(&mut self, global: bool, range: Range<usize>) {
        let Some(name) = self.definition_name() else {
            self.diagnose(D::MalformedDefinition, Some("let"), range);
            return;
        };
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        let mut at = pos;
        let bytes = text.as_bytes();
        while at < bytes.len() && matches!(bytes[at], b' ' | b'=') {
            at += 1;
        }
        self.commit(at);
        let Some(target) = self.definition_name() else {
            self.define(&name, None, global, range);
            return;
        };
        let binding = match self.lookup_macro(&target, range.start) {
            Some(mut binding) if binding.end.as_deref() != Some("unknown") => {
                binding.name = name.clone();
                binding
            }
            _ => Binding {
                name: name.clone(),
                kind: BindingKind::Let,
                params: 0,
                default: None,
                parameter_text: None,
                body: target,
                end: None,
            },
        };
        self.define(&name, Some(binding), global, range);
    }

    fn prefixed(&mut self, global: bool, range: Range<usize>) {
        let token = self.next_token();
        if token.kind == Kind::Word {
            let name = self.slice(&token, token.start + 1, token.name_end);
            match name.as_str() {
                "def" | "edef" | "gdef" | "xdef" => {
                    let global = global || matches!(name.as_str(), "gdef" | "xdef");
                    return self.def(global, range);
                }
                "let" => return self.let_(global, range),
                "global" => return self.prefixed(true, range),
                "long" | "protected" | "outer" => return self.prefixed(global, range),
                _ => {}
            }
        }
        self.unread(&token);
    }

    fn new_environment(&mut self, range: Range<usize>) {
        self.star();
        let Some((name, _)) = self.group() else {
            self.diagnose(D::MalformedDefinition, Some("newenvironment"), range);
            return;
        };
        let name = name.as_str().trim().to_owned();
        let params = self
            .optional()
            .and_then(|(n, _)| n.as_str().trim().parse::<u8>().ok())
            .unwrap_or(0)
            .min(9);
        let default = self.optional().map(|(d, _)| d.as_str().to_owned());
        let begin = self.group().map(|(b, _)| b.as_str().to_owned());
        let end = self.group().map(|(e, _)| e.as_str().to_owned());
        let binding = match (begin, end) {
            (Some(begin), Some(end)) => Some(Binding {
                name: name.clone(),
                kind: BindingKind::Environment,
                params,
                default,
                parameter_text: None,
                body: begin,
                end: Some(end),
            }),
            _ => None,
        };
        self.define(&name, binding, false, range);
    }

    fn new_theorem(&mut self) {
        let star = self.star();
        let Some((name, _)) = self.group() else {
            return;
        };
        let shared = self.optional().map(|(s, _)| s.as_str().trim().to_owned());
        let display = self.group().map(|(d, _)| d.as_str().trim().to_owned());
        let within = self.optional().map(|(w, _)| w.as_str().trim().to_owned());
        let name = name.as_str().trim().to_owned();
        let Some(builder) = self.building() else {
            return;
        };
        let registry = &mut builder.registry;
        let counter = if star {
            None
        } else if let Some(shared) = shared {
            registry
                .id(&shared)
                .or_else(|| registry.theorems.get(&shared).and_then(|t| t.counter))
        } else {
            let within = within.and_then(|w| registry.id(&w));
            Some(registry.add(&name, within, Format::Arabic))
        };
        let style = registry.style;
        registry.theorems.insert(
            name,
            Theorem {
                display: display.unwrap_or_default(),
                counter,
                style,
            },
        );
    }

    // ------------------------------------------------------------ environments

    fn begin(&mut self, range: Range<usize>) {
        let Some((name, name_range)) = self.group() else {
            return;
        };
        let range = range.start..range.end.max(name_range.end);
        let name = name.as_str().trim().to_owned();
        let synthetic = self.inputs.last().is_some_and(|i| i.synthetic);
        if synthetic && name == SYNTHETIC_TITLE {
            self.close_inline();
            if self.push_frame(FrameKind::Env(
                Arc::from(name.as_str()),
                Env::Block(TagKind::TitleBlock),
            )) {
                self.emit(Event::Start(Tag::TitleBlock), range);
            }
            return;
        }
        if let Some((class, args)) = commands::environment(&name) {
            self.known_env(&name, class, args, range);
            return;
        }
        if let Some(theorem) = self.theorem(&name) {
            self.theorem_env(&name, theorem, range);
            return;
        }
        if let Some(binding) = self.lookup_macro(&name, range.start)
            && binding.kind == BindingKind::Environment
        {
            self.user_env(&name, binding, range);
            return;
        }
        if self.st.phase != Phase::Body {
            self.push_frame(FrameKind::Env(Arc::from(name.as_str()), Env::Transparent));
            return;
        }
        self.diagnose(D::UnknownEnvironment, Some(&name), range.clone());
        self.close_inline();
        // Adjacent arguments stay with the visible `\begin`.
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        let mut end = pos;
        let bytes = text.as_bytes();
        while let Some(&open @ (b'{' | b'[')) = bytes.get(end) {
            let close = if open == b'{' { b'}' } else { b']' };
            match math::matching(text, end + 1, text.len(), open, close) {
                Some(c) => end = c + 1,
                None => break,
            }
        }
        let begin_text = format!("\\begin{{{name}}}{}", &text[pos..end]);
        self.commit(end);
        let span = self.make_span(&rc, range.start, end.max(range.end));
        if self.push_frame(FrameKind::Env(Arc::from(name.as_str()), Env::Unsupported)) {
            self.emit(
                Event::Start(Tag::Unsupported {
                    name: Cow::Owned(name),
                    block: true,
                }),
                span.clone(),
            );
            self.emit(Event::Raw(Cow::Owned(begin_text)), span);
        }
    }

    fn theorem(&self, name: &str) -> Option<Theorem> {
        if let Some(theorem) = self.registry().theorems.get(name) {
            return Some(theorem.clone());
        }
        commands::predefined_theorem(name).map(|display| Theorem {
            display: display.to_owned(),
            counter: None,
            style: TheoremStyle::Plain,
        })
    }

    fn theorem_env(&mut self, name: &str, theorem: Theorem, range: Range<usize>) {
        self.close_inline();
        if !self.push_frame(FrameKind::Env(
            Arc::from(name),
            Env::Block(TagKind::Theorem),
        )) {
            return;
        }
        let number = theorem.counter.map(|id| {
            self.step_counter(id);
            let number = self.format_counter(id);
            self.st.label = Arc::from(number.as_str());
            number
        });
        self.emit(
            Event::Start(Tag::Theorem {
                name: theorem.display,
                number,
                style: theorem.style,
            }),
            range.clone(),
        );
        self.st.para.indent = false;
        self.theorem_note(range);
    }

    fn theorem_note(&mut self, range: Range<usize>) {
        if self.peek() == Some(b'[') && self.st.phase == Phase::Body {
            let (rc, pos) = self.cursor();
            let text: &str = rc.as_deref().unwrap_or(self.src);
            if let Some(at) = skip_blank(text, pos) {
                self.commit(at + 1);
                self.open_block_inline(Tag::TheoremNote, true, range);
            }
        }
    }

    fn user_env(&mut self, name: &str, binding: Binding, range: Range<usize>) {
        let end = Arc::from(binding.end.clone().unwrap_or_default());
        if !self.push_frame(FrameKind::Env(Arc::from(name), Env::User(end))) {
            return;
        }
        let mut args = Vec::new();
        let mut params = binding.params;
        if let Some(default) = &binding.default {
            args.push(
                self.optional()
                    .map_or_else(|| default.clone(), |(a, _)| a.as_str().to_owned()),
            );
            params = params.saturating_sub(1);
        }
        for _ in 0..params {
            args.push(
                self.argument()
                    .map(|(a, _)| a.as_str().to_owned())
                    .unwrap_or_default(),
            );
        }
        let expansion = substitute(&binding.body, &args);
        let span = self.inputs.last().map_or(range, |i| i.span.clone());
        self.inputs.push(Input {
            text: Rc::from(expansion),
            pos: 0,
            lex: LexState {
                line_start: false,
                at_letter: self.st.lex.at_letter,
            },
            span,
            synthetic: false,
            close_env: false,
        });
    }

    fn known_env(&mut self, name: &str, class: EnvClass, args: &str, range: Range<usize>) {
        let body = self.st.phase == Phase::Body;
        match class {
            EnvClass::Document => {
                self.push_frame(FrameKind::Env(Arc::from(name), Env::Document));
                self.st.phase = Phase::Body;
            }
            EnvClass::List(kind) => {
                self.skip_args(args);
                self.close_inline();
                let depth = self
                    .st
                    .stack
                    .iter()
                    .filter(|f| matches!(f.kind, FrameKind::Env(_, Env::List { kind: k, .. }) if k == kind))
                    .count() as u8;
                if self.push_frame(FrameKind::Env(
                    Arc::from(name),
                    Env::List {
                        kind,
                        depth,
                        count: 0,
                        item: false,
                    },
                )) {
                    self.emit(Event::Start(Tag::List(kind)), range);
                }
            }
            EnvClass::Quote(kind) => self.block_env(name, Tag::Quote(kind), range),
            EnvClass::Abstract => self.block_env(name, Tag::Abstract, range),
            EnvClass::Align(align) => self.block_env(name, Tag::Align(align), range),
            EnvClass::Proof => {
                self.block_env(name, Tag::Proof, range.clone());
                self.theorem_note(range);
            }
            EnvClass::Float(kind) => {
                self.skip_args(args);
                self.close_inline();
                if self.push_frame(FrameKind::Env(Arc::from(name), Env::Float(kind))) {
                    self.emit(Event::Start(Tag::Figure(kind)), range);
                }
            }
            EnvClass::Tabular => {
                let mut columns = None;
                for arg in args.bytes() {
                    match arg {
                        b'o' => {
                            self.optional();
                        }
                        _ => columns = self.group().map(|(c, _)| parse_columns(c.as_str())),
                    }
                }
                self.close_inline();
                let columns: Arc<[Column]> = columns.unwrap_or_default().into();
                if self.push_frame(FrameKind::Env(
                    Arc::from(name),
                    Env::Table(Box::new(Table {
                        columns: Arc::clone(&columns),
                        row: false,
                        col: 0,
                        span: 1,
                        next: None,
                    })),
                )) {
                    self.emit(Event::Start(Tag::Table { columns }), range);
                }
            }
            EnvClass::Bibliography => {
                self.skip_args(args);
                self.close_inline();
                if self.push_frame(FrameKind::Env(
                    Arc::from(name),
                    Env::Bibliography {
                        count: 0,
                        item: false,
                    },
                )) {
                    self.emit(Event::Start(Tag::Bibliography), range);
                }
            }
            EnvClass::Subequations => {
                self.step_counter(numbering::EQUATION);
                let parent = self.format_counter(numbering::EQUATION);
                if self.push_frame(FrameKind::Env(
                    Arc::from(name),
                    Env::Subequations {
                        parent: parent.clone(),
                        count: 0,
                    },
                )) {
                    self.st.label = Arc::from(parent);
                }
            }
            EnvClass::Transparent => {
                self.skip_args(args);
                self.push_frame(FrameKind::Env(Arc::from(name), Env::Transparent));
            }
            EnvClass::Verbatim(language) => self.verbatim(name, language, range),
            EnvClass::Hidden | EnvClass::RawBlock => {
                let (rc, pos) = self.cursor();
                let text: &str = rc.as_deref().unwrap_or(self.src);
                let found = math::find_env_end(text, pos, text.len(), name);
                let after = found.map_or(text.len(), |(_, after)| after);
                if found.is_none() {
                    self.diagnose(D::UnterminatedEnvironment, Some(name), range.clone());
                }
                let raw = format!("\\begin{{{name}}}{}", &text[pos..after]);
                self.commit(after);
                if class == EnvClass::RawBlock && body {
                    let span = self.make_span(&rc, range.start, after);
                    self.close_inline();
                    self.emit(
                        Event::Start(Tag::Unsupported {
                            name: Cow::Owned(name.to_owned()),
                            block: true,
                        }),
                        span.clone(),
                    );
                    self.emit(Event::Raw(Cow::Owned(raw)), span.clone());
                    self.emit(Event::End(TagKind::Unsupported), span);
                }
            }
            EnvClass::Math(kind) => {
                let (rc, pos) = self.cursor();
                let text: &str = rc.as_deref().unwrap_or(self.src);
                match math::find_env_end(text, pos, text.len(), name) {
                    Some((body_end, after)) => {
                        let body_text = self.make_str(&rc, pos, body_end);
                        self.commit(after);
                        let span = self.make_span(&rc, range.start, after);
                        self.math(body_text, kind, name, span);
                    }
                    None => {
                        self.diagnose(D::UnterminatedMath, Some(name), range.clone());
                        self.push_frame(FrameKind::Env(Arc::from(name), Env::Transparent));
                    }
                }
            }
        }
    }

    fn block_env(&mut self, name: &str, tag: Tag<'a>, range: Range<usize>) {
        self.close_inline();
        let kind = tag.kind();
        if self.push_frame(FrameKind::Env(Arc::from(name), Env::Block(kind))) {
            self.emit(Event::Start(tag), range);
            self.st.para.indent = false;
        }
    }

    fn end(&mut self, range: Range<usize>) {
        let Some((name, name_range)) = self.group() else {
            return;
        };
        let range = range.start..range.end.max(name_range.end);
        let name = name.as_str().trim().to_owned();
        let Some(index) = self
            .st
            .stack
            .iter()
            .rposition(|f| matches!(&f.kind, FrameKind::Env(n, _) if **n == *name))
        else {
            if self.st.phase == Phase::Body {
                self.diagnose(D::MismatchedEnd, Some(&name), range);
            }
            return;
        };
        while self.st.stack.len() > index + 1 {
            let unclosed = match &self.st.stack.last().unwrap().kind {
                FrameKind::Env(n, _) => n.to_string(),
                _ => "{".to_owned(),
            };
            self.diagnose(D::MismatchedEnd, Some(&unclosed), range.clone());
            self.close_top(range.end);
        }
        let user_end = match &self.st.stack[index].kind {
            FrameKind::Env(_, Env::User(end)) => Some(Arc::clone(end)),
            _ => None,
        };
        if let Some(end) = user_end {
            // Run the end code inside the environment, then close it.
            self.inputs.push(Input {
                text: Rc::from(&*end),
                pos: 0,
                lex: LexState {
                    line_start: false,
                    at_letter: self.st.lex.at_letter,
                },
                span: range,
                synthetic: false,
                close_env: true,
            });
            return;
        }
        if name == "document" {
            self.close_top(range.end);
            self.close_all(range.end);
            self.st.phase = Phase::Ended;
            return;
        }
        if matches!(
            self.st.stack[index].kind,
            FrameKind::Env(_, Env::Unsupported)
        ) {
            self.close_inline();
            self.emit(
                Event::Raw(Cow::Owned(format!("\\end{{{name}}}"))),
                range.clone(),
            );
        }
        self.close_top(range.end);
    }

    fn close_all(&mut self, at: usize) {
        self.close_inline();
        while !self.st.stack.is_empty() {
            self.close_top(at);
        }
    }

    fn item(&mut self, token: &Tok, bib: bool, range: Range<usize>) {
        if self.st.para.open && !self.st.para.block && !self.in_inline_frame() && token.depth == 0 {
            // End the paragraph first, so a checkpoint can precede the item.
            self.unread(token);
            self.close_inline();
            return;
        }
        let Some(index) = self.st.stack.iter().rposition(|f| {
            matches!(
                f.kind,
                FrameKind::Env(_, Env::List { .. } | Env::Bibliography { .. })
            )
        }) else {
            if bib {
                self.optional();
                self.group();
            }
            self.diagnose(D::MismatchedEnd, Some("item"), range);
            return;
        };
        while self.st.stack.len() > index + 1 {
            self.close_top(range.start);
        }
        self.close_inline();
        let at = range.start..range.start;
        if bib {
            let label = self.optional().map(|(l, _)| note_text(l.as_str()));
            let key = self.group().map(|(k, _)| k.as_str().trim().to_owned());
            let FrameKind::Env(_, Env::Bibliography { count, item }) =
                &mut self.st.stack[index].kind
            else {
                return;
            };
            let close = *item;
            *item = true;
            *count += 1;
            let label = label.unwrap_or_else(|| count.to_string());
            if close {
                self.emit(Event::End(TagKind::BibItem), at.clone());
            }
            if let (Some(key), Some(builder)) = (key, self.building()) {
                builder.bibliography.insert(key, label.clone());
            }
            self.emit(
                Event::Start(Tag::BibItem {
                    label: format!("[{label}]"),
                }),
                range,
            );
            self.st.para.indent = false;
            return;
        }
        let has_label = self.peek() == Some(b'[');
        let FrameKind::Env(
            _,
            Env::List {
                kind,
                depth,
                count,
                item,
            },
        ) = &mut self.st.stack[index].kind
        else {
            return;
        };
        let close = *item;
        *item = true;
        let generated = if *kind == ListKind::Enumerate && !has_label {
            *count += 1;
            Some(enumerate_label(*depth, *count))
        } else {
            None
        };
        if close {
            self.emit(Event::End(TagKind::Item), at);
        }
        if let Some((label, reference)) = &generated {
            self.st.label = Arc::from(reference.as_str());
            let _ = label;
        }
        self.emit(
            Event::Start(Tag::Item {
                label: generated.map(|(label, _)| label),
            }),
            range.clone(),
        );
        self.st.para.indent = false;
        if has_label {
            let (rc, pos) = self.cursor();
            let text: &str = rc.as_deref().unwrap_or(self.src);
            if let Some(at) = skip_blank(text, pos) {
                self.commit(at + 1);
                self.open_block_inline(Tag::ItemLabel, true, range);
            }
        }
    }

    fn caption(&mut self, name: &Str<'a>, range: Range<usize>) {
        let kind = if name.as_str() == "captionof" {
            match self
                .group()
                .map(|(k, _)| k.as_str().trim().to_owned())
                .as_deref()
            {
                Some("table") => FigureKind::Table,
                _ => FigureKind::Figure,
            }
        } else {
            self.st
                .stack
                .iter()
                .rev()
                .find_map(|f| match f.kind {
                    FrameKind::Env(_, Env::Float(kind)) => Some(kind),
                    _ => None,
                })
                .unwrap_or(FigureKind::Figure)
        };
        self.star();
        self.optional();
        let counter = match kind {
            FigureKind::Figure => Some(numbering::FIGURE),
            FigureKind::Table => Some(numbering::TABLE),
            FigureKind::Algorithm => {
                if let Some(builder) = self.building() {
                    builder.registry.add("algorithm", None, Format::Arabic);
                }
                self.registry().id("algorithm")
            }
        };
        let number = counter.map(|id| {
            self.step_counter(id);
            let number = self.format_counter(id);
            self.st.label = Arc::from(number.as_str());
            number
        });
        if self.peek() == Some(b'{') && self.st.phase == Phase::Body {
            self.raw_open_brace();
            self.open_block_inline(Tag::Caption { number, kind }, false, range);
        } else {
            self.group();
        }
    }

    fn make_title(&mut self, range: Range<usize>) {
        let Ctx::Read(index) = &self.ctx else {
            // The title block changes no counters: LaTeX resets footnotes
            // after it, and the first pass does not need its text.
            return;
        };
        if self.st.phase != Phase::Body {
            return;
        }
        let mut synthetic = format!("\\begin{{{SYNTHETIC_TITLE}}}");
        for (part, text) in ["title", "author", "date"].iter().zip(&index.title) {
            if let Some(text) = text
                && !text.trim().is_empty()
            {
                synthetic.push_str(&format!("\\{SYNTHETIC_TITLE}{{{part}}}{{{text}}}"));
            }
        }
        synthetic.push_str(&format!("\\end{{{SYNTHETIC_TITLE}}}"));
        self.inputs.push(Input {
            text: Rc::from(synthetic),
            pos: 0,
            lex: LexState {
                line_start: false,
                at_letter: false,
            },
            span: range,
            synthetic: true,
            close_env: false,
        });
    }

    fn title_part(&mut self, range: Range<usize>) {
        let part = self.group().map(|(p, _)| p.as_str().to_owned());
        let tag = match part.as_deref() {
            Some("title") => Tag::Title,
            Some("author") => Tag::Author,
            _ => Tag::Date,
        };
        if self.peek() == Some(b'{') {
            self.raw_open_brace();
            self.open_block_inline(tag, false, range);
        }
    }

    // ------------------------------------------------------------------- code

    fn verbatim(&mut self, name: &str, language: Language, range: Range<usize>) {
        let language = match language {
            Language::None => None,
            Language::Optional => self.optional().and_then(|(options, _)| {
                options.as_str().split(',').find_map(|option| {
                    let (key, value) = option.split_once('=')?;
                    (key.trim() == "language").then(|| Cow::Owned(value.trim().to_lowercase()))
                })
            }),
            Language::Mandatory => {
                self.optional();
                self.group().map(|(l, _)| l.trimmed())
            }
        };
        self.close_inline();
        let marker = format!("\\end{{{name}}}");
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        // The rest of the `\begin` line is not content.
        let start = match text[pos..].find('\n') {
            Some(i) if text[pos..pos + i].trim().is_empty() => pos + i + 1,
            _ => pos,
        };
        let found = text[start..].find(&marker).map(|i| start + i);
        if found.is_none() {
            self.diagnose(D::UnterminatedEnvironment, Some(name), range.clone());
        }
        let content_end = found.unwrap_or(text.len());
        self.emit(Event::Start(Tag::Code { language }), range.clone());
        if rc.is_some() {
            // Inside an expansion: emit the content at once.
            let content = text[start..content_end].to_owned();
            self.commit(found.map_or(text.len(), |f| f + marker.len()));
            let span = self.inputs.last().map_or(range.clone(), |i| i.span.clone());
            if self.wants_text() {
                self.append(Cow::Owned(content), span.clone());
            }
            self.emit(Event::End(TagKind::Code), span);
            return;
        }
        self.pos = start;
        self.st.lex.line_start = true;
        self.st.verbatim = Some(Verbatim {
            end: content_end,
            marker: marker.len(),
        });
    }

    /// One line of verbatim content, so checkpoints can fall between lines.
    fn verbatim_line(&mut self) {
        let Some(verbatim) = self.st.verbatim.clone() else {
            return;
        };
        if self.pos >= verbatim.end {
            let at = self.pos;
            let after = (verbatim.end + verbatim.marker).min(self.src.len());
            self.emit(Event::End(TagKind::Code), at..after);
            self.pos = after;
            self.st.lex.line_start = false;
            self.st.verbatim = None;
            self.st.para.indent = false;
            return;
        }
        let line_end = self.src[self.pos..verbatim.end]
            .find('\n')
            .map_or(verbatim.end, |i| self.pos + i + 1);
        let range = self.pos..line_end;
        if self.wants_text() {
            self.append(Cow::Borrowed(&self.src[range.clone()]), range);
        }
        self.pos = line_end;
    }

    fn verb(&mut self, range: Range<usize>) {
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        let mut at = pos;
        if text.as_bytes().get(at) == Some(&b'*') {
            at += 1;
        }
        let Some(delimiter) = text[at..].chars().next() else {
            return;
        };
        let start = at + delimiter.len_utf8();
        let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        let Some(end) = text[start..line_end].find(delimiter).map(|i| start + i) else {
            self.diagnose(D::UnterminatedMath, Some("verb"), range.clone());
            return;
        };
        self.commit(end + delimiter.len_utf8());
        let code = self.make_str(&rc, start, end);
        let code_range = self.make_span(&rc, start, end);
        self.inline_code(code, code_range, range);
    }

    fn inline_code(&mut self, code: Str<'a>, code_range: Range<usize>, range: Range<usize>) {
        if self.st.phase != Phase::Body {
            return;
        }
        self.inline_start(range.start);
        self.emit(Event::Start(Tag::InlineCode), range.clone());
        if self.wants_text() {
            self.append(code.cow(), code_range);
        }
        self.emit(Event::End(TagKind::InlineCode), range);
    }

    // ------------------------------------------------------------------- math

    fn dollar(&mut self, token: &Tok, range: Range<usize>) {
        let double = self.peek_immediate() == Some(b'$');
        if double {
            let (rc, pos) = self.cursor();
            self.commit(pos + 1);
            let _ = rc;
        }
        let delim = if double {
            Delim::DoubleDollar
        } else {
            Delim::Dollar
        };
        let _ = token;
        self.delimited_math(delim, double, range);
    }

    fn delimited_math(&mut self, delim: Delim, display: bool, range: Range<usize>) {
        let (rc, pos) = self.cursor();
        let text: &str = rc.as_deref().unwrap_or(self.src);
        let Some(close) = math::find_end(text, pos, text.len(), delim) else {
            self.diagnose(D::UnterminatedMath, None, range.clone());
            let shown = match delim {
                Delim::Dollar => "$",
                Delim::DoubleDollar => "$$",
                Delim::Paren => "\\(",
                Delim::Bracket => "\\[",
            };
            self.push_text(Cow::Borrowed(shown), range);
            return;
        };
        let width = match delim {
            Delim::Dollar => 1,
            _ => 2,
        };
        let body = self.make_str(&rc, pos, close);
        self.commit(close + width);
        let span = self.make_span(&rc, range.start, close + width);
        let kind = if display {
            MathEnv::Unnumbered
        } else {
            MathEnv::Inline
        };
        self.math(body, kind, "", span);
    }

    fn subequation(&mut self) -> Option<String> {
        let frame = self
            .st
            .stack
            .iter_mut()
            .rev()
            .find_map(|f| match &mut f.kind {
                FrameKind::Env(_, Env::Subequations { parent, count }) => Some((parent, count)),
                _ => None,
            })?;
        *frame.1 += 1;
        Some(format!("{}{}", frame.0, text::alph(*frame.1, false)))
    }

    fn next_equation(&mut self) -> String {
        if let Some(number) = self.subequation() {
            return number;
        }
        self.step_counter(numbering::EQUATION);
        self.format_counter(numbering::EQUATION)
    }

    fn bind_labels(&mut self, labels: &[String], value: Option<&str>, range: &Range<usize>) {
        match (labels, value) {
            ([], _) => {}
            ([label], Some(value)) => self.record_label(label.clone(), value, range.clone()),
            _ => self.diagnose(
                D::UnsupportedLabelPlacement,
                labels.first().map(String::as_str),
                range.clone(),
            ),
        }
    }

    /// Numbers a formula's rows, binds its labels, and emits it.
    fn math(&mut self, body: Str<'a>, kind: MathEnv, env: &str, range: Range<usize>) {
        let max = self.limits().max_math;
        let numbered_env = matches!(kind, MathEnv::Equation | MathEnv::Rows | MathEnv::Multline);
        if body.as_str().len() > max {
            self.diagnose(D::LimitExceeded, Some("math"), range.clone());
            if numbered_env && numbering::EQUATION < 64 {
                self.st.uncertain |= 1 << numbering::EQUATION;
            }
            if self.st.phase == Phase::Body {
                let raw = self
                    .src
                    .get(range.clone())
                    .map_or_else(|| Cow::Owned(body.as_str().to_owned()), Cow::Borrowed);
                self.inline(Event::Raw(raw), range);
            }
            return;
        }
        let split = is_rows_env(env);
        let analysis = math::analyze(body.as_str(), split, env.starts_with("eqnarray"));
        for global in &analysis.globals {
            let text = body.as_str()[global.clone()].to_owned();
            self.global_definition(&text, range.clone());
        }
        let mut tags: Vec<Option<String>> = vec![None; analysis.rows.len()];
        // The tag of a single-number formula goes after the whole body.
        let mut single = None;
        let current = Arc::clone(&self.st.label);
        match kind {
            MathEnv::Inline => {
                for row in &analysis.rows {
                    self.bind_labels(&row.labels, Some(&current), &range);
                }
            }
            MathEnv::Unnumbered => {
                let single = !split;
                for row in &analysis.rows {
                    match (&row.tag, single) {
                        (Some(tag), _) => self.bind_labels(&row.labels, Some(tag), &range),
                        (None, true) => self.bind_labels(&row.labels, Some(&current), &range),
                        (None, false) => self.bind_labels(&row.labels, None, &range),
                    }
                }
            }
            MathEnv::Equation | MathEnv::Multline => {
                let notag = analysis.rows.iter().any(|r| r.notag);
                let explicit = analysis.rows.iter().find_map(|r| r.tag.clone());
                let labels: Vec<String> = analysis
                    .rows
                    .iter()
                    .flat_map(|r| r.labels.clone())
                    .collect();
                let value = if let Some(tag) = explicit {
                    Some(tag)
                } else if notag {
                    None
                } else {
                    let number = self.next_equation();
                    single = Some(format!("({number})"));
                    Some(number)
                };
                self.bind_labels(&labels, value.as_deref(), &range);
            }
            MathEnv::Rows => {
                let eqnarray = env.starts_with("eqnarray");
                let mut pending: Vec<String> = Vec::new();
                let last = analysis.rows.len() - 1;
                for (i, row) in analysis.rows.iter().enumerate() {
                    let value = if let Some(tag) = &row.tag {
                        Some(tag.clone())
                    } else if row.notag {
                        None
                    } else {
                        let number = self.next_equation();
                        // KaTeX drops an empty last row; LaTeX still numbers it.
                        if !(i == last && row.empty) {
                            tags[i] = Some(format!("({number})"));
                        }
                        Some(number)
                    };
                    if value.is_none() && eqnarray && !row.labels.is_empty() {
                        self.bind_labels(&row.labels, None, &range);
                        continue;
                    }
                    pending.extend(row.labels.iter().cloned());
                    if value.is_some() {
                        let labels = std::mem::take(&mut pending);
                        self.bind_labels(&labels, value.as_deref(), &range);
                    }
                }
                // Labels still pending at the end are never defined.
            }
        }
        if self.st.phase != Phase::Body {
            return;
        }
        let (prepared, context) = match &self.ctx {
            Ctx::Read(index) => {
                let resolve = |kind: RefKind, label: &str| {
                    Reference {
                        kind,
                        label: Cow::Borrowed(label),
                        resolved: index.labels.get(label).map(|v| v.to_string()),
                    }
                    .display()
                };
                let inner = math::prepare(body.as_str(), &analysis, &tags, resolve);
                let prepared = wrap(kind, env, inner, single);
                (prepared, MacroContext::at(&index.store, range.start))
            }
            Ctx::Build(_) => (String::new(), MacroContext::empty()),
        };
        let display = kind != MathEnv::Inline;
        self.inline(
            Event::Math(Math {
                display,
                prepared: Cow::Owned(prepared),
                context,
            }),
            range,
        );
        if display {
            // TeX ignores spaces after a display, as after a line break.
            self.st.para.space = None;
            self.st.para.content = false;
        }
    }

    /// Applies `\gdef` and similar definitions found inside a formula.
    fn global_definition(&mut self, text: &str, range: Range<usize>) {
        if !matches!(self.ctx, Ctx::Build(_)) {
            return;
        }
        let saved_pos = self.pos;
        let saved_lex = self.st.lex;
        self.inputs.push(Input {
            text: Rc::from(text),
            pos: 0,
            lex: LexState {
                line_start: false,
                at_letter: false,
            },
            span: range.clone(),
            synthetic: false,
            close_env: false,
        });
        let depth = self.inputs.len();
        let token = self.next_token();
        if token.kind == Kind::Word {
            let name = self.slice(&token, token.start + 1, token.name_end);
            match name.as_str() {
                "gdef" | "xdef" => self.def(true, range),
                "global" => self.prefixed(true, range),
                _ => {}
            }
        }
        self.inputs.truncate(depth - 1);
        self.pos = saved_pos;
        self.st.lex = saved_lex;
    }

    // ------------------------------------------------------------------ tables

    fn cell_end(&mut self, range: Range<usize>) {
        if !self.st.para.cell {
            self.open_inline(range.start);
        }
        self.close_inline();
    }

    fn row_end(&mut self, range: Range<usize>) {
        if !self.st.para.cell {
            let row = self.table_frame().map(|t| t.row);
            if row == Some(false) {
                self.open_inline(range.start);
            }
        }
        if self.st.para.cell {
            self.close_inline();
        }
        if let Some(table) = self.table_frame() {
            table.row = false;
            table.col = 0;
            table.next = None;
        }
        self.emit(Event::End(TagKind::Row), range);
    }
}

/// Skips spaces, comments, and at most one line end; `None` at the end.
fn skip_blank(text: &str, mut pos: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut newlines = 0;
    while pos < bytes.len() {
        match bytes[pos] {
            b' ' | b'\t' | b'\r' => pos += 1,
            b'\n' => {
                newlines += 1;
                if newlines > 1 {
                    return None;
                }
                pos += 1;
            }
            b'%' => {
                pos = bytes[pos..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |i| pos + i + 1);
            }
            _ => return Some(pos),
        }
    }
    None
}

fn owned_str<'a>(text: &str) -> Str<'a> {
    Str::Shared(Rc::from(text), 0, text.len())
}

fn substitute(body: &str, args: &[String]) -> String {
    if !body.contains('#') {
        return body.to_owned();
    }
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '#' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('#') => {
                chars.next();
                out.push('#');
            }
            Some(d @ '1'..='9') => {
                chars.next();
                let index = d as usize - '1' as usize;
                if let Some(arg) = args.get(index) {
                    out.push_str(arg);
                }
            }
            _ => out.push('#'),
        }
    }
    out
}

fn command_binding(name: &str, params: u8, default: Option<String>, body: String) -> Binding {
    Binding {
        name: name.to_owned(),
        kind: BindingKind::Command,
        params,
        default,
        parameter_text: None,
        body,
        end: None,
    }
}

fn enumerate_label(depth: u8, count: u32) -> (String, String) {
    match depth {
        0 => (format!("{count}."), count.to_string()),
        1 => (
            format!("({})", text::alph(count, false)),
            text::alph(count, false),
        ),
        2 => (
            format!("{}.", text::roman(count, false)),
            text::roman(count, false),
        ),
        _ => (
            format!("{}.", text::alph(count, true)),
            text::alph(count, true),
        ),
    }
}

/// Column alignments from a `tabular` specification.
pub(crate) fn parse_columns(spec: &str) -> Vec<Column> {
    let bytes = spec.as_bytes();
    let mut columns = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        i += 1;
        let group = |i: &mut usize| -> Option<String> {
            let mut at = *i;
            while at < bytes.len() && bytes[at] == b' ' {
                at += 1;
            }
            if bytes.get(at) != Some(&b'{') {
                return None;
            }
            let close = math::matching(spec, at + 1, spec.len(), b'{', b'}')?;
            *i = close + 1;
            Some(spec[at + 1..close].to_owned())
        };
        match c {
            b'l' | b'X' | b'L' => columns.push(Column {
                align: Align::Left,
                width: None,
            }),
            b'c' | b'C' => columns.push(Column {
                align: Align::Center,
                width: None,
            }),
            b'r' | b'R' | b'S' => columns.push(Column {
                align: Align::Right,
                width: None,
            }),
            b'p' | b'm' | b'b' => {
                let width = group(&mut i);
                columns.push(Column {
                    align: Align::Left,
                    width,
                });
            }
            b'@' | b'!' | b'>' | b'<' => {
                group(&mut i);
            }
            b'D' => {
                group(&mut i);
                group(&mut i);
                group(&mut i);
                columns.push(Column {
                    align: Align::Center,
                    width: None,
                });
            }
            b'*' => {
                let count = group(&mut i).and_then(|n| n.trim().parse::<usize>().ok());
                let inner = group(&mut i).map(|s| parse_columns(&s)).unwrap_or_default();
                for _ in 0..count.unwrap_or(1).min(256) {
                    columns.extend(inner.iter().cloned());
                }
            }
            _ => {}
        }
    }
    columns
}

/// Wraps a prepared body in an environment KaTeX supports, without numbering.
fn wrap(kind: MathEnv, env: &str, inner: String, tag: Option<String>) -> String {
    let base = env.trim_end_matches('*');
    match kind {
        MathEnv::Inline => inner,
        MathEnv::Equation => match tag {
            Some(tag) => format!("{inner} \\tag*{{{tag}}}"),
            None => inner,
        },
        MathEnv::Multline => {
            let tag = tag.map(|t| format!(" \\tag*{{{t}}}")).unwrap_or_default();
            format!("\\begin{{gathered}}{inner}\\end{{gathered}}{tag}")
        }
        MathEnv::Rows | MathEnv::Unnumbered => {
            let name = match base {
                "align" | "flalign" | "eqnarray" => "align*",
                "gather" => "gather*",
                "alignat" => "alignat*",
                "multline" => "gathered",
                _ => return inner,
            };
            format!("\\begin{{{name}}}{inner}\\end{{{name}}}")
        }
    }
}

impl<'a> Parser<'a> {
    pub(crate) fn set_cancel(&mut self, cancel: &'a AtomicBool) {
        self.cancel = Some(cancel);
        self.next_cancel = self.pos;
    }
}

/// Converts a short text argument, such as a citation note or a bibliography
/// label, without events: symbols and ligatures become characters, groups
/// disappear, styles and other commands with a braced argument keep only the
/// argument's text, and commands without one stay as written.
fn note_text(note: &str) -> String {
    let note = note.trim();
    let mut out = String::with_capacity(note.len());
    let mut chars = note.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' | '}' => {}
            '~' => out.push('\u{a0}'),
            '\\' => {
                let start = i + 1;
                let mut end = start;
                while let Some(&(j, d)) = chars.peek() {
                    if !d.is_ascii_alphabetic() {
                        break;
                    }
                    end = j + d.len_utf8();
                    chars.next();
                }
                if end == start {
                    let Some((_, d)) = chars.next() else {
                        out.push('\\');
                        break;
                    };
                    let name = &note[start..start + d.len_utf8()];
                    match commands::symbol(name) {
                        Some(text) => out.push_str(text),
                        None => {
                            out.push('\\');
                            out.push(d);
                        }
                    }
                    continue;
                }
                let name = &note[start..end];
                let text = match commands::command(name).map(|spec| spec.cmd) {
                    Some(Cmd::Symbol(text)) => Some(text),
                    Some(Cmd::Style(_) | Cmd::Declare(_) | Cmd::Transparent) => Some(""),
                    _ if note[end..].starts_with('{') => Some(""),
                    _ => None,
                };
                match text {
                    Some(text) => {
                        out.push_str(text);
                        while chars.peek().is_some_and(|&(_, d)| d.is_whitespace()) {
                            chars.next();
                        }
                    }
                    None => out.push_str(&note[i..end]),
                }
            }
            c => out.push(c),
        }
    }
    text::ligatures(&out).into_owned()
}
