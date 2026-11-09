use std::sync::atomic::AtomicBool;

use latex_preview::{DiagnosticKind, Event, Index, Limits, Options, Spanned, Tag, TagKind, index};

fn build(source: &str) -> Index {
    build_with(source, Options::default())
}

fn build_with(source: &str, options: Options) -> Index {
    index(source, &options, &AtomicBool::new(false)).expect("not cancelled")
}

fn events<'a>(index: &'a Index, source: &'a str) -> Vec<Spanned<'a>> {
    index.events(source).collect()
}

/// Text and references, flattened, for compact assertions.
fn text(source: &str) -> String {
    let index = build(source);
    let mut out = String::new();
    for spanned in index.events(source) {
        match spanned.event {
            Event::Text(text) => out.push_str(&text),
            Event::Reference(reference) => out.push_str(&reference.display()),
            Event::Citation(citation) => out.push_str(&citation.display()),
            Event::Raw(raw) => out.push_str(&raw),
            Event::Math(math) => {
                out.push('$');
                out.push_str(&math.prepared);
                out.push('$');
            }
            Event::End(TagKind::Paragraph) => out.push('\n'),
            _ => {}
        }
    }
    out
}

fn maths<'a>(index: &'a Index, source: &'a str) -> Vec<latex_preview::Math<'a>> {
    index
        .events(source)
        .filter_map(|s| match s.event {
            Event::Math(math) => Some(math),
            _ => None,
        })
        .collect()
}

#[test]
fn global_assignments_survive_groups_and_preamble_groups_stay_local() {
    // The cases in bench/latex/scoping.tex, checked with pdfLaTeX.
    let source = r"\documentclass{article}
{\newcommand{\grouped}{x}}
\def\x{a}{\gdef\x{b}}
\def\y{a}{\def\y{c}\gdef\y{b}}
\begin{document}
\x\y \grouped
\end{document}";
    let index = build(source);
    let out = text(source);
    assert!(out.starts_with("bb"), "{out}");
    assert!(
        out.contains("\\grouped"),
        "an undefined command stays visible: {out}"
    );
    assert!(
        index
            .diagnostics()
            .first()
            .iter()
            .any(|d| d.kind == DiagnosticKind::UnknownCommand
                && d.name.as_deref() == Some("grouped"))
    );
}

#[test]
fn formulas_see_the_bindings_visible_where_they_start() {
    let source = r"\newcommand{\v}{1}
$\v$ {\renewcommand{\v}{2} $\v$} $\v$ \renewcommand{\v}{3} $\v$";
    let index = build(source);
    let maths = maths(&index, source);
    let body = |i: usize| -> String {
        maths[i]
            .context
            .bindings()
            .find(|b| b.name == "\\v")
            .map(|b| b.body.clone())
            .unwrap()
    };
    assert_eq!(
        (0..4).map(body).collect::<Vec<_>>(),
        ["1", "2", "1", "3"],
        "a redefinition affects only later formulas, and a group's ends with it"
    );
    assert_eq!(maths[0].context.hash(), maths[2].context.hash());
    assert_ne!(maths[0].context.hash(), maths[1].context.hash());
}

#[test]
fn global_definitions_inside_formulas_apply_to_later_content() {
    let source = r"$\gdef\g{x} \def\l{y}$ $\g\l$";
    let index = build(source);
    let maths = maths(&index, source);
    let names: Vec<String> = maths[1]
        .context
        .bindings()
        .map(|b| b.name.clone())
        .collect();
    assert_eq!(
        names,
        ["\\g"],
        "a local definition stays inside its formula"
    );
}

#[test]
fn a_rejected_redefinition_makes_later_formulas_incomplete() {
    let options = Options {
        limits: Limits {
            max_assignments: 1,
            ..Limits::default()
        },
        ..Options::default()
    };
    let source = r"\newcommand{\x}{a} $\x$ \renewcommand{\x}{b} $\x$";
    let index = build_with(source, options);
    let maths = maths(&index, source);
    assert!(!maths[0].context.is_incomplete());
    assert!(maths[1].context.is_incomplete());
    assert!(
        index
            .diagnostics()
            .first()
            .iter()
            .any(|d| d.kind == DiagnosticKind::LimitExceeded)
    );
}

