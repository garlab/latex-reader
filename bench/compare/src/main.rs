//! Measures other LaTeX parsers and KaTeX coverage, for comparison with
//! latex-reader. See CONTRIBUTING.md.
//!
//! ```text
//! latex-parser-probe parse <tree-sitter|mitex> FILE   one timed parse, as JSON
//! latex-parser-probe structure FILE                   structure counts from both parsers
//! latex-parser-probe dump <tree-sitter|mitex> FILE    the parse tree
//! latex-parser-probe katex                            formula and macro checks
//! ```

use std::{collections::BTreeMap, time::Instant};

use katex_native::katex::{KatexContext, Settings, macros::MacroDefinition};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let read = |path: &str| std::fs::read_to_string(path).expect("readable UTF-8 input");
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["parse", parser, path] => parse(parser, &read(path)),
        ["structure", path] => structure(&read(path)),
        ["dump", "tree-sitter", path] => println!("{}", tree_sitter(&read(path)).root_node()),
        ["dump", "mitex", path] => println!("{:#?}", mitex(&read(path))),
        ["katex"] => katex(),
        _ => {
            eprintln!("usage: see the module documentation in src/main.rs");
            std::process::exit(2);
        }
    }
}

fn tree_sitter(text: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&codebook_tree_sitter_latex::LANGUAGE.into())
        .unwrap();
    parser.parse(text, None).unwrap()
}

/// `parse` runs MiTeX's macro engine before building the tree.
fn mitex(text: &str) -> mitex_parser::syntax::SyntaxNode {
    mitex_parser::parse(text, mitex_spec_gen::DEFAULT_SPEC.clone())
}

/// Times the parse alone; counting nodes afterwards is not part of the measurement.
fn parse(parser: &str, text: &str) {
    let start = Instant::now();
    let (nanos, nodes) = match parser {
        "tree-sitter" => {
            let tree = tree_sitter(text);
            (
                start.elapsed().as_nanos(),
                tree.root_node().descendant_count(),
            )
        }
        "mitex" => {
            let root = mitex(text);
            (start.elapsed().as_nanos(), root.descendants().count())
        }
        _ => panic!("unknown parser {parser}"),
    };
    println!(
        "{{\"parser\":\"{parser}\",\"bytes\":{},\"nanos\":{nanos},\"nodes\":{nodes}}}",
        text.len()
    );
}

fn structure(text: &str) {
    let tree = tree_sitter(text);
    let mut kinds = BTreeMap::<&str, usize>::new();
    let mut errors = 0;
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        errors += usize::from(node.is_error() || node.is_missing());
        if node.is_named() {
            *kinds.entry(node.kind()).or_default() += 1;
        }
        stack.extend(node.children(&mut cursor));
    }
    let count = |names: &[&str]| {
        names
            .iter()
            .map(|n| kinds.get(n).unwrap_or(&0))
            .sum::<usize>()
    };
    println!(
        "tree-sitter: sections {}, environments {}, formulas {}, error or missing nodes {errors}",
        count(&["section"]),
        count(&[
            "generic_environment",
            "math_environment",
            "verbatim_environment"
        ]),
        count(&["inline_formula", "displayed_equation"]),
    );
    let root = mitex(text);
    let kind = |name: &str| {
        root.descendants()
            .filter(|n| format!("{:?}", n.kind()) == name)
            .count()
    };
    let errors = root
        .descendants_with_tokens()
        .filter(|n| format!("{:?}", n.kind()).contains("Error"))
        .count();
    println!(
        "mitex: environments {}, formulas {}, error nodes or tokens {errors}",
        kind("ItemEnv"),
        kind("ItemFormula"),
    );
}

fn katex() {
    let context = KatexContext::default();
    let cases = [
        r"\begin{equation} E = mc^2 \end{equation}",
        r"\begin{equation}\label{eq:a} E = mc^2 \end{equation}",
        r"\begin{align} a &= b \\ c &= d \nonumber \\ e &= f \end{align}",
        r"\begin{align} a &= b \tag{7} \\ c &= d \end{align}",
        r"\begin{gather*} a \\ b \end{gather*}",
        r"\begin{multline} a + b \\ + c \end{multline}",
        r"\begin{eqnarray} a &=& b \end{eqnarray}",
        r"\begin{flalign} a &= b \end{flalign}",
        r"\begin{equation}\begin{split} a &= b \\ &= c \end{split}\end{equation}",
        r"f(x) = \begin{cases} 1 & x > 0 \\ 0 & \text{otherwise} \end{cases}",
        r"\eqref{eq:a}",
        r"\DeclareMathOperator{\tr}{tr} \tr A",
        r"\operatorname{tr} A",
        r"\bm{x} + \boldsymbol{y}",
        r"\mathbbm{1}",
        r"x \text{ for all } y",
        // Candidate rewrite targets: unnumbered environments with explicit tags.
        r"\begin{equation} x \notag \end{equation}",
        r"\begin{align*} a &= b \tag{1} \\ c &= d \\ e &= f \tag{2} \end{align*}",
        r"\begin{align*} a &= b \tag*{A} \end{align*}",
        r"\begin{gather*} a \tag{3} \\ b \end{gather*}",
        r"\begin{equation*} \begin{gathered} a + b \\ + c \end{gathered} \tag{4} \end{equation*}",
    ];
    for tex in cases {
        let settings = Settings::builder().display_mode(true).build();
        match katex_native::layout_numbered(&context, tex, &settings, 1) {
            Ok(layout) => println!(
                "ok     numbers {} tag {} | {tex}",
                layout.equation_numbers,
                layout.tag.is_some()
            ),
            Err(err) => println!("error  {err} | {tex}"),
        }
    }

    let settings = Settings::builder().display_mode(false).build();
    for (name, body) in [
        (r"\R", r"\mathbb{R}"),
        (r"\tr", r"\operatorname{tr}"),
        (r"\norm", r"\left\lVert #1 \right\rVert"),
    ] {
        settings
            .macros
            .borrow_mut()
            .insert(name.into(), MacroDefinition::String(body.into()));
    }
    for tex in [r"x \in \R^n", r"\tr A", r"\norm{x}"] {
        let result = katex_native::layout_numbered(&context, tex, &settings, 1);
        println!(
            "macro map    {:5} | {tex}",
            if result.is_ok() { "ok" } else { "error" }
        );
    }

    let settings = Settings::builder()
        .display_mode(false)
        .global_group(true)
        .build();
    let definitions = r"\newcommand{\abs}[1]{\left|#1\right|}\def\eps{\varepsilon}";
    katex_native::layout_numbered(&context, definitions, &settings, 1).unwrap();
    for tex in [r"\abs{x}", r"\eps"] {
        let result = katex_native::layout_numbered(&context, tex, &settings, 1);
        println!(
            "global group {:5} | {tex}",
            if result.is_ok() { "ok" } else { "error" }
        );
    }
}
