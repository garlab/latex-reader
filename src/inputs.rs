//! Splices included files into the text that [`index`](crate::index) reads.
//!
//! LaTeX reads `\input` and similar commands while it typesets, so included
//! text shares the document's macros, counters, and labels. [`expand`] gives
//! the parser the same view: it replaces each command with the file's text,
//! skipping comments and verbatim content. The crate still reads no files:
//! the application supplies them through a callback and keeps its own path
//! policy. Event ranges then refer to the expanded text.

use std::{borrow::Cow, collections::HashMap};

use crate::{
    commands::{self, EnvClass},
    lexer::{self, Kind, LexState},
};

/// What a requested file is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    /// LaTeX source: `\input`, `\include`, `\subfile`, and `\import`.
    Source,
    /// A file shown as code: `\lstinputlisting`, `\VerbatimInput`, and
    /// `\inputminted`.
    Listing,
    /// The `.bbl` file that `\bibliography` reads.
    Bibliography,
    /// `\IfFileExists`: only whether the file can be read matters, so the
    /// callback may return an empty string.
    Exists,
}

/// Bounds on expansion. A command over a limit stays in the text, where the
/// parser shows it as source.
#[derive(Clone, Copy, Debug)]
pub struct InputLimits {
    /// Nesting of included source files. TeX allows 15 open files.
    pub max_depth: usize,
    /// Files read, counting repeated reads.
    pub max_files: usize,
    /// Size of the expanded text.
    pub max_bytes: usize,
}

impl Default for InputLimits {
    fn default() -> Self {
        Self {
            max_depth: 15,
            max_files: 256,
            max_bytes: 64 << 20,
        }
    }
}

/// The source with included files in place.
#[derive(Clone, Debug)]
pub struct Expanded<'a> {
    /// The text to index. Borrowed when nothing was included.
    pub text: Cow<'a, str>,
    /// Paths that were read, as passed to the callback, without repeats.
    pub files: Vec<String>,
    /// For each command that stayed in the text, the first path tried.
    pub missing: Vec<String>,
}

/// Replaces include commands with the text of the files they name.
///
/// `read` receives a path relative to the main document's directory, or an
/// absolute one, and returns the file's text, or `None` if it can't be read.
/// LaTeX source names without an extension are tried with `.tex` first, as
/// TeX does. `jobname` (the main file's name without `.tex`) locates the
/// `.bbl` file for `\bibliography`.
pub fn expand<'a>(
    source: &'a str,
    jobname: Option<&str>,
    limits: &InputLimits,
    read: &mut dyn FnMut(&str, InputKind) -> Option<String>,
) -> Expanded<'a> {
    let mut expander = Expander {
        jobname,
        limits: *limits,
        read,
        files: Vec::new(),
        missing: Vec::new(),
        reads: 0,
        size: source.len(),
        open: Vec::new(),
        only: None,
        macros: HashMap::new(),
    };
    let text = match expander.splice(source, "", 0) {
        Some(text) => Cow::Owned(text),
        None => Cow::Borrowed(source),
    };
    Expanded {
        text,
        files: expander.files,
        missing: expander.missing,
    }
}

struct Expander<'r, 'j> {
    jobname: Option<&'j str>,
    limits: InputLimits,
    read: &'r mut dyn FnMut(&str, InputKind) -> Option<String>,
    files: Vec<String>,
    missing: Vec<String>,
    reads: usize,
    /// Size of the expanded text so far, counting unexpanded source.
    size: usize,
    /// Source files being expanded, to refuse cycles.
    open: Vec<String>,
    /// `\includeonly` names.
    only: Option<Vec<String>>,
    /// Macros without arguments whose bodies are plain text, for file names
    /// such as `\input{\dir/part}`.
    macros: HashMap<String, String>,
}

/// A command's replacement and where the command ends.
struct Splice {
    text: String,
    end: usize,
}

