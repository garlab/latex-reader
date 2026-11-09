//! Prints a document's events, outline, and diagnostics, with included files
//! in place:
//! `cargo run --example dump -- paper.tex`.

use std::sync::atomic::AtomicBool;

use latex_preview::{Event, Options};

fn main() {
    let path = std::env::args().nth(1).expect("a .tex file");
    let source = std::fs::read_to_string(&path).expect("readable UTF-8");
    // Included files resolve against the main file's directory, as in LaTeX.
    let path = std::path::Path::new(&path);
    let dir = path.parent().unwrap_or(std::path::Path::new(""));
    let jobname = path.file_stem().and_then(|s| s.to_str());
    let mut read = |name: &str, _| std::fs::read_to_string(dir.join(name)).ok();
    let expanded = latex_preview::expand(&source, jobname, &Default::default(), &mut read);
    if !expanded.files.is_empty() || !expanded.missing.is_empty() {
        eprintln!(
            "included {:?}, missing {:?}",
            expanded.files, expanded.missing
        );
    }
    let source = expanded.text;
    let index = latex_preview::index(&source, &Options::default(), &AtomicBool::new(false))
        .expect("not cancelled");
    let mut depth = 0usize;
    for spanned in index.events(&source) {
        if matches!(spanned.event, Event::End(_)) {
            depth = depth.saturating_sub(1);
        }
        let shown = match &spanned.event {
            Event::Math(math) => format!(
                "Math(display: {}, {:?}, context {:x})",
                math.display,
                math.prepared,
                math.context.hash() >> 96
            ),
            other => format!("{other:?}"),
        };
        println!(
            "{:width$}{shown}  @{:?}",
            "",
            spanned.range,
            width = depth * 2
        );
        if matches!(spanned.event, Event::Start(_)) {
            depth += 1;
        }
    }
    println!("\noutline:");
    for entry in index.outline() {
        println!("  {} {:?} {}", entry.level, entry.number, entry.title);
    }
    println!("\ndiagnostics: {}", index.diagnostics().total());
    for diagnostic in index.diagnostics().first() {
        println!(
            "  {:?} {:?} @{:?}",
            diagnostic.kind, diagnostic.name, diagnostic.range
        );
    }
    println!("by name:");
    let mut counts: Vec<_> = index.diagnostics().counts().collect();
    counts.sort_by_key(|(_, _, count)| std::cmp::Reverse(*count));
    for (kind, name, count) in counts {
        println!("  {count:5} {kind:?} {name}");
    }
    println!("checkpoints: {}", index.checkpoints().len());
}