#[test]
fn a_malformed_definition_is_unknown_until_its_group_ends() {
    // `\DeclareMathOperator{\x}` without its text cannot be applied.
    let source = r"\newcommand{\x}{a} {\DeclareMathOperator{\x} $\x$} $\x$";
    let index = build(source);
    let found = maths(&index, source);
    assert!(found[0].context.is_incomplete());
    assert!(
        !found[1].context.is_incomplete(),
        "the local rejection ended with its group"
    );
    assert!(
        index
            .diagnostics()
            .first()
            .iter()
            .any(|d| d.kind == DiagnosticKind::MalformedDefinition)
    );
}

const NUMBERING: &str = r"\documentclass{article}
\usepackage{amsmath}
\begin{document}
\setcounter{section}{6}
\section{Seven}
\setcounter{equation}{40}
\begin{align}
a &= b \notag \label{pending} \\
c &= d
\end{align}
\begin{align}
e &= f \\
g &= h \notag \label{trailing}
\end{align}
\begin{equation*}
k = l \label{starred}
\end{equation*}
\begin{multline}
m \\ n \label{multline}
\end{multline}
\begin{equation}
o \tag{A} \label{tagged}
\end{equation}
\begin{gather*}
p \tag{B} \label{gathered}
\end{gather*}
\begin{subequations}\label{group}
\begin{align}
q \label{qa} \\ r
\end{align}
\end{subequations}
\end{document}";

#[test]
fn labels_follow_amsmath_placement() {
    let index = build(NUMBERING);
    assert_eq!(index.label("pending"), Some("41"));
    assert_eq!(index.label("trailing"), None);
    assert_eq!(index.label("starred"), Some("7"));
    assert_eq!(index.label("multline"), Some("43"));
    assert_eq!(index.label("tagged"), Some("A"));
    assert_eq!(index.label("gathered"), Some("B"));
    assert_eq!(index.label("group"), Some("44"));
    assert_eq!(index.label("qa"), Some("44a"));
}

#[test]
fn prepared_tex_numbers_rows_explicitly() {
    let index = build(NUMBERING);
    let maths = maths(&index, NUMBERING);
    assert_eq!(maths.len(), 7);
    assert!(maths[0].prepared.starts_with("\\begin{align*}"));
    assert!(maths[0].prepared.contains("\\tag*{(41)}"));
    assert!(!maths[0].prepared.contains("\\label"));
    // A two-row align has two numbers; multline has one.
    assert!(maths[1].prepared.contains("\\tag*{(42)}"));
    assert!(!maths[1].prepared.contains("(43)"));
    assert!(maths[3].prepared.starts_with("\\begin{gathered}"));
    assert_eq!(maths[3].prepared.matches("\\tag*").count(), 1);
    assert!(maths[3].prepared.contains("(43)"));
    // An explicit tag replaces the number and does not step the counter.
    assert!(maths[4].prepared.contains("\\tag{A}"));
    assert!(!maths[4].prepared.contains("\\tag*"));
    assert!(maths[6].prepared.contains("\\tag*{(44a)}"));
    assert!(maths[6].prepared.contains("\\tag*{(44b)}"));
}

#[test]
fn references_resolve_forward_and_unresolved_ones_show_their_key() {
    let source = r"See \eqref{e} and \ref{missing}.
\begin{equation}\label{e} x \end{equation}";
    assert!(text(source).starts_with("See (1) and [missing]."));
}

#[test]
fn eqnarray_rows_are_rewritten_and_numbered() {
    let source = r"\begin{eqnarray} a &=& b \\ c &<& d \nonumber \end{eqnarray}";
    let index = build(source);
    let prepared = maths(&index, source)[0].prepared.to_string();
    assert_eq!(
        prepared,
        "\\begin{align*} a &= b  \\tag*{(1)}\\\\ c &< d \\nonumber \\end{align*}"
    );
}

