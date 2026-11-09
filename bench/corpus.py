"""Build the benchmark corpus of public LaTeX papers described in CONTRIBUTING.md.

Downloads two pinned arXiv source archives, verifies their SHA-256 checksums,
and writes these inputs to an output directory outside the repository:

  body.tex       every .tex file of both papers, in path order, joined with
                 newlines, with \\begin{document} and \\end{document} removed
  body-x100.tex  body.tex repeated 100 times
  tricky.tex     a preamble macro, a verbatim block, \\verb, and a theorem

Archives are read in memory; nothing is extracted. Every output is checked
against its recorded checksum, so a changed archive or procedure fails loudly.
"""

import argparse
import gzip
import hashlib
import io
import re
import sys
import tarfile
import urllib.request
from pathlib import Path

ARCHIVES = [
    ("1706.03762v7", "2e7a7d9ee2520d22eb23dae0cb148e167be02a30d6cb3f11b1c67723194339c6"),
    ("2005.14165v4", "79c1afc60beccb0bd6d2f1362bb214e4bbda16e0dbc80c1b37d3b172049d5021"),
]
OUTPUTS = {
    "body.tex": "a65b151ecedeb1eda523a50c128cb38fd070da6ec01b0de861f0176f6f5de3b9",
    "body-x100.tex": "da4f6442d1808f4701350b9ed2a37b005942779ebba6f7bd944db6cfed0f18a4",
    "tricky.tex": "c916fd1d542c3ce53973b0e997aa653c3b31379d4b552977ec193d0cc413ea17",
}
TRICKY = r"""\newcommand{\R}{\mathbb{R}}
\section{Intro}\label{sec:a}
Text with \emph{em} and 50\% and $x \in \R$ % comment {
\begin{verbatim}
raw { $ % \end{itemize}
\end{verbatim}
\verb|a{b$|
\begin{theorem}[Name] Body \cite{k}. \end{theorem}
"""
USER_AGENT = "latex-reader-bench (LaTeX parser corpus)"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fetch(identifier: str, expected: str, cache: Path) -> bytes:
    path = cache / f"{identifier}.tar.gz"
    if path.exists():
        data = path.read_bytes()
    else:
        request = urllib.request.Request(
            f"https://arxiv.org/e-print/{identifier}", headers={"User-Agent": USER_AGENT}
        )
        with urllib.request.urlopen(request, timeout=60) as response:
            data = response.read()
    if sha256(data) != expected:
        sys.exit(f"{identifier}: archive checksum mismatch")
    path.write_bytes(data)
    return data


def tex_sources(archive: bytes) -> list[str]:
    files = {}
    with tarfile.open(fileobj=io.BytesIO(gzip.decompress(archive))) as tar:
        for member in tar.getmembers():
            name = member.name.removeprefix("./")
            if member.isfile() and name.endswith(".tex"):
                text = tar.extractfile(member).read().decode("utf-8", errors="replace")
                files[name] = text.replace("\r\n", "\n").replace("\r", "\n")
    return [files[name] for name in sorted(files)]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("output", type=Path, help="directory outside the repository")
    output = parser.parse_args().output.resolve()
    repository = Path(__file__).resolve().parents[1]
    if output == repository or repository in output.parents:
        sys.exit("write the corpus outside the repository")
    output.mkdir(parents=True, exist_ok=True)

    sources = []
    for identifier, expected in ARCHIVES:
        sources += tex_sources(fetch(identifier, expected, output))
    body = re.sub(r"\\(begin|end)\{document\}", "", "\n".join(sources) + "\n")
    contents = {"body.tex": body, "body-x100.tex": body * 100, "tricky.tex": TRICKY}
    for name, text in contents.items():
        data = text.encode()
        if sha256(data) != OUTPUTS[name]:
            sys.exit(f"{name}: output checksum mismatch")
        (output / name).write_bytes(data)
        print(f"{name}: {len(data):,} bytes, sha256 {OUTPUTS[name]}")


if __name__ == "__main__":
    main()