impl Expander<'_, '_> {
    /// The expanded text, or `None` if nothing changed. `dir` prefixes paths
    /// inside files reached through `\import`.
    fn splice(&mut self, text: &str, dir: &str, depth: usize) -> Option<String> {
        let mut out: Option<String> = None;
        let mut copied = 0;
        let mut pos = 0;
        let mut state = LexState::default();
        // The branch of `\IfFileExists` that is not taken.
        let mut skip: Option<(usize, usize)> = None;
        loop {
            if let Some((start, end)) = skip
                && pos >= start
            {
                pos = pos.max(end);
                skip = None;
            }
            let token = lexer::next(text, pos, text.len(), &mut state);
            pos = token.end;
            match token.kind {
                Kind::Eof => break,
                Kind::Word => {}
                _ => continue,
            }
            let name = &text[token.start + 1..token.name_end];
            let splice = match name {
                "makeatletter" => {
                    state.at_letter = true;
                    continue;
                }
                "makeatother" => {
                    state.at_letter = false;
                    continue;
                }
                "begin" => {
                    pos = skip_verbatim(text, pos);
                    continue;
                }
                "verb" => {
                    pos = skip_verb(text, pos);
                    continue;
                }
                "includeonly" => {
                    if let Some((names, _)) = group(text, pos) {
                        self.only = Some(
                            names
                                .split(',')
                                .map(|n| n.trim().to_owned())
                                .filter(|n| !n.is_empty())
                                .collect(),
                        );
                    }
                    continue;
                }
                "newcommand" | "renewcommand" | "providecommand" | "def" | "gdef" | "edef"
                | "xdef" => {
                    self.definition(name, text, pos);
                    continue;
                }
                "IfFileExists" => {
                    let Some((path, then_start, then_end)) = self
                        .path(text, pos)
                        .and_then(|(path, end)| Some((path, raw_group(text, end)?)))
                        .map(|(path, (_, start, end))| (path, start, end))
                    else {
                        continue;
                    };
                    let exists = self.reads < self.limits.max_files && {
                        self.reads += 1;
                        (self.read)(&path, InputKind::Exists).is_some()
                    };
                    if exists {
                        pos = then_start + 1;
                        skip = raw_group(text, then_end).map(|(_, start, end)| (start, end));
                    } else {
                        pos = raw_group(text, then_end).map_or(then_end, |(_, start, _)| start + 1);
                    }
                    continue;
                }
                "endinput" if depth > 0 => {
                    // TeX finishes the current line, then stops reading the file.
                    let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i + 1);
                    let out = out.get_or_insert_with(String::new);
                    out.push_str(&text[copied..end]);
                    return Some(std::mem::take(out));
                }
                _ => self.command(name, text, pos, dir, depth),
            };
            if let Some(splice) = splice {
                let out = out.get_or_insert_with(|| String::with_capacity(text.len()));
                out.push_str(&text[copied..token.start]);
                out.push_str(&splice.text);
                copied = splice.end;
                pos = splice.end;
                state.line_start = splice.text.ends_with('\n');
            }
        }
        let mut out = out?;
        out.push_str(&text[copied..]);
        Some(out)
    }

    /// The replacement for an include command whose arguments start at `pos`.
    fn command(
        &mut self,
        name: &str,
        text: &str,
        pos: usize,
        dir: &str,
        depth: usize,
    ) -> Option<Splice> {
        match name {
            "input" => {
                let (file, end) = self
                    .path(text, pos)
                    .or_else(|| bare_name(text, pos).map(|(n, e)| (n.to_owned(), e)))?;
                let mut names = Vec::new();
                if !dir.is_empty() {
                    names.extend(source_names(&join(dir, &file)));
                }
                names.extend(source_names(&file));
                self.source(&names, text, end, dir, depth, false)
            }
            "include" => {
                let (file, end) = self.path(text, pos)?;
                if self
                    .only
                    .as_ref()
                    .is_some_and(|only| !only.iter().any(|n| n == file.trim()))
                {
                    return Some(Splice {
                        text: String::new(),
                        end,
                    });
                }
                // `\include` is `\clearpage\input{...}\clearpage`.
                let names = [format!("{}.tex", join(dir, &file))];
                let mut splice = self.source(&names, text, end, dir, depth, false)?;
                splice.text = format!("\\clearpage\n{}\\clearpage\n", splice.text);
                Some(splice)
            }
            "subfile" => {
                let (file, end) = self.path(text, pos)?;
                let names = source_names(&join(dir, &file));
                self.source(&names, text, end, dir, depth, true)
            }
            "import" | "subimport" | "inputfrom" | "subinputfrom" | "includefrom"
            | "subincludefrom" => {
                let star = text[pos..].starts_with('*');
                let (path, after) = self.path(text, pos + usize::from(star))?;
                let (file, end) = self.path(text, after)?;
                let base = if name.starts_with("sub") {
                    join(dir, &path)
                } else {
                    path
                };
                let names = source_names(&join(&base, &file));
                self.source(&names, text, end, &base, depth, false)
            }
            "lstinputlisting" => {
                let (options, after) = optional(text, pos).unwrap_or(("", pos));
                let (file, end) = self.path(text, after)?;
                let head = if options.is_empty() {
                    "\\begin{lstlisting}\n".to_owned()
                } else {
                    format!("\\begin{{lstlisting}}[{options}]\n")
                };
                self.listing(&join(dir, &file), options, head, "lstlisting", end)
            }
            "VerbatimInput" | "BVerbatimInput" | "LVerbatimInput" | "verbatiminput" => {
                let star = text[pos..].starts_with('*');
                let pos = pos + usize::from(star);
                let (options, after) = optional(text, pos).unwrap_or(("", pos));
                let (file, end) = self.path(text, after)?;
                let head = "\\begin{verbatim}\n".to_owned();
                self.listing(&join(dir, &file), options, head, "verbatim", end)
            }
            "inputminted" => {
                let (options, after) = optional(text, pos).unwrap_or(("", pos));
                let (language, after) = group(text, after)?;
                let (file, end) = self.path(text, after)?;
                let head = format!("\\begin{{minted}}{{{}}}\n", language.trim());
                self.listing(&join(dir, &file), options, head, "minted", end)
            }
            "bibliography" => {
                let (_, end) = group(text, pos)?;
                let path = format!("{}.bbl", self.jobname?);
                let content = self.load(&[path], InputKind::Bibliography)?;
                let text = self.fits(with_newline(content))?;
                Some(Splice { text, end })
            }
            _ => None,
        }
    }

    /// A braced file name, with plain-text macros replaced.
    fn path(&self, text: &str, pos: usize) -> Option<(String, usize)> {
        let (raw, _, end) = raw_group(text, pos)?;
        let mut path = String::with_capacity(raw.len());
        let mut rest = raw.trim();
        while let Some(i) = rest.find('\\') {
            path.push_str(&rest[..i]);
            let name_len = rest[i + 1..]
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(rest.len() - i - 1);
            let value = self.macros.get(&rest[i + 1..i + 1 + name_len])?;
            path.push_str(value);
            rest = rest[i + 1 + name_len..].trim_start_matches(' ');
        }
        path.push_str(rest);
        (!path.is_empty()).then_some((path, end))
    }

    /// Records `\newcommand{\name}{text}` and `\def\name{text}` when the body
    /// is plain text.
    fn definition(&mut self, command: &str, text: &str, pos: usize) {
        let pos = skip_spaces(text, pos);
        let pos = pos + usize::from(text[pos..].starts_with('*'));
        let (name, after) = if let Some((name, _, end)) = raw_group(text, pos) {
            (name.trim(), end)
        } else {
            let rest = &text[pos..];
            let len = rest
                .get(1..)
                .and_then(|r| r.find(|c: char| !c.is_ascii_alphabetic()))
                .map_or(rest.len(), |i| i + 1);
            (&rest[..len], pos + len)
        };
        let Some(name) = name.strip_prefix('\\') else {
            return;
        };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphabetic()) {
            return;
        }
        let Some((body, start, _)) = raw_group(text, after) else {
            return;
        };
        // Arguments, or parameter text after `\def\name`, make it a real macro.
        if skip_spaces(text, after) != start || body.contains(['\\', '#']) {
            self.macros.remove(name);
            return;
        }
        if command == "providecommand" && self.macros.contains_key(name) {
            return;
        }
        self.macros.insert(name.to_owned(), body.trim().to_owned());
    }

    /// Reads a source file and expands it in turn, with `dir` for imports.
    fn source(
        &mut self,
        names: &[String],
        text: &str,
        end: usize,
        dir: &str,
        depth: usize,
        body_only: bool,
    ) -> Option<Splice> {
        if depth >= self.limits.max_depth {
            self.missing.push(names.first()?.clone());
            return None;
        }
        let (path, content) = self.load_named(names, InputKind::Source)?;
        if self.open.contains(&path) {
            self.missing.push(path);
            return None;
        }
        self.open.push(path);
        let content = if body_only {
            document_body(&content).to_owned()
        } else {
            content
        };
        // Sizes count every file read, an upper bound on the expanded text.
        let expanded = self
            .fits(content)
            .map(|content| self.splice(&content, dir, depth + 1).unwrap_or(content));
        self.open.pop();
        Some(Splice {
            text: with_newline(expanded?),
            // The file's last line end stands for the rest of the command's line.
            end: rest_of_line(text, end),
        })
    }

    fn listing(
        &mut self,
        path: &str,
        options: &str,
        head: String,
        env: &str,
        end: usize,
    ) -> Option<Splice> {
        let content = self.load(&[path.to_owned()], InputKind::Listing)?;
        let content = content.replace("\r\n", "\n");
        let (first, last) = line_range(options);
        let lines: Vec<&str> = content.lines().collect();
        let last = last.unwrap_or(lines.len()).min(lines.len());
        let first = first.unwrap_or(1).max(1);
        let selected = lines.get(first - 1..last).unwrap_or_default();
        let closing = format!("\\end{{{env}}}");
        if selected.iter().any(|line| line.contains(&closing)) {
            self.missing.push(path.to_owned());
            return None;
        }
        let mut text = head;
        for line in selected {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(&closing);
        let text = self.fits(text)?;
        Some(Splice { text, end })
    }

    /// Reads the first of `names` that exists.
    fn load(&mut self, names: &[String], kind: InputKind) -> Option<String> {
        self.load_named(names, kind).map(|(_, content)| content)
    }

    fn load_named(&mut self, names: &[String], kind: InputKind) -> Option<(String, String)> {
        for name in names {
            if self.reads >= self.limits.max_files {
                break;
            }
            self.reads += 1;
            if let Some(content) = (self.read)(name, kind) {
                if !self.files.contains(name) {
                    self.files.push(name.clone());
                }
                return Some((name.clone(), content));
            }
        }
        if let Some(first) = names.first() {
            self.missing.push(first.clone());
        }
        None
    }

    /// Accepts `text` if the expanded document stays within the size limit.
    fn fits(&mut self, text: String) -> Option<String> {
        let size = self.size.checked_add(text.len())?;
        if size > self.limits.max_bytes {
            return None;
        }
        self.size = size;
        Some(text)
    }
}

