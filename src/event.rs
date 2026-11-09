//! Events reported for a document. Each carries the source range it came from;
//! a range is a span, not a byte-for-byte mapping, since text is decoded.

use std::{borrow::Cow, ops::Range, sync::Arc};

use crate::macros::MacroContext;

#[derive(Clone, Debug, PartialEq)]
pub struct Spanned<'a> {
    pub event: Event<'a>,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event<'a> {
    Start(Tag<'a>),
    End(TagKind),
    /// Decoded text: ligatures, accents, and escapes are already applied.
    Text(Cow<'a, str>),
    Math(Math<'a>),
    Reference(Reference<'a>),
    Citation(Citation<'a>),
    Image(Image<'a>),
    /// `\input`, `\include`, or a bibliography file, for the host to resolve.
    Include(Include<'a>),
    Hint(Hint),
    LineBreak,
    /// A horizontal rule between table rows, such as `\hline` or `\midrule`.
    TableRule,
    /// Source shown as is, such as an unknown command or its delimiters.
    Raw(Cow<'a, str>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tag<'a> {
    /// `\maketitle`: contains `Title`, `Author`, and `Date` in that order.
    TitleBlock,
    Title,
    Author,
    Date,
    Abstract,
    /// `level`: 0 part, 1 chapter, 2 section, 3 subsection, 4 subsubsection,
    /// 5 paragraph, 6 subparagraph.
    Heading {
        level: u8,
        number: Option<String>,
    },
    Paragraph {
        indent: bool,
    },
    List(ListKind),
    /// `label` is the generated label of an enumerated item, such as `1.` or
    /// `(a)`. An explicit `\item[...]` label follows as `ItemLabel` instead.
    Item {
        label: Option<String>,
    },
    ItemLabel,
    Quote(QuoteKind),
    Align(Align),
    Code {
        language: Option<Cow<'a, str>>,
    },
    InlineCode,
    Theorem {
        name: String,
        number: Option<String>,
        style: TheoremStyle,
    },
    /// The optional note of a theorem or proof, such as `[Rayleigh]`.
    TheoremNote,
    Proof,
    Figure(FigureKind),
    Caption {
        number: Option<String>,
        kind: FigureKind,
    },
    Table {
        columns: Arc<[Column]>,
    },
    Row,
    Cell {
        span: u16,
        align: Align,
    },
    Style(Style),
    Link {
        url: Cow<'a, str>,
    },
    Footnote {
        number: Option<String>,
    },
    Bibliography,
    BibItem {
        label: String,
    },
    /// An unknown command or environment. Its name and delimiters follow as
    /// `Raw` events, around any parsed content.
    Unsupported {
        name: Cow<'a, str>,
        block: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagKind {
    TitleBlock,
    Title,
    Author,
    Date,
    Abstract,
    Heading,
    Paragraph,
    List,
    Item,
    ItemLabel,
    Quote,
    Align,
    Code,
    InlineCode,
    Theorem,
    TheoremNote,
    Proof,
    Figure,
    Caption,
    Table,
    Row,
    Cell,
    Style,
    Link,
    Footnote,
    Bibliography,
    BibItem,
    Unsupported,
}

impl Tag<'_> {
    pub fn kind(&self) -> TagKind {
        match self {
            Tag::TitleBlock => TagKind::TitleBlock,
            Tag::Title => TagKind::Title,
            Tag::Author => TagKind::Author,
            Tag::Date => TagKind::Date,
            Tag::Abstract => TagKind::Abstract,
            Tag::Heading { .. } => TagKind::Heading,
            Tag::Paragraph { .. } => TagKind::Paragraph,
            Tag::List(_) => TagKind::List,
            Tag::Item { .. } => TagKind::Item,
            Tag::ItemLabel => TagKind::ItemLabel,
            Tag::Quote(_) => TagKind::Quote,
            Tag::Align(_) => TagKind::Align,
            Tag::Code { .. } => TagKind::Code,
            Tag::InlineCode => TagKind::InlineCode,
            Tag::Theorem { .. } => TagKind::Theorem,
            Tag::TheoremNote => TagKind::TheoremNote,
            Tag::Proof => TagKind::Proof,
            Tag::Figure(_) => TagKind::Figure,
            Tag::Caption { .. } => TagKind::Caption,
            Tag::Table { .. } => TagKind::Table,
            Tag::Row => TagKind::Row,
            Tag::Cell { .. } => TagKind::Cell,
            Tag::Style(_) => TagKind::Style,
            Tag::Link { .. } => TagKind::Link,
            Tag::Footnote { .. } => TagKind::Footnote,
            Tag::Bibliography => TagKind::Bibliography,
            Tag::BibItem { .. } => TagKind::BibItem,
            Tag::Unsupported { .. } => TagKind::Unsupported,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListKind {
    Itemize,
    Enumerate,
    Description,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuoteKind {
    Quote,
    Quotation,
    Verse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TheoremStyle {
    /// Bold head, italic body.
    #[default]
    Plain,
    /// Bold head, upright body.
    Definition,
    /// Italic head, upright body.
    Remark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FigureKind {
    Figure,
    Table,
    Algorithm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Emphasis,
    Italic,
    Slanted,
    Bold,
    Monospace,
    SmallCaps,
    SansSerif,
    Underline,
    Strike,
    Superscript,
    Subscript,
    /// Relative size: `\tiny` is -4, `\normalsize` 0, `\Huge` 5.
    Size(i8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub align: Align,
    /// The width of a paragraph column, such as `3cm` in `p{3cm}`.
    pub width: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hint {
    Align(Align),
    VerticalSpace,
    PageBreak,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Math<'a> {
    pub display: bool,
    /// TeX ready for KaTeX: labels removed, references resolved, unsupported
    /// environments rewritten, and an explicit `\tag` on each numbered row.
    pub prepared: Cow<'a, str>,
    /// The macro bindings visible where the formula starts.
    pub context: MacroContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefKind {
    Ref,
    Eqref,
    Page,
    /// `\autoref` and `\cref`: a name precedes the number.
    Named,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reference<'a> {
    pub kind: RefKind,
    pub label: Cow<'a, str>,
    /// The number or tag the label resolves to.
    pub resolved: Option<String>,
}

impl Reference<'_> {
    /// Text to show: the number, in parentheses for `\eqref`, or the label in
    /// brackets when unresolved. Page numbers do not exist in a preview.
    pub fn display(&self) -> String {
        match (&self.resolved, self.kind) {
            (Some(number), RefKind::Eqref) => format!("({number})"),
            (Some(number), RefKind::Ref | RefKind::Named) => number.clone(),
            _ => format!("[{}]", self.label),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Citation<'a> {
    pub keys: Vec<Cow<'a, str>>,
    /// The bibliography label of each key, when the document defines it.
    pub resolved: Vec<Option<String>>,
    pub note: Option<String>,
}

impl Citation<'_> {
    /// Text to show, such as `[1, 2]` or `[key]`.
    pub fn display(&self) -> String {
        let items: Vec<&str> = self
            .keys
            .iter()
            .zip(&self.resolved)
            .map(|(key, label)| label.as_deref().unwrap_or(key))
            .collect();
        match &self.note {
            Some(note) => format!("[{}, {note}]", items.join(", ")),
            None => format!("[{}]", items.join(", ")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image<'a> {
    pub path: Cow<'a, str>,
    pub options: Option<Cow<'a, str>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IncludeKind {
    /// `\input`, `\subfile`, and `\import`.
    Input,
    Include,
    Bibliography,
    /// A file shown as code, such as `\lstinputlisting`.
    Listing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Include<'a> {
    pub kind: IncludeKind,
    pub path: Cow<'a, str>,
}
