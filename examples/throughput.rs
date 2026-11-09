//! Times the first pass and a full event read of a file:
//! `cargo run --release --example throughput -- paper.tex [runs]`.

use std::{sync::atomic::AtomicBool, time::Instant};

use latex_reader::Options;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a .tex file");
    let runs: usize = args.next().map_or(7, |r| r.parse().expect("a run count"));
    let source = std::fs::read_to_string(&path).expect("readable UTF-8");
    let cancel = AtomicBool::new(false);
    let mut first = Vec::new();
    let mut read = Vec::new();
    let mut events;
    for _ in 0..runs {
        let start = Instant::now();
        let index = latex_reader::index(&source, &Options::default(), &cancel).unwrap();
        first.push(start.elapsed().as_secs_f64());
        let start = Instant::now();
        events = index.events(&source).count();
        read.push(start.elapsed().as_secs_f64());
        if first.len() == runs {
            let total = index.diagnostics().total();
            println!(
                "{} bytes, {} checkpoints, {events} events, {total} diagnostics",
                source.len(),
                index.checkpoints().len()
            );
        }
    }
    let median = |values: &mut Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    let (first, read) = (median(&mut first), median(&mut read));
    let rate = |seconds: f64| source.len() as f64 / seconds / 1e6;
    println!("first pass {:.1} ms ({:.0} MB/s)", first * 1e3, rate(first));
    println!("events     {:.1} ms ({:.0} MB/s)", read * 1e3, rate(read));
}
