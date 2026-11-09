//! Known commands and environments. Each belongs to one explicit class;
//! anything absent here is unknown, kept visible as source, and diagnosed.
//!
//! Argument patterns use one letter per argument: `s` an optional star, `o` an
//! optional bracket argument, and `m` a mandatory argument.

use crate::event::{Align, FigureKind, Hint, IncludeKind, ListKind, QuoteKind, RefKind, Style};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// A style applied to its last mandatory argument, such as `\textbf`.
    Style(Style),
    /// A style applied to the rest of the group, such as `\bfseries`.
    Declare(Style),
    /// The last mandatory argument renders without a marker.
    Transparent,
    /// Produces nothing visible; arguments are consumed.
    Invisible,
    Symbol(&'static str),
    Accent,
    Section(u8),
    Hint(Hint),
    LineBreak,
    Par,
    NoIndent,
    Special(Special),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Special {
    Begin,
    End,
    Item,
    Label,
    Ref(RefKind),
    Cite,
    Footnote,
    Href,
    Url,
    IncludeGraphics,
    GraphicsPath,
    Caption,
    MakeTitle,
    Title,
    Author,
    Date,
    And,
    Verb,
    LstInline,
    MintInline,
    Include(IncludeKind),
    BibItem,
    NewTheorem,
    TheoremStyle,
    SetCounter,
    AddToCounter,
    StepCounter,
    NewCounter,
    NumberWithin,
    Appendix,
    DocumentClass,
    UsePackage,
    MakeAtLetter,
    MakeAtOther,
    IfFalse,
    Multicolumn,
    Rule,
    NewCommand,
    RenewCommand,
    ProvideCommand,
    Def { global: bool },
    Let { global: bool },
    Global,
    Prefix,
    DeclareMathOperator,
    NewEnvironment,
    EnsureMath,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Spec {
    pub cmd: Cmd,
    pub args: &'static str,
}

const fn spec(cmd: Cmd, args: &'static str) -> Option<Spec> {
    Some(Spec { cmd, args })
}

pub(crate) fn command(name: &str) -> Option<Spec> {
    use self::Special as S;
    use Cmd::*;
    match name {
        // Styles with an argument.
        "emph" => spec(Style(crate::Style::Emphasis), "m"),
        "textit" => spec(Style(crate::Style::Italic), "m"),
        "textsl" => spec(Style(crate::Style::Slanted), "m"),
        "textbf" => spec(Style(crate::Style::Bold), "m"),
        "texttt" => spec(Style(crate::Style::Monospace), "m"),
        "textsc" => spec(Style(crate::Style::SmallCaps), "m"),
        "textsf" => spec(Style(crate::Style::SansSerif), "m"),
        "underline" | "uline" => spec(Style(crate::Style::Underline), "m"),
        "sout" => spec(Style(crate::Style::Strike), "m"),
        "textsuperscript" => spec(Style(crate::Style::Superscript), "m"),
        "textsubscript" => spec(Style(crate::Style::Subscript), "m"),
        // Declarations for the rest of the group.
        "em" => spec(Declare(crate::Style::Emphasis), ""),
        "itshape" | "it" => spec(Declare(crate::Style::Italic), ""),
        "slshape" | "sl" => spec(Declare(crate::Style::Slanted), ""),
        "bfseries" | "bf" => spec(Declare(crate::Style::Bold), ""),
        "ttfamily" | "tt" => spec(Declare(crate::Style::Monospace), ""),
        "scshape" | "sc" => spec(Declare(crate::Style::SmallCaps), ""),
        "sffamily" | "sf" => spec(Declare(crate::Style::SansSerif), ""),
        "tiny" => spec(Declare(crate::Style::Size(-4)), ""),
        "scriptsize" => spec(Declare(crate::Style::Size(-3)), ""),
        "footnotesize" => spec(Declare(crate::Style::Size(-2)), ""),
        "small" => spec(Declare(crate::Style::Size(-1)), ""),
        "large" => spec(Declare(crate::Style::Size(1)), ""),
        "Large" => spec(Declare(crate::Style::Size(2)), ""),
        "LARGE" => spec(Declare(crate::Style::Size(3)), ""),
        "huge" => spec(Declare(crate::Style::Size(4)), ""),
        "Huge" => spec(Declare(crate::Style::Size(5)), ""),
        // Wrappers without a visible meaning of their own: the content renders.
        "textnormal" | "textrm" | "textup" | "textmd" | "mbox" | "hbox" | "fbox" | "text"
        | "textmu" | "ignorespaces" => spec(Transparent, "m"),
        "textcolor" | "colorbox" => spec(Transparent, "omm"),
        "fcolorbox" => spec(Transparent, "ommm"),
        "makebox" | "framebox" => spec(Transparent, "oom"),
        "shortstack" | "hyperref" => spec(Transparent, "om"),
        "multirow" => spec(Transparent, "momom"),
        "parbox" => spec(Transparent, "ooomm"),
        "raisebox" => spec(Transparent, "moom"),
        "resizebox" => spec(Transparent, "smmm"),
        "scalebox" => spec(Transparent, "mom"),
        "rotatebox" => spec(Transparent, "omm"),
        // No visible output in any consumer.
        "normalfont"
        | "rmfamily"
        | "upshape"
        | "mdseries"
        | "normalsize"
        | "protect"
        | "relax"
        | "phantomsection"
        | "nobreak"
        | "hfill"
        | "hfil"
        | "vfill"
        | "vfil"
        | "null"
        | "strut"
        | "sloppy"
        | "fussy"
        | "frenchspacing"
        | "nonfrenchspacing"
        | "tableofcontents"
        | "listoffigures"
        | "listoftables"
        | "fi"
        | "else"
        | "selectfont"
        | "onecolumn"
        | "twocolumn"
        | "balance"
        | "flushbottom"
        | "raggedbottom"
        | "allowbreak"
        | "break"
        | "noalign"
        | "indent"
        | "nolinebreak"
        | "nopagebreak"
        | "IEEEpeerreviewmaketitle"
        | "ignorespacesafterend"
        | "unskip"
        | "endinput"
        | "printbibliography" => spec(Invisible, ""),
        "index"
        | "glossary"
        | "pagestyle"
        | "thispagestyle"
        | "pagenumbering"
        | "bibliographystyle"
        | "hypersetup"
        | "geometry"
        | "color"
        | "pagecolor"
        | "usetikzlibrary"
        | "tikzset"
        | "pgfplotsset"
        | "lstset"
        | "captionsetup"
        | "setlist"
        | "keywords"
        | "DeclareGraphicsExtensions"
        | "addbibresource"
        | "linespread"
        | "hyphenation"
        | "nocite"
        | "enlargethispage"
        | "affil"
        | "affiliation"
        | "email"
        | "institute"
        | "address"
        | "orcid"
        | "markright"
        | "subjclass"
        | "ccsdesc"
        | "setcopyright"
        | "acmConference"
        | "acmYear"
        | "acmDOI"
        | "acmISBN"
        | "copyrightyear"
        | "acmBooktitle"
        | "acmPrice"
        | "titlerunning"
        | "authorrunning" => spec(Invisible, "som"),
        "setlength" | "addtolength" | "settowidth" | "markboth" => spec(Invisible, "mm"),
        "definecolor" => spec(Invisible, "mmm"),
        "newlength" | "addvspace" | "hspace" => spec(Invisible, "sm"),
        "fontsize" => spec(Invisible, "mm"),
        "addcontentsline" => spec(Invisible, "mmm"),
        "pdfbookmark" => spec(Invisible, "omm"),
        "phantom" | "hphantom" | "vphantom" => spec(Invisible, "m"),
        "rule" => spec(Invisible, "omm"),
        "PassOptionsToPackage" => spec(Invisible, "mm"),
        // Length registers, usually inside arguments that are read raw.
        "linewidth" | "textwidth" | "columnwidth" | "hsize" | "vsize" | "textheight"
        | "baselineskip" | "parindent" | "parskip" | "tabcolsep" | "arraystretch" | "fill"
        | "unitlength" | "paperwidth" | "paperheight" | "boldmath" | "unboldmath" | "pdfoutput"
        | "newblock" => spec(Invisible, ""),
        // Text symbols.
        "ldots" | "dots" | "textellipsis" => spec(Symbol("…"), ""),
        "LaTeX" => spec(Symbol("LaTeX"), ""),
        "LaTeXe" => spec(Symbol("LaTeX2ε"), ""),
        "TeX" => spec(Symbol("TeX"), ""),
        "BibTeX" => spec(Symbol("BibTeX"), ""),
        "textendash" => spec(Symbol("–"), ""),
        "textemdash" => spec(Symbol("—"), ""),
        "textbullet" => spec(Symbol("•"), ""),
        "textperiodcentered" => spec(Symbol("·"), ""),
        "textdegree" => spec(Symbol("°"), ""),
        "textbackslash" => spec(Symbol("\\"), ""),
        "textasciitilde" => spec(Symbol("~"), ""),
        "textasciicircum" => spec(Symbol("^"), ""),
        "textbar" => spec(Symbol("|"), ""),
        "textless" => spec(Symbol("<"), ""),
        "textgreater" => spec(Symbol(">"), ""),
        "textunderscore" => spec(Symbol("_"), ""),
        "textbraceleft" => spec(Symbol("{"), ""),
        "textbraceright" => spec(Symbol("}"), ""),
        "textdollar" => spec(Symbol("$"), ""),
        "textquoteleft" => spec(Symbol("‘"), ""),
        "textquoteright" => spec(Symbol("’"), ""),
        "textquotedblleft" => spec(Symbol("“"), ""),
        "textquotedblright" => spec(Symbol("”"), ""),
        "guillemotleft" | "guillemetleft" => spec(Symbol("«"), ""),
        "guillemotright" | "guillemetright" => spec(Symbol("»"), ""),
        "textsection" | "S" => spec(Symbol("§"), ""),
        "textparagraph" | "P" => spec(Symbol("¶"), ""),
        "copyright" | "textcopyright" => spec(Symbol("©"), ""),
        "textregistered" => spec(Symbol("®"), ""),
        "texttrademark" => spec(Symbol("™"), ""),
        "dag" | "textdagger" => spec(Symbol("†"), ""),
        "ddag" | "textdaggerdbl" => spec(Symbol("‡"), ""),
        "pounds" | "textsterling" => spec(Symbol("£"), ""),
        "euro" | "texteuro" => spec(Symbol("€"), ""),
        "textonehalf" => spec(Symbol("½"), ""),
        "textperthousand" => spec(Symbol("‰"), ""),
        "checkmark" => spec(Symbol("✓"), ""),
        "ss" => spec(Symbol("ß"), ""),
        "ae" => spec(Symbol("æ"), ""),
        "AE" => spec(Symbol("Æ"), ""),
        "oe" => spec(Symbol("œ"), ""),
        "OE" => spec(Symbol("Œ"), ""),
        "o" => spec(Symbol("ø"), ""),
        "O" => spec(Symbol("Ø"), ""),
        "aa" => spec(Symbol("å"), ""),
        "AA" => spec(Symbol("Å"), ""),
        "l" => spec(Symbol("ł"), ""),
        "L" => spec(Symbol("Ł"), ""),
        "i" => spec(Symbol("ı"), ""),
        "j" => spec(Symbol("ȷ"), ""),
        "quad" => spec(Symbol("\u{2003}"), ""),
        "qquad" => spec(Symbol("\u{2003}\u{2003}"), ""),
        "enspace" => spec(Symbol("\u{2002}"), ""),
        "thinspace" => spec(Symbol("\u{2009}"), ""),
        "space" => spec(Symbol(" "), ""),
        "slash" => spec(Symbol("/"), ""),
        "today" => spec(Symbol("today"), ""),
        "u" | "v" | "H" | "c" | "k" | "r" | "d" | "b" | "t" => spec(Accent, ""),
        // Structure.
        "part" => spec(Section(0), "som"),
        "chapter" => spec(Section(1), "som"),
        "section" => spec(Section(2), "som"),
        "subsection" => spec(Section(3), "som"),
        "subsubsection" => spec(Section(4), "som"),
        "paragraph" => spec(Section(5), "som"),
        "subparagraph" => spec(Section(6), "som"),
        "centering" => spec(Hint(crate::Hint::Align(Align::Center)), ""),
        "raggedright" => spec(Hint(crate::Hint::Align(Align::Left)), ""),
        "raggedleft" => spec(Hint(crate::Hint::Align(Align::Right)), ""),
        "smallskip" | "medskip" | "bigskip" => spec(Hint(crate::Hint::VerticalSpace), ""),
        "vspace" => spec(Hint(crate::Hint::VerticalSpace), "sm"),
        "newpage" | "clearpage" | "cleardoublepage" => spec(Hint(crate::Hint::PageBreak), ""),
        "pagebreak" => spec(Hint(crate::Hint::PageBreak), "o"),
        "newline" => spec(LineBreak, ""),
        "linebreak" => spec(LineBreak, "o"),
        "par" => spec(Par, ""),
        "noindent" => spec(NoIndent, ""),
        // Commands with their own handling.
        "begin" => spec(Special(S::Begin), ""),
        "end" => spec(Special(S::End), ""),
        "item" => spec(Special(S::Item), ""),
        "label" => spec(Special(S::Label), ""),
        "ref" | "Ref" | "vref" => spec(Special(S::Ref(RefKind::Ref)), ""),
        "eqref" => spec(Special(S::Ref(RefKind::Eqref)), ""),
        "pageref" => spec(Special(S::Ref(RefKind::Page)), ""),
        "autoref" | "cref" | "Cref" | "nameref" => spec(Special(S::Ref(RefKind::Named)), ""),
        "cite" | "citep" | "citet" | "citealp" | "citealt" | "citeauthor" | "citeyear"
        | "parencite" | "textcite" | "autocite" | "footcite" | "Cite" | "Citep" | "Citet" => {
            spec(Special(S::Cite), "")
        }
        "footnote" | "thanks" => spec(Special(S::Footnote), ""),
        // A mark for a footnote given elsewhere; the text is not repeated.
        "footnotemark" => spec(Invisible, "o"),
        "href" => spec(Special(S::Href), ""),
        "url" | "nolinkurl" => spec(Special(S::Url), ""),
        "includegraphics" => spec(Special(S::IncludeGraphics), ""),
        "graphicspath" => spec(Special(S::GraphicsPath), ""),
        "caption" | "captionof" => spec(Special(S::Caption), ""),
        "maketitle" => spec(Special(S::MakeTitle), ""),
        "title" => spec(Special(S::Title), ""),
        "author" => spec(Special(S::Author), ""),
        "date" => spec(Special(S::Date), ""),
        "and" | "AND" | "And" => spec(Special(S::And), ""),
        "verb" => spec(Special(S::Verb), ""),
        "lstinline" => spec(Special(S::LstInline), ""),
        "mintinline" => spec(Special(S::MintInline), ""),
        "input" | "subfile" | "import" | "subimport" | "inputfrom" | "subinputfrom"
        | "includefrom" | "subincludefrom" => spec(Special(S::Include(IncludeKind::Input)), ""),
        "lstinputlisting" | "VerbatimInput" | "BVerbatimInput" | "LVerbatimInput"
        | "verbatiminput" | "inputminted" => spec(Special(S::Include(IncludeKind::Listing)), ""),
        "include" => spec(Special(S::Include(IncludeKind::Include)), ""),
        "bibliography" => spec(Special(S::Include(IncludeKind::Bibliography)), ""),
        "bibitem" => spec(Special(S::BibItem), ""),
        "newtheorem" => spec(Special(S::NewTheorem), ""),
        "theoremstyle" => spec(Special(S::TheoremStyle), ""),
        "setcounter" => spec(Special(S::SetCounter), ""),
        "addtocounter" => spec(Special(S::AddToCounter), ""),
        "stepcounter" | "refstepcounter" => spec(Special(S::StepCounter), ""),
        "newcounter" => spec(Special(S::NewCounter), ""),
        "numberwithin" | "counterwithin" => spec(Special(S::NumberWithin), ""),
        "appendix" => spec(Special(S::Appendix), ""),
        "documentclass" | "documentstyle" => spec(Special(S::DocumentClass), ""),
        "usepackage" | "RequirePackage" => spec(Special(S::UsePackage), ""),
        "makeatletter" => spec(Special(S::MakeAtLetter), ""),
        "makeatother" => spec(Special(S::MakeAtOther), ""),
        "iffalse" => spec(Special(S::IfFalse), ""),
        "multicolumn" => spec(Special(S::Multicolumn), ""),
        "hline" | "toprule" | "midrule" | "bottomrule" | "cline" | "cmidrule" | "specialrule" => {
            spec(Special(S::Rule), "")
        }
        "newcommand" | "DeclareRobustCommand" => spec(Special(S::NewCommand), ""),
        "renewcommand" => spec(Special(S::RenewCommand), ""),
        "providecommand" => spec(Special(S::ProvideCommand), ""),
        "def" | "edef" => spec(Special(S::Def { global: false }), ""),
        "gdef" | "xdef" => spec(Special(S::Def { global: true }), ""),
        "let" => spec(Special(S::Let { global: false }), ""),
        "global" => spec(Special(S::Global), ""),
        "long" | "protected" | "outer" => spec(Special(S::Prefix), ""),
        "DeclareMathOperator" => spec(Special(S::DeclareMathOperator), ""),
        "newenvironment" | "renewenvironment" => spec(Special(S::NewEnvironment), ""),
        "ensuremath" => spec(Special(S::EnsureMath), ""),
        _ => None,
    }
}

/// Control symbols: `\` followed by one character.
pub(crate) fn symbol(c: &str) -> Option<&'static str> {
    Some(match c {
        "%" => "%",
        "$" => "$",
        "&" => "&",
        "#" => "#",
        "_" => "_",
        "{" => "{",
        "}" => "}",
        " " | "\n" | "\t" | "\r" => " ",
        "," => "\u{2009}",
        ";" | ":" | ">" => " ",
        "!" | "/" | "-" | "@" => "",
        "|" => "‖",
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MathEnv {
    Equation,
    Rows,
    Multline,
    Unnumbered,
    Inline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    None,
    /// `lstlisting`: `[language=...]`.
    Optional,
    /// `minted`: `{language}`.
    Mandatory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvClass {
    Document,
    List(ListKind),
    Quote(QuoteKind),
    Abstract,
    Align(Align),
    Verbatim(Language),
    /// Content that produces nothing, such as the `comment` environment.
    Hidden,
    Math(MathEnv),
    Proof,
    Float(FigureKind),
    Tabular,
    Bibliography,
    Subequations,
    /// The body renders without a marker.
    Transparent,
    /// Non-text content, such as TikZ: kept as one source block.
    RawBlock,
}

/// Known environments and the arguments that follow `\begin{name}`.
pub(crate) fn environment(name: &str) -> Option<(EnvClass, &'static str)> {
    use EnvClass::*;
    Some(match name {
        "document" => (Document, ""),
        "itemize" => (List(ListKind::Itemize), "o"),
        "enumerate" => (List(ListKind::Enumerate), "o"),
        "description" => (List(ListKind::Description), "o"),
        "quote" => (Quote(QuoteKind::Quote), ""),
        "quotation" => (Quote(QuoteKind::Quotation), ""),
        "verse" => (Quote(QuoteKind::Verse), ""),
        "abstract" => (Abstract, ""),
        "center" => (Align(crate::Align::Center), ""),
        "flushleft" => (Align(crate::Align::Left), ""),
        "flushright" => (Align(crate::Align::Right), ""),
        "verbatim" | "verbatim*" | "Verbatim" | "BVerbatim" | "alltt" => {
            (Verbatim(Language::None), "")
        }
        "lstlisting" => (Verbatim(Language::Optional), ""),
        "minted" => (Verbatim(Language::Mandatory), ""),
        "comment" => (Hidden, ""),
        "equation" => (Math(MathEnv::Equation), ""),
        "align" | "gather" | "flalign" | "eqnarray" => (Math(MathEnv::Rows), ""),
        "alignat" => (Math(MathEnv::Rows), ""),
        "multline" => (Math(MathEnv::Multline), ""),
        "equation*" | "align*" | "gather*" | "flalign*" | "eqnarray*" | "alignat*"
        | "multline*" | "displaymath" => (Math(MathEnv::Unnumbered), ""),
        "math" => (Math(MathEnv::Inline), ""),
        "proof" => (Proof, ""),
        "figure" | "figure*" | "SCfigure" => (Float(FigureKind::Figure), "o"),
        "wrapfigure" => (Float(FigureKind::Figure), "omom"),
        "table" | "table*" => (Float(FigureKind::Table), "o"),
        "algorithm" | "algorithm*" => (Float(FigureKind::Algorithm), "o"),
        "tabular" | "longtable" => (Tabular, "om"),
        "tabular*" | "tabularx" | "tabulary" => (Tabular, "mom"),
        "thebibliography" => (Bibliography, "m"),
        "subequations" => (Subequations, ""),
        "minipage" => (Transparent, "ooom"),
        "adjustwidth" | "adjustwidth*" => (Transparent, "mm"),
        "multicols" | "multicols*" => (Transparent, "mo"),
        "small" | "footnotesize" | "scriptsize" | "large" | "sloppypar" | "spacing"
        | "singlespace" | "onehalfspace" | "doublespace" | "adjustbox" | "titlepage" | "frame"
        | "landscape" | "samepage" | "IEEEkeywords" | "keywords" | "CCSXML" | "acks"
        | "appendices" => (Transparent, ""),
        "tikzpicture" | "pgfpicture" | "tikzcd" | "circuitikz" | "forest" | "pspicture"
        | "picture" | "asy" | "algorithmic" | "algorithmic*" | "algorithm2e" | "axis"
        | "filecontents" | "filecontents*" | "lrbox" => (RawBlock, ""),
        _ => return None,
    })
}

/// Names of theorem-like environments that classes often predefine. Without a
/// `\newtheorem`, they render with their name but no number.
pub(crate) fn predefined_theorem(name: &str) -> Option<&'static str> {
    Some(match name.trim_end_matches('*') {
        "theorem" => "Theorem",
        "lemma" => "Lemma",
        "proposition" => "Proposition",
        "corollary" => "Corollary",
        "definition" => "Definition",
        "remark" => "Remark",
        "example" => "Example",
        "conjecture" => "Conjecture",
        "claim" => "Claim",
        "fact" => "Fact",
        "observation" => "Observation",
        "assumption" => "Assumption",
        "hypothesis" => "Hypothesis",
        "problem" => "Problem",
        "exercise" => "Exercise",
        "notation" => "Notation",
        "note" => "Note",
        _ => return None,
    })
}