#[test]
fn sections_theorems_and_numberwithin_number_like_latex() {
    let source = r"\documentclass{report}
\usepackage{amsmath,amsthm}
\newtheorem{theorem}{Theorem}[chapter]
\theoremstyle{definition}
\newtheorem{definition}[theorem]{Definition}
\numberwithin{equation}{section}
\begin{document}
\chapter{One}
\section{A}
\begin{theorem}[Name]\label{t} x \end{theorem}
\begin{definition}\label{d} y \end{definition}
\begin{equation}\label{e} z \end{equation}
\subsection{B}\label{s}
\subsubsection{Unnumbered at depth 2}
\appendix
\chapter{Extra}\label{c}
\end{document}";
    let index = build(source);
    assert_eq!(index.label("t"), Some("1.1"));
    assert_eq!(index.label("d"), Some("1.2"));
    assert_eq!(index.label("e"), Some("1.1.1"));
    assert_eq!(index.label("s"), Some("1.1.1"));
    assert_eq!(index.label("c"), Some("A"));
    let outline: Vec<_> = index
        .outline()
        .iter()
        .map(|e| (e.level, e.number.clone(), e.title.clone()))
        .collect();
    assert_eq!(outline[0], (1, Some("1".into()), "One".into()));
    assert_eq!(outline[3], (4, None, "Unnumbered at depth 2".into()));
    let theorem = events(&index, source)
        .into_iter()
        .find_map(|s| match s.event {
            Event::Start(Tag::Theorem {
                name,
                number,
                style,
            }) if name == "Definition" => Some((number, style)),
            _ => None,
        });
    assert_eq!(
        theorem,
        Some((Some("1.2".into()), latex_preview::TheoremStyle::Definition))
    );
}

