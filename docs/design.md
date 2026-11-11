# Design

How latex-reader models a LaTeX document, and the rules it follows. The
[README](../README.md) describes the API and scope; this document is for
changing the crate.

## Modules

| Module | Responsibility |
| --- | --- |
| `lexer` | Tokens with byte ranges under TeX's default category codes: comments, blank lines, spaces after control words, `\makeatletter`. |
| `inputs` | `expand`: splices included files into the text before indexing. |
| `commands` | The explicit classification of every known command and environment, and their argument shapes. |
| `parser` | One engine for both passes: the first pass (building the index) and reading events from a checkpoint. |
| `index` | `Index`, checkpoints, windows, the outline, labels, and the `Events` iterator. |
| `macros` | The binding store with TeX's save stack, and `MacroContext`. |
| `numbering` | Counters, `\numberwithin`, theorem counters, and number formats. |
| `math` | Formula analysis (rows, `\notag`, `\tag`, labels, global definitions) and preparation for KaTeX. |
| `text` | Ligatures, accents, roman and alphabetic numbering. |
| `diagnostics` | Bounded diagnostics: the first ones with positions, then counts per name. |

## Reading model

`index` is a linear scan. It records the binding store, labels and their
numbers, the bibliography, the outline, diagnostics, and checkpoints. Events
come from a second pass over the same source with the finished index, from the
start (`Index::events`) or from any checkpoint (`Index::window`). Because both
passes run the same parser, forward references resolve, and a window produces
exactly the events of the corresponding part of a sequential read. The tests
check that equivalence for every checkpoint of a document with checkpoints
inside lists, verbatim blocks, and tables.

A checkpoint holds the source offset and the complete parser state: lexer
state, the stack of open groups and environments (with list counters and table
columns), counters, uncertain counters, the current label, paragraph state, and
any open verbatim block. Macro bindings are not copied: contexts are looked up by
source offset in the shared binding store.

Checkpoints are placed at the first safe boundary after every
`Options::checkpoint_spacing` bytes (32 KiB): a paragraph break at any
environment depth, before an `\item`, after a table row, or between verbatim
lines. Spacing never grows with the document, so long environments still get
checkpoints. Past `Options::max_checkpoints`, indexing stops; the rest of the
document is reported as `Raw` source in chunks of at most 64 KiB, with a
diagnostic.

Both passes poll the cancellation flag every 64 KiB of input and return
`Cancelled` without partial state. `Options::revision` is stored in the index so
hosts can discard results for an outdated source.

## Included files

`expand` runs before indexing and returns one text, so included files share the
document's macros, counters, and labels, and checkpoints, contexts, and ranges
need no per-file offsets. It scans with the crate's lexer, skipping comments and
the bodies of verbatim, hidden, and raw environments and `\verb`. File names may
use macros whose bodies are plain text, recorded from `\newcommand` and `\def`
as the scan goes; `\IfFileExists` is evaluated through the callback, and only
the taken branch's definitions apply. A file's last line end stands for the
rest of the including line, so a paragraph continues across files as in TeX.
`\include` adds the `\clearpage` around the file that LaTeX adds, which ends the
paragraph.

## Classification and fallbacks

Every command and environment falls into one explicit class in `commands`:

1. **Mapped** to events and hints. Alignment commands and environments, spacing,
   and page breaks become `Hint` events that consumers may ignore; they are
   never discarded.
2. **Transparent**: the last mandatory argument renders without a marker, for
   wrappers with no visible meaning such as `\mbox` or `\textnormal`.
3. **Invisible**: no output in any consumer, such as `\label` (recorded),
   `\index`, `\usepackage`, or `\geometry`. Arguments are consumed.
4. **User definitions** from `\newcommand`, `\def`, `\let`,
   `\DeclareMathOperator`, and `\newenvironment`, expanded in text with their
   arguments and optional defaults.
5. **Unknown**: everything else. An unknown command becomes an `Unsupported` tag
   whose name and argument delimiters stay visible as `Raw` events around the
   parsed arguments. An unknown environment keeps its `\begin` and `\end` lines
   around a parsed body. Environments with non-text content, such as
   `tikzpicture`, become one `Raw` block. Each adds a diagnostic.

Adding support means moving a name from the unknown class to another one, with
a test.

## Macro scoping

