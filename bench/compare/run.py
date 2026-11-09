"""Time other LaTeX parsers on the corpus from bench/corpus.py.

Each run is a fresh probe process that parses one in-memory input once. Parse
time excludes reading the file; peak RSS covers the whole process, including
the input string. Medians are printed per parser and input. Build the probe
first:

  cargo build --release --manifest-path bench/compare/Cargo.toml
  python3 -I bench/compare/run.py <corpus-directory>
"""

import argparse
import json
import platform
import statistics
import subprocess
import sys
from pathlib import Path

PARSERS = ["tree-sitter", "mitex"]
INPUTS = ["body.tex", "body-x100.tex"]


def measure(probe: Path, parser: str, path: Path) -> tuple[int, int, int]:
    """Returns (bytes, nanoseconds, peak RSS in bytes) for one process."""
    if platform.system() == "Darwin":
        time_command, rss_scale = ["/usr/bin/time", "-l"], 1
    else:
        time_command, rss_scale = ["/usr/bin/time", "-v"], 1024
    result = subprocess.run(
        [*time_command, str(probe), "parse", parser, str(path)],
        capture_output=True,
        text=True,
        check=True,
    )
    report = json.loads(result.stdout)
    rss = None
    for line in result.stderr.splitlines():
        if "maximum resident set size" in line.lower():
            rss = int(line.split()[0] if platform.system() == "Darwin" else line.split()[-1])
    if rss is None:
        sys.exit("could not read peak RSS from /usr/bin/time")
    return report["bytes"], report["nanos"], rss * rss_scale


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--runs", type=int, default=7)
    default_probe = Path(__file__).parent / "target/release/latex-parser-probe"
    parser.add_argument("--probe", type=Path, default=default_probe)
    args = parser.parse_args()

    print(f"{platform.machine()} {platform.system()} {platform.release()}, {args.runs} runs")
    print(f"{'parser':12} {'input':14} {'bytes':>12} {'median':>11} {'MB/s':>7} {'peak RSS':>10}")
    for name in INPUTS:
        for candidate in PARSERS:
            runs = [measure(args.probe, candidate, args.corpus / name) for _ in range(args.runs)]
            size = runs[0][0]
            nanos = statistics.median(run[1] for run in runs)
            rss = statistics.median(run[2] for run in runs)
            print(
                f"{candidate:12} {name:14} {size:>12,} {nanos / 1e6:>8.1f} ms "
                f"{size / nanos * 1e3:>7.1f} {rss / 2**20:>6.1f} MiB"
            )


if __name__ == "__main__":
    main()