/// Names TeX tries for `\input{name}`: `.tex` first unless the name has an
/// extension.
fn source_names(name: &str) -> Vec<String> {
    let name = name.trim();
    let file = name.rsplit('/').next().unwrap_or(name);
    if file.contains('.') {
        vec![name.to_owned(), format!("{name}.tex")]
    } else {
        vec![format!("{name}.tex"), name.to_owned()]
    }
}

fn join(dir: &str, file: &str) -> String {
    let file = file.trim();
    if dir.is_empty() || file.starts_with('/') {
        return file.to_owned();
    }
    if dir.ends_with('/') {
        format!("{dir}{file}")
    } else {
        format!("{dir}/{file}")
    }
}

fn with_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Past the line end that follows `pos`, if only spaces come before it.
fn rest_of_line(text: &str, pos: usize) -> usize {
    let rest = &text[pos..];
    let spaces = rest.len() - rest.trim_start_matches([' ', '\t', '\r']).len();
    match rest[spaces..].chars().next() {
        Some('\n') => pos + spaces + 1,
        None => text.len(),
        _ => pos,
    }
}

/// The text between `\begin{document}` and `\end{document}`, for `\subfile`.
fn document_body(text: &str) -> &str {
    let Some(start) = text.find("\\begin{document}") else {
        return text;
    };
    let body = &text[start + "\\begin{document}".len()..];
    match body.find("\\end{document}") {
        Some(end) => &body[..end],
        None => body,
    }
}