The store follows TeX's save stack. Groups are braces, `\begingroup` and
`\bgroup` pairs, every environment, and every formula. Local assignments
(`\newcommand`, `\def`, `\let`, `\DeclareMathOperator`, `\newenvironment`)
record the name's previous binding once per group; global assignments (`\gdef`,
`\xdef`, `\global`) mark the binding global, and leaving a group restores saved
bindings except global ones. The preamble has no special scope: a definition
inside an explicit preamble group is local to it. Counters are global, as in
LaTeX.

Every assignment is recorded with its source offset, so a `MacroContext` for
any offset is a lookup, and earlier contexts never change: after
`\def\x{a}{\gdef\x{b}}`, content before the group sees `a` and content after it
sees `b`. A context's hash is the wrapping sum of a 128-bit hash of each visible
binding's name and definition, so identical bindings give identical hashes
however they were reached.

`\gdef` and `\global` inside a formula apply to later content, as in KaTeX; other
definitions inside a formula stay local to it.

A definition that cannot be applied, because it is malformed or exceeds
`Limits::max_assignments` or `max_definition_bytes`, makes its name unknown
rather than leaving the previous value visible. Contexts are incomplete until
the group that made the failed assignment ends, or to the end of the document
for global or over-budget failures.

## Numbering

The crate assigns every equation number and writes it into the prepared TeX,
so KaTeX's numbering is never used, and rewrites cannot change numbers. The
rules follow the [amsmath manual](https://www.latex-project.org/help/documentation/amsldoc.pdf):

| Construct | Numbering |
| --- | --- |
| `equation` | One number, unless the body has `\notag` or `\nonumber`. `\tag{x}` replaces it without advancing the counter. |
| `equation*`, `displaymath`, `\[...\]`, `$$...$$` | No number; `\tag` still displays. |
| `split`, `aligned`, `gathered` inside `equation` | One number for the whole. |
| `align`, `gather`, `alignat`, `flalign`, `eqnarray` | One number per row, except rows with `\notag`; `\tag` replaces that row's number. |
| Starred variants | No numbers; rows may carry `\tag`. |
| `multline` | One number for the whole. |
| `subequations` | 1a, 1b, …; a label on the outer environment refers to 1. |
| `\label` on a numbered or tagged row | That number or tag. |
| `\label` on an unnumbered amsmath row | The next numbered or tagged row of the same environment; undefined if there is none. |
| `\label` in an unnumbered environment such as `equation*` | The current label, such as the section. The current label is scoped like a macro, so numbers made inside an environment do not outlast it. |
| `\setcounter`, `\addtocounter`, `\numberwithin` | Applied in source order. |

Several labels pending for one row, and labels on unnumbered `eqnarray` rows,
are left undefined with a diagnostic rather than guessed.
`tests/fixtures/scoping.tex` records the label placement and scoping cases as
checked with pdfLaTeX (TeX Live 2024, LaTeX 2023-11-01, amsmath 2.17o); its
comments state the expected results.

Rewrites for KaTeX happen after numbering: numbered environments become their
starred forms with `\tag*{(n)}` per numbered row; `multline` becomes `gathered`
inside `equation*`; `eqnarray` and `flalign` become `align*`, with `&op&`
reduced to `&op`.

## Limits and failure propagation

`Limits` bounds assignments, definition bytes, nesting, expansion depth, steps,
and size, formula size, labels, and positioned diagnostics. Exceeding a limit
keeps the region as source with a diagnostic, and state derived from it is not
silently trusted: a formula over the size limit makes later equation numbers
uncertain, shown as `?`, until a counter reset; a failed definition makes
contexts incomplete; a label that was not recorded leaves its references
unresolved, shown as the key in brackets.

## Not implemented

- Storing checkpoint records outside memory past a resident cap. Indexing stops
  at `max_checkpoints` instead.
- A replay budget: a single paragraph, formula, or group larger than the
  checkpoint spacing is read whole by the window that contains it.
- Resuming the first pass after an edit from the last unaffected checkpoint.
- Redefined `\theequation`, BibTeX databases without a `.bbl` file, and package
  coverage beyond the command tables.

Input expansion also returns a compact root-source map. Unchanged spans map
byte for byte; inserted files, including nested inputs, map to the invoking root
command. The number of root replacements is bounded by `InputLimits::max_files`.
`expand_cancellable` checks between tokens and before file callbacks; the host
is responsible for bounding and cancelling work inside its callback.
