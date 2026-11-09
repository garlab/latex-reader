# latex-reader

Reads LaTeX documents into a stream of semantic events with source ranges:
headings, paragraphs, lists, theorems, tables, figures, footnotes, references,
citations, code, and formulas prepared for [KaTeX](https://katex.org). It applies
the rules LaTeX would apply to number and reference the document, without
running a TeX engine:

- section, theorem, figure, and table numbers by document class, including
  chapters, `\appendix`, and `\numberwithin`;
- amsmath equation numbering, row by row, with `\notag`, `\tag`, and
  `subequations`, and labels placed where amsmath places them;
- `\ref`, `\eqref`, and `\cite` resolved against labels and the bibliography,
  including forward references;
- user macros with TeX's scoping: each formula comes with the definitions
  visible where it starts;
- files named by `\input`, `\include`, and listing commands, through a callback.

Reading never fails. Unknown commands and environments stay visible as source
and are reported as diagnostics. The crate has no dependencies, does not render,
and reads no files itself.

## Example

```rust
use std::sync::atomic::AtomicBool;
use latex_reader::{Event, Options, Tag};

let source = r"\section{Results}\label{s:res}
\newcommand{\R}{\mathbb{R}}
\begin{align}
  f(x) &= x^2 \label{eq:f} \\
  g(x) &= 2x
\end{align}
Section~\ref{s:res} shows \eqref{eq:f} for $x \in \R$.";

let index = latex_reader::index(source, &Options::default(), &AtomicBool::new(false))?;
for spanned in index.events(source) {
    // `spanned.range` is the event's byte range in `source`.
    match spanned.event {
        Event::Start(Tag::Heading { level, number }) => {
            assert_eq!((level, number.as_deref()), (2, Some("1")));
        }
        Event::Math(math) if math.display => {
            // Numbers are explicit, so KaTeX's own numbering never applies.
            assert!(math.prepared.contains(r"\tag*{(1)}"));
            assert!(math.prepared.contains(r"\tag*{(2)}"));
        }
        Event::Reference(reference) => println!("{}", reference.display()), // 1, then (1)
        _ => {}
    }
}
assert_eq!(index.label("eq:f"), Some("1"));
# Ok::<(), latex_reader::Cancelled>(())
```

`examples/dump.rs` prints the events, outline, and diagnostics of a file:
`cargo run --example dump -- paper.tex`.

## Formulas

Each `Math` event carries `prepared` TeX and a `MacroContext`. Labels are
removed, references inside formulas are resolved, and `multline`, `eqnarray`,
and `flalign` are rewritten to environments KaTeX supports. Numbered
environments become starred ones with an explicit `\tag*{(n)}` on each numbered
row.

`MacroContext::bindings` lists the definitions visible at the formula, and
`MacroContext::hash` identifies them by content, so it can key caches. With
katex-rs, build one macro map per context: insert `Binding::simple_body` where
there is one, and otherwise parse `Binding::katex_definition` once with
`globalGroup` on. Then render each formula with `globalGroup` off on a copy of
that map, so definitions inside a formula stay local to it. A context is
incomplete when a definition before it could not be applied, because it was
malformed or over a limit; show those formulas as source rather than with a
stale definition.

## Included files

`expand` replaces `\input`, `\include` (with `\includeonly`), `\subfile`, and the
`import` package's commands with the files they name before indexing, so they
share macros, counters, and labels. `\lstinputlisting`, `\VerbatimInput`, and
`\inputminted` become code, with `firstline` and `lastline`, and `\bibliography`
reads the main file's `.bbl` when BibTeX has produced one. Event ranges then
refer to the expanded text; keep the original source for exact copies.

```rust
use std::{collections::HashMap, sync::atomic::AtomicBool};
use latex_reader::{InputKind, InputLimits, Options};

let files = HashMap::from([("intro.tex", r"\section{Introduction}")]);
// Paths are relative to the main file's directory; the callback owns file access.
let mut read = |path: &str, _kind: InputKind| files.get(path).map(|text| text.to_string());
let source = r"\input{intro} More text.";
let expanded = latex_reader::expand(source, Some("main"), &InputLimits::default(), &mut read);
assert_eq!(expanded.files, ["intro.tex"]);
let index = latex_reader::index(&expanded.text, &Options::default(), &AtomicBool::new(false))?;
assert_eq!(index.outline()[0].title, "Introduction");
# Ok::<(), latex_reader::Cancelled>(())
```

Missing files, cycles, and expansion over `InputLimits` leave the command in
place, reported as an `Include` event.

## Large documents and interactive use

`index` reads the document once, recording labels, numbers, macro bindings, the
outline, diagnostics, and checkpoints about every 32 KiB. `Index::window` reads
from one checkpoint to the next, and concatenating all windows gives exactly the
events of `Index::events`, so a viewer can read only what it shows. Both passes
poll a cancellation flag, and an index records a host-supplied revision.

On two public arXiv papers (374 KB) and the same text repeated 100 times
(37 MB), a release build on an Apple M2 Pro ran the first pass at 125–159 MB/s,
with a peak of 41 MiB for the 37 MB input, including the input string itself.

## Scope

Supported: the title block and abstract; sectioning; text styles, sizes, and
alignment hints; lists; quotes; `verbatim`, `lstlisting`, `minted`, and `\verb`;
`tabular` with column alignment, `\multicolumn`, and rules; figures and captions;
theorems from `\newtheorem` and `\theoremstyle`; proofs; footnotes; links;
`thebibliography`; user commands and environments with optional arguments; and
inline and display math. Text follows TeX's rules for comments, spaces, and
paragraphs, with ligatures, accents, and escapes decoded.

Not supported: running TeX or loading packages and classes, line and page
breaking, TikZ and other drawing environments (kept as source), BibTeX
databases without a `.bbl` file, and redefined `\theequation`. There are no
pages, so `\pageref` shows its key in brackets.

`Limits` bounds definitions, nesting, expansion, formula size, labels, and
diagnostics. Exceeding a limit keeps the region as source with a diagnostic, and
state that depends on it, such as later equation numbers or macro contexts, is
marked uncertain rather than silently wrong.

## Alternatives

| Crate or tool | What it gives you |
| --- | --- |
| [tree-sitter-latex](https://github.com/latex-lsp/tree-sitter-latex), [MiTeX](https://github.com/mitex-rs/mitex)'s parser | A syntax tree; numbering, references, and macro scoping are left to you. On the corpus above, tree-sitter-latex parsed at 10.6 MB/s and needed 1.3 GB for 37 MB. MiTeX expands macros before parsing, which shifts source offsets. |
| [pulldown-latex](https://crates.io/crates/pulldown-latex), [tylax](https://github.com/scipenai/tylax) | Formulas converted to MathML, or documents to Typst text, without source ranges. |
| [Tectonic](https://github.com/tectonic-typesetting/tectonic) | A TeX engine: exact output, but it needs TeX resources and typesets every page. |
| [Pandoc](https://pandoc.org)'s LaTeX reader | The closest in purpose: a document model with numbering and macros, in Haskell, for whole documents, without source positions. |

latex-reader trades exactness for speed and robustness: it approximates what
LaTeX computes, keeps every source position, and leaves layout to you.

## Requirements and license

Rust 1.90 or newer. MIT; see [LICENSE](LICENSE). To work on the crate, see
[CONTRIBUTING.md](CONTRIBUTING.md).