/// `firstline`, `lastline`, and `linerange` from listing options.
fn line_range(options: &str) -> (Option<usize>, Option<usize>) {
    let mut first = None;
    let mut last = None;
    for option in options.split(',') {
        let Some((key, value)) = option.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches(['{', '}']);
        match key.trim() {
            "firstline" => first = value.parse().ok(),
            "lastline" => last = value.parse().ok(),
            "linerange" => {
                if let Some((a, b)) = value.split_once('-') {
                    first = a.trim().parse().ok();
                    last = b.trim().parse().ok();
                }
            }
            _ => {}
        }
    }
    (first, last)
}

fn skip_spaces(text: &str, pos: usize) -> usize {
    let rest = &text[pos..];
    pos + rest.len() - rest.trim_start_matches([' ', '\t', '\r', '\n']).len()
}

/// A braced argument without commands, and the position after it.
fn group(text: &str, pos: usize) -> Option<(&str, usize)> {
    let (content, _, end) = raw_group(text, pos)?;
    (!content.contains('\\')).then_some((content, end))
}

/// A braced argument's content, where its `{` is, and the position after it.
fn raw_group(text: &str, pos: usize) -> Option<(&str, usize, usize)> {
    let start = skip_spaces(text, pos);
    if !text[start..].starts_with('{') {
        return None;
    }
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, byte) in text.as_bytes()[start..].iter().enumerate() {
        match byte {
            _ if escaped => escaped = false,
            b'\\' => escaped = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[start + 1..start + i], start, start + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

fn optional(text: &str, pos: usize) -> Option<(&str, usize)> {
    let start = skip_spaces(text, pos);
    if !text[start..].starts_with('[') {
        return None;
    }
    let end = text[start..].find(']')? + start;
    Some((&text[start + 1..end], end + 1))
}

/// `\input name`: TeX's own syntax, ended by a space.
fn bare_name(text: &str, pos: usize) -> Option<(&str, usize)> {
    let rest = &text[pos..];
    let len = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '{' | '}' | '\\' | '%'))
        .unwrap_or(rest.len());
    (len > 0).then(|| (&rest[..len], pos + len))
}

