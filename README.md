# latex-preview

Fast, approximate reading of LaTeX documents for previews. The crate reads a
document once and reports semantic events with source ranges: headings,
paragraphs, lists, theorems, tables, figures, footnotes, references, code, and
formulas prepared for [KaTeX](https://katex.org). It does not execute TeX,
render, or read files. Applications own layout, caches, file access, and
revision publication.

Status: early. The API is not stable and the crate is not published.

```rust
use std::sync::atomic::AtomicBool;
use latex_preview::{Event, Options};

let source = r"\section{Intro}\label{s} See \ref{s} and $x \in \mathbb{R}$.";
let index = latex_preview::index(source, &Options::default(), &AtomicBool::new(false))?;
for spanned in index.events(source) {
    match spanned.event {
        Event::Math(math) => { /* math.prepared, math.context */ }
        _ => {}
    }
}
# Ok::<(), latex_preview::Cancelled>(())
```

## Reading model

`index` is the first pass. It records macro bindings, labels, the outline,
diagnostics, and checkpoints, and polls a cancellation flag every 64 KiB.
`Index::events` reads the whole document; `Index::window` reads from one
checkpoint to the next. Concatenating every window gives exactly the events of
`Index::events`, so a viewer can parse only what it shows. `Checkpoint::open_tags`
lists the tags a window's events continue inside. An index records the host's
revision; windows require the same source.

Checkpoints are placed at the first safe point after every 32 KiB: a paragraph
break at any environment depth, before an `\item`, after a table row, or
between verbatim lines. Spacing is fixed. Past `max_checkpoints`, indexing stops
and the rest of the document is reported as `Raw` source, in chunks of at most
64 KiB, with a diagnostic.

## Formulas

Each `Math` event carries TeX prepared for KaTeX and a `MacroContext`.

- Labels are removed, `\ref` and `\eqref` inside formulas are resolved, and
  `multline`, `eqnarray`, and `flalign` are rewritten to environments KaTeX
  supports. Numbered environments become starred ones with an explicit
  `\tag*{(n)}` on each numbered row, so KaTeX's own numbering never applies.
- Numbering follows amsmath: one number per row of `align`, `gather`,
  `alignat`, `flalign`, and `eqnarray`, except rows with `\notag`; one number for
  `equation` and `multline`; explicit `\tag` without a counter step;
  `subequations`; `\numberwithin`; and `\setcounter`. A label on an unnumbered
  row binds to the next numbered row of the same environment, or stays
  undefined. A label in `equation*` binds to the current label, such as the
  section. Several labels pending for one row, and labels on unnumbered
  `eqnarray` rows, are diagnosed and left undefined.
- `MacroContext` holds the bindings visible where the formula starts. Scoping
  follows TeX's save stack: groups, environments, and formulas are groups;
  `\newcommand`, `\def`, and `\let` are local; `\gdef` and `\global` survive the
  group. A redefinition changes only later formulas. `MacroContext::hash`
  identifies the visible bindings by content, for cache keys. Build a KaTeX
  macro map from `MacroContext::bindings` once per context, then render each
  formula with KaTeX's `globalGroup` off on a copy of that map.
- A definition that cannot be applied, because it is malformed or over the
  definition budget, makes later contexts incomplete until its group ends, or
  to the end of the document when global or over budget. Show those formulas as
  source.

## Text and structure

Sectioning commands are numbered by document class, including chapters,
`\appendix`, and `secnumdepth`. Theorems follow `\newtheorem`, shared counters,
`[within]` counters, and `\theoremstyle`; common theorem names that a class
predefines render with their name but no number. Lists number like LaTeX.
`thebibliography` labels resolve `\cite`. Text follows TeX's rules for
comments, blank lines, and spaces after control words, with ligatures, accents,
and escapes decoded. User macros expand in text, with optional arguments;
`\newenvironment` definitions expand around their body.

`verbatim`, `lstlisting` (with `language=`), `minted`, and `\verb` keep their
content literal. `comment` environments and `\iffalse` blocks are hidden.

## Included files

`expand` replaces `\input`, `\include` (honoring `\includeonly`), `\subfile`,
and the `import` package's commands with the text of the files they name, before
indexing, so included text shares macros, counters, and labels. `\endinput`
stops a file after its line. `\lstinputlisting`, `\VerbatimInput`,
`\verbatiminput`, and `\inputminted` become code environments, with
`firstline`, `lastline`, and `linerange`. `\bibliography` reads the main file's
`.bbl` when one exists. Comments and verbatim content are skipped.

The application reads files through a callback and keeps its own path policy.
Paths are relative to the main file's directory, as in LaTeX; names without an
extension try `.tex` first. File names may use macros whose bodies are plain
text, and `\IfFileExists` selects the branch whose definitions apply. Missing
files, cycles, nesting deeper than 15 files, and expansion beyond 256 reads or
64 MiB leave the command in place; the index reports it as an `Include` event.
Event ranges then refer to the expanded text, which the application keeps
alongside the original source.

## Unknown input

Every command and environment belongs to an explicit class: mapped to events,
transparent wrappers that render their argument, commands with no visible
output, or user definitions. Anything else is unknown: an `Unsupported` tag
keeps the name and delimiters visible as `Raw` events around parsed arguments,
and a diagnostic records it. Environments with non-text content, such as
`tikzpicture`, become one `Raw` block. Alignment, spacing, and page breaks are
`Hint` events that a consumer may ignore.

## Limits

`Limits` bounds assignments (16,384), definition bytes (1 MiB), nesting (256),
expansion depth (32), expansion steps (1,000,000), expansion size (64 KiB),
formula size (64 KiB), labels (65,536), and positioned diagnostics (256).
Exceeding a limit keeps the region as source with a diagnostic. A formula over
the size limit makes later equation numbers uncertain, shown as `?`, until a
counter reset.

Not yet supported: spilling checkpoints to disk past a memory cap, splitting
paragraphs or formulas larger than the replay budget, BibTeX databases without
a `.bbl` file, and `\theequation` redefinitions.

## Performance

On the corpus from gloss's `bench/latex` harness (two arXiv papers, 374 KB, and
the same text repeated 100 times, 37 MB), a release build on an Apple M2 Pro
with Rust 1.99 measured, as medians of five or seven runs:

| Input | First pass | Events | Checkpoints | Peak RSS |
| --- | ---: | ---: | ---: | ---: |
| 374 KB | 3.0 ms (125 MB/s) | 3.9 ms (96 MB/s) | 12 | — |
| 37 MB | 236 ms (159 MB/s) | 305 ms (123 MB/s) | 1,134 | 41 MiB |

Peak RSS includes the 36 MiB input string. Run
`cargo run --release --example throughput -- <file>` to measure a document.

## License

MIT; see [LICENSE](LICENSE).