#[test]
fn unknown_input_stays_visible_and_is_diagnosed() {
    let source = r"\centering Text \foo{bar}[x] baz. \begin{custom}{arg} body \end{custom}";
    let index = build(source);
    let all = events(&index, source);
    let raws: Vec<String> = all
        .iter()
        .filter_map(|s| match &s.event {
            Event::Raw(raw) => Some(raw.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(
        raws,
        [
            "\\foo",
            "{",
            "}",
            "[",
            "]",
            "\\begin{custom}{arg}",
            "\\end{custom}"
        ]
    );
    assert!(all.iter().any(|s| matches!(s.event, Event::Hint(_))));
    assert!(text(source).contains("bar"));
    let kinds: Vec<_> = index.diagnostics().first().iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        [
            DiagnosticKind::UnknownCommand,
            DiagnosticKind::UnknownEnvironment
        ]
    );
}

#[test]
fn included_files_share_the_document_state() {
    let main = "\\documentclass{article}\\begin{document}\n\\input{defs}\n\\section{A}\\label{a}\n\\include{part}\nSee \\ref{b}.\n\\lstinputlisting{gone.py}\n\\end{document}";
    let mut read = |path: &str, _| match path {
        "defs.tex" => Some("\\newcommand{\\R}{\\mathbb{R}}".to_owned()),
        "part.tex" => Some("\\section{B}\\label{b}Text $\\R$, after \\ref{a}.".to_owned()),
        _ => None,
    };
    let expanded = latex_preview::expand(main, None, &Default::default(), &mut read);
    assert_eq!(expanded.files, ["defs.tex", "part.tex"]);
    assert_eq!(expanded.missing, ["gone.py"]);
    let source = expanded.text.as_ref();
    let index = build(source);
    assert_eq!(text(source), "ABText $\\R$, after 1.\nSee 2.\n");
    let maths = maths(&index, source);
    assert!(maths[0].context.bindings().any(|b| &*b.name == "\\R"));
    let listing = events(&index, source)
        .into_iter()
        .find(|s| matches!(&s.event, Event::Include(i) if i.kind == latex_preview::IncludeKind::Listing))
        .unwrap();
    assert_eq!(&source[listing.range], "\\lstinputlisting{gone.py}");
    assert_eq!(index.diagnostics().total(), 0);
    let index = build("\\graphicspath{{figures/}{ img/ }}\\includegraphics{plot}");
    assert_eq!(index.graphics_path(), ["figures/", "img/"]);
}

#[test]
fn verbatim_keeps_content_literal() {
    let source = "a\n\\begin{verbatim}\nraw { $ % \\end{itemize}\n\\end{verbatim}\n\\verb|a{b$| c";
    let out = text(source);
    assert!(out.contains("raw { $ % \\end{itemize}\n"), "{out}");
    assert!(out.contains("a{b$ c"), "{out}");
}

#[test]
fn text_conversions_follow_tex() {
    assert_eq!(
        text("``Quoted''---dash--range \\'ecole \\\"{o} 50\\% \\LaTeX{} ok"),
        "“Quoted”—dash–range école ö 50% LaTeX ok\n"
    );
    assert_eq!(text("a % comment\nb\n\nc"), "a b\nc\n");
}

#[test]
fn user_macros_expand_in_text_with_arguments() {
    let source = r"\newcommand{\name}[1]{\textbf{#1}!}\newcommand{\opt}[2][d]{#1-#2}
\name{Hi} \opt{x} \opt[y]{z}";
    assert_eq!(text(source), "Hi! d-x y-z\n");
}

#[test]
fn citations_resolve_against_the_bibliography() {
    let source = r"See \cite[p.~2]{a,b}.
\begin{thebibliography}{9}
\bibitem{a} First. \bibitem[X]{b} Second.
\end{thebibliography}";
    assert!(text(source).starts_with("See [1, X, p.\u{a0}2]."));
    let source = r"See \cite[\S 3.1, pp.~3--5 \unknown]{a}.";
    assert_eq!(text(source), "See [a, §3.1, pp.\u{a0}3–5 \\unknown].\n");
}

#[test]
fn spaces_after_a_display_are_ignored() {
    let source = "Before\n\\[ x \\]\n after \\begin{equation*}\ny\n\\end{equation*}  again.";
    assert_eq!(text(source), "Before $ x $after $\ny\n$again.\n");
}

fn assert_windows_match(source: &str, options: Options) -> Index {
    let index = build_with(source, options);
    let whole = events(&index, source);
    let mut windows = Vec::new();
    let mut starts = Vec::new();
    for checkpoint in 0..index.checkpoints().len() {
        starts.push((windows.len(), checkpoint));
        windows.extend(index.window(source, checkpoint));
    }
    for (i, (a, b)) in whole.iter().zip(&windows).enumerate() {
        if a != b {
            let (_, checkpoint) = starts.iter().rev().find(|(at, _)| *at <= i).unwrap();
            let offset = index.checkpoints()[*checkpoint].offset();
            panic!(
                "event {i} differs in window {checkpoint} at offset {offset} ({:?}):\n{a:?}\n{b:?}\ncontext: {:?}",
                index.checkpoints()[*checkpoint].open_tags(),
                &source[offset.saturating_sub(40)..(offset + 40).min(source.len())]
            );
        }
    }
    assert_eq!(whole.len(), windows.len());
    index
}

fn large_document() -> String {
    let mut source = String::from(
        "\\documentclass{article}\n\\newcommand{\\R}{\\mathbb{R}}\n\\begin{document}\n",
    );
    for i in 0..40 {
        source.push_str(&format!(
            "\\section{{Part {i}}}\\label{{s{i}}}\nText with $x \\in \\R$ and \\emph{{style}} {{\\bf bold\n\nacross}} see \\ref{{s{i}}}.\n\n"
        ));
        source.push_str("\\begin{itemize}\n");
        for j in 0..20 {
            source.push_str(&format!("\\item item {j} with \\textbf{{bold}} text\n"));
        }
        source.push_str("\\end{itemize}\n\\begin{verbatim}\n");
        for j in 0..20 {
            source.push_str(&format!("line {j} {{ $ %\n"));
        }
        source.push_str("\\end{verbatim}\n\\begin{tabular}{lc}\n");
        for j in 0..20 {
            source.push_str(&format!("a{j} & b \\\\ \\hline\n"));
        }
        source.push_str("\\end{tabular}\n\\begin{align}\na &= b \\\\ c &= d\n\\end{align}\n");
        source.push_str(&format!("{{\\renewcommand{{\\R}}{{{i}}} $\\R$}}\n\n"));
    }
    source.push_str("\\end{document}\n");
    source
}

#[test]
fn windows_from_every_checkpoint_repeat_the_sequential_events() {
    let source = large_document();
    let options = Options {
        checkpoint_spacing: 256,
        ..Options::default()
    };
    let index = assert_windows_match(&source, options);
    assert!(
        index.checkpoints().len() > 200,
        "{}",
        index.checkpoints().len()
    );
    // Checkpoints fall inside lists, verbatim blocks, and tables.
    let open: Vec<Vec<TagKind>> = index.checkpoints().iter().map(|c| c.open_tags()).collect();
    assert!(open.iter().any(|tags| tags.contains(&TagKind::Item)));
    assert!(open.iter().any(|tags| tags.contains(&TagKind::Code)));
    assert!(open.iter().any(|tags| tags.contains(&TagKind::Table)));
}

#[test]
fn a_window_near_the_end_needs_no_earlier_parsing() {
    let source = large_document();
    let options = Options {
        checkpoint_spacing: 1024,
        ..Options::default()
    };
    let index = build_with(&source, options);
    let last = index.checkpoints().len() - 1;
    let window: Vec<_> = index.window(&source, last).collect();
    let whole = events(&index, &source);
    assert_eq!(&whole[whole.len() - window.len()..], &window[..]);
    // The last section's formula still sees the preamble macro.
    let math = window.iter().rev().find_map(|s| match &s.event {
        Event::Math(m) if m.prepared == "\\R" && !m.display => Some(m.clone()),
        _ => None,
    });
    if let Some(math) = math {
        assert!(math.context.bindings().any(|b| b.name == "\\R"));
    }
}

#[test]
fn indexing_stops_at_the_checkpoint_cap_and_shows_the_rest_as_source() {
    let source = large_document();
    let options = Options {
        checkpoint_spacing: 256,
        max_checkpoints: 5,
        ..Options::default()
    };
    let index = assert_windows_match(&source, options);
    let stopped = index.stopped().expect("stopped");
    assert_eq!(index.checkpoints().len(), 5);
    let raw: usize = index
        .events(&source)
        .filter_map(|s| match s.event {
            Event::Raw(raw) if s.range.start >= stopped => Some(raw.len()),
            _ => None,
        })
        .sum();
    assert_eq!(raw, source.len() - stopped);
    assert!(
        index
            .diagnostics()
            .first()
            .iter()
            .any(|d| d.kind == DiagnosticKind::IndexStopped)
    );
}

#[test]
fn cancellation_stops_the_first_pass_and_windows() {
    let source = large_document();
    let cancel = AtomicBool::new(true);
    assert!(index(&source, &Options::default(), &cancel).is_err());
    let index = build(&source);
    let mut events = index.events(&source).with_cancel(&cancel);
    assert!(events.next().is_none());
    assert!(events.cancelled());
}

#[test]
fn author_blocks_keep_names_and_thanks_as_footnotes() {
    let source = r"\title{T}\author{A\thanks{Equal.}\\ Lab \And B\footnotemark[1]}
\begin{document}\maketitle\end{document}";
    let index = build(source);
    let all = events(&index, source);
    assert!(
        !all.iter().any(|s| matches!(s.event, Event::Raw(_))),
        "{all:?}"
    );
    assert!(
        all.iter()
            .any(|s| matches!(&s.event, Event::Start(Tag::Footnote { number: None })))
    );
    assert!(text(source).contains("Equal."));
}