/// After `\begin`: skips the body of verbatim and hidden environments.
fn skip_verbatim(text: &str, pos: usize) -> usize {
    let Some((name, after)) = group(text, pos) else {
        return pos;
    };
    let verbatim = matches!(
        commands::environment(name),
        Some((
            EnvClass::Verbatim(_) | EnvClass::Hidden | EnvClass::RawBlock,
            _
        ))
    );
    if !verbatim {
        return pos;
    }
    let closing = format!("\\end{{{name}}}");
    text[after..]
        .find(&closing)
        .map_or(text.len(), |i| after + i + closing.len())
}

/// After `\verb`: skips the delimited argument.
fn skip_verb(text: &str, pos: usize) -> usize {
    let mut chars = text[pos..].char_indices();
    let mut delimiter = chars.next();
    if delimiter.is_some_and(|(_, c)| c == '*') {
        delimiter = chars.next();
    }
    let Some((_, delimiter)) = delimiter else {
        return text.len();
    };
    for (i, c) in chars {
        if c == delimiter || c == '\n' {
            return pos + i + c.len_utf8();
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn run(source: &str, files: &[(&str, &str)]) -> (String, Vec<String>, Vec<String>) {
        let files: HashMap<&str, &str> = files.iter().copied().collect();
        let mut read = |path: &str, _| files.get(path).map(|s| (*s).to_owned());
        let expanded = expand(source, Some("main"), &InputLimits::default(), &mut read);
        (expanded.text.into_owned(), expanded.files, expanded.missing)
    }

    #[test]
    fn input_names_follow_tex() {
        let (text, files, missing) = run(
            "a \\input{one}\nb \\input two.tex\n\\input{three}\nc",
            &[("one.tex", "1\n"), ("two.tex", "2"), ("three.tex", "3")],
        );
        assert_eq!(text, "a 1\nb 2\n3\nc");
        assert_eq!(files, ["one.tex", "two.tex", "three.tex"]);
        assert!(missing.is_empty());
    }

    #[test]
    fn comments_verbatim_and_missing_files_stay_as_written() {
        let source = "% \\input{a}\n\\begin{verbatim}\n\\input{a}\n\\end{verbatim}\n\\verb|\\input{a}| \\input{gone}";
        let (text, files, missing) = run(source, &[("a.tex", "A")]);
        assert_eq!(text, source);
        assert!(files.is_empty());
        assert_eq!(missing, ["gone.tex"]);
    }

    #[test]
    fn nested_files_cycles_and_endinput() {
        let (text, _, missing) = run(
            "\\input{a}",
            &[
                ("a.tex", "x \\input{b}\n"),
                ("b.tex", "y \\input{a}\nz \\endinput w\nnot read\n"),
            ],
        );
        assert_eq!(text, "x y \\input{a}\nz \\endinput w\n");
        assert_eq!(missing, ["a.tex"]);
    }

    #[test]
    fn includeonly_imports_and_subfiles() {
        let (text, _, _) = run(
            "\\includeonly{one}\\include{one}\\include{two}\\import{dir/}{p}\\subfile{s}",
            &[
                ("one.tex", "1"),
                ("two.tex", "2"),
                ("dir/p.tex", "p \\input{q}"),
                ("dir/q.tex", "q"),
                (
                    "s.tex",
                    "\\documentclass[main]{subfiles}\\begin{document}S\\end{document}",
                ),
            ],
        );
        assert_eq!(
            text,
            "\\includeonly{one}\\clearpage\n1\n\\clearpage\np q\nS\n"
        );
    }

    #[test]
    fn listings_select_lines_and_bibliographies_use_the_bbl_file() {
        let (text, _, _) = run(
            "\\lstinputlisting[language=Python,firstline=2]{a.py}\n\\VerbatimInput{b.txt}\n\\bibliography{refs}",
            &[
                ("a.py", "skip\nkeep\n"),
                ("b.txt", "\\end{verbatim}"),
                (
                    "main.bbl",
                    "\\begin{thebibliography}{1}\\end{thebibliography}",
                ),
            ],
        );
        assert_eq!(
            text,
            "\\begin{lstlisting}[language=Python,firstline=2]\nkeep\n\\end{lstlisting}\n\\VerbatimInput{b.txt}\n\\begin{thebibliography}{1}\\end{thebibliography}\n"
        );
    }

    #[test]
    fn file_names_use_plain_macros_and_file_tests() {
        let (text, files, _) = run(
            "\\newcommand{\\root}{gone}\\IfFileExists{here.txt}{\\renewcommand{\\root}{src}}{}\n\\def\\ext{tex}\\input{\\root/a.\\ext}\\input{\\unknown/a}",
            &[("here.txt", ""), ("src/a.tex", "A")],
        );
        assert!(
            text.contains("}{}\n\\def\\ext{tex}A\n\\input{\\unknown/a}"),
            "{text}"
        );
        assert_eq!(files, ["src/a.tex"]);
        let (text, _, _) = run(
            "\\newcommand{\\root}{gone}\\IfFileExists{absent}{\\def\\root{src}}{\\def\\root{other}}\\input{\\root/a}",
            &[("src/a.tex", "A"), ("other/a.tex", "B")],
        );
        assert!(text.ends_with("}B\n"), "{text}");
    }

    #[test]
    fn limits_leave_commands_in_place() {
        let files: HashMap<&str, &str> = [("a.tex", "0123456789")].into_iter().collect();
        let mut read = |path: &str, _| files.get(path).map(|s| (*s).to_owned());
        let limits = InputLimits {
            max_bytes: 12,
            ..InputLimits::default()
        };
        let expanded = expand("\\input{a}", None, &limits, &mut read);
        assert_eq!(expanded.text, "\\input{a}");
        let limits = InputLimits {
            max_depth: 0,
            ..InputLimits::default()
        };
        let expanded = expand("\\input{a}", None, &limits, &mut read);
        assert_eq!(expanded.text, "\\input{a}");
    }
}
