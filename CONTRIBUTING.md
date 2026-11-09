# Contributing

Issues and pull requests are welcome. [docs/design.md](docs/design.md) explains
the reading model, the scoping and numbering rules, and where each part lives.

## Principles

- **Reading, not rendering.** The crate reads LaTeX and reports events,
  contexts, numbers, and diagnostics. Layout, caches, file access, and revision
  publication belong to applications; files arrive through `expand`'s callback.
- **Nothing disappears silently.** Unsupported input stays visible as `Raw`
  source and is diagnosed. When a limit or an error affects later state, that
  state is marked uncertain rather than left stale.
- **Positions are exact.** Every event keeps its source range, and reading from
  a checkpoint gives the same events as reading from the start.
- **Work is bounded.** New features need explicit limits, poll cancellation in
  long loops, and must not make memory or time grow faster than the input.
- **Behavior matches LaTeX where it is modeled.** When a rule is uncertain,
  check it with pdfLaTeX, and prefer an unresolved result with a diagnostic to a
  guess.
- **The README describes what is implemented.** Planned features stay out of it.

Keep everything in the repository suitable for publication: generic examples,
relative paths, and no personal or machine-specific details. Large inputs,
generated output, and local measurements stay outside the repository.

## Development

Rust 1.90 or newer; the crate has no dependencies.

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo +1.90.0 check --all-targets --locked
```

`tests/behavior.rs` covers the public behavior: scoping, numbering, labels,
fallbacks, included files, windows from every checkpoint, the checkpoint cap,
and cancellation. The README's examples run as documentation tests. Inspect
the events for a file with `cargo run --example dump -- file.tex`.

To support a new command or environment, classify it in `src/commands.rs` (see
[Classification and fallbacks](docs/design.md#classification-and-fallbacks)),
handle it in the parser if it needs more than its class provides, and add a test
that shows its events and that no diagnostic remains.

### Checking behavior against LaTeX

`tests/fixtures/scoping.tex` records how pdfLaTeX scopes definitions and places
labels; its comments state the expected results, and the behavior tests assert
the same cases. When changing these rules, compile it outside the repository and
compare:

```sh
pdflatex -interaction=nonstopmode -no-shell-escape scoping.tex
```

## Benchmarks

`bench/corpus.py` downloads the sources of two public arXiv papers,
[1706.03762v7](https://arxiv.org/abs/1706.03762v7) and
[2005.14165v4](https://arxiv.org/abs/2005.14165v4), verifies their checksums, and
writes three inputs to a directory outside the repository: `body.tex` (all their
`.tex` files joined, 374,049 bytes), `body-x100.tex` (that text repeated 100
times), and `tricky.tex` (verbatim, `\verb`, and a theorem).

```sh
python3 -I bench/corpus.py <corpus-directory>
cargo run --release --example throughput -- <corpus-directory>/body.tex
```

On an Apple M2 Pro with Rust 1.99, medians of seven runs:

| Input | First pass | Events | Checkpoints | Peak RSS |
| --- | ---: | ---: | ---: | ---: |
| 374 KB | 3.0 ms (125 MB/s) | 3.9 ms (96 MB/s) | 12 | — |
| 37 MB | 236 ms (159 MB/s) | 305 ms (123 MB/s) | 1,134 | 41 MiB |

Peak RSS includes the 36 MiB input string.

### Comparing with other parsers

`bench/compare` is a standalone Cargo package that times tree-sitter-latex and
MiTeX's parser on the same corpus, dumps their trees, and checks which display
constructs KaTeX (through katex-native) renders:

```sh
cargo build --release --manifest-path bench/compare/Cargo.toml
python3 -I bench/compare/run.py <corpus-directory>
bench/compare/target/release/latex-parser-probe structure <corpus-directory>/body.tex
bench/compare/target/release/latex-parser-probe katex
```

Same machine, syntax trees only, parse time excluding file reading:

| Parser | 374 KB | 37 MB | Peak RSS, 37 MB |
| --- | ---: | ---: | ---: |
| tree-sitter-latex (`codebook-tree-sitter-latex` 0.6.1) | 36.1 ms | 3.54 s | 1,273 MiB |
| MiTeX (`mitex-parser` 0.2.4) | 4.9 ms | 366 ms | 247 MiB |

On `tricky.tex`, MiTeX tokenizes verbatim content, so a `{` inside it swallows
the rest of the file, and its macro expansion shifts offsets;
tree-sitter-latex keeps exact offsets but reports `\verb` as an error.

## Using a local checkout

To build an application against a local checkout, add an override to an
untracked `.cargo/config.toml` in that application, adapting the path:

```toml
[patch.crates-io]
latex-reader = { path = "../latex-reader" }
```

## Releasing

The [release workflow](.github/workflows/release.yml) publishes version tags
through crates.io's Trusted Publishing, for this workflow and the `release`
environment; the repository stores no registry token.

1. Set the new version in `Cargo.toml`, run `cargo check` so `Cargo.lock`
   follows, update the README if the API changed, commit, and push `main`.
2. Tag that commit and push the tag:

   ```sh
   git tag -a vX.Y.Z -m release
   git push origin vX.Y.Z
   ```

The workflow checks that the tag matches `Cargo.toml` and is on `main`, runs the
tests and a dry-run publish, then publishes. A version already on crates.io is
skipped, so re-running is safe. Trusted Publishing can only be configured for a
crate that exists, so 0.1.0 is published by hand with `cargo publish`.

Before 1.0, a release that adds an event, tag, or enum variant is a breaking
change and bumps the minor version.
