//! Math delimiters, row analysis, and preparation for KaTeX.
//!
//! Numbering is decided by the parser from the rows found here. Prepared TeX
//! uses unnumbered environments with an explicit `\tag{n}` on each numbered
//! row, so KaTeX's own numbering never applies.

use std::ops::Range;

use crate::{
    event::RefKind,
    lexer::{self, Kind, LexState},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delim {
    Dollar,
    DoubleDollar,
    Paren,
    Bracket,
}

/// Finds the closing delimiter of math that starts at `from`, returning the
/// delimiter's position. Escapes and comments are skipped; a blank line in
/// inline math, or a group closed before the delimiter, ends the search.
pub(crate) fn find_end(text: &str, from: usize, end: usize, delim: Delim) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = from;
    let mut depth = 0usize;
    let mut blank = false;
    while i < end {
        match bytes[i] {
            b'\\' if i + 1 < end => {
                let next = bytes[i + 1];
                if depth == 0
                    && ((delim == Delim::Paren && next == b')')
                        || (delim == Delim::Bracket && next == b']'))
                {
                    return Some(i);
                }
                i += 2;
                blank = false;
                continue;
            }
            b'%' => {
                i = bytes[i..end]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(end, |p| i + p);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            b'$' if depth == 0 => match delim {
                Delim::Dollar => return Some(i),
                Delim::DoubleDollar if bytes.get(i + 1) == Some(&b'$') => return Some(i),
                _ => {}
            },
            b'\n' => {
                if blank && matches!(delim, Delim::Dollar | Delim::Paren) {
                    return None;
                }
                blank = true;
                i += 1;
                continue;
            }
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        blank = false;
        i += 1;
    }
    None
}

/// Finds `\end{name}` matching an environment whose body starts at `from`,
/// returning the body end and the position after `\end{name}`.
pub(crate) fn find_env_end(
    text: &str,
    from: usize,
    end: usize,
    name: &str,
) -> Option<(usize, usize)> {
    let mut state = LexState {
        line_start: false,
        at_letter: false,
    };
    let mut pos = from;
    let mut depth = 0usize;
    loop {
        let token = lexer::next(text, pos, end, &mut state);
        match token.kind {
            Kind::Eof => return None,
            Kind::Word => {
                let word = &text[token.start + 1..token.name_end];
                if (word == "begin" || word == "end")
                    && let Some((env, after)) = braced(text, token.end, end)
                    && env.trim() == name
                {
                    if word == "begin" {
                        depth += 1;
                    } else if depth == 0 {
                        return Some((token.start, after));
                    } else {
                        depth -= 1;
                    }
                    pos = after;
                    continue;
                }
            }
            _ => {}
        }
        pos = token.end;
    }
}

/// `{...}` at `pos` after optional spaces: the content and the position after.
pub(crate) fn braced(text: &str, pos: usize, end: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    let mut i = pos;
    while i < end && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    if i >= end || bytes[i] != b'{' {
        return None;
    }
    let close = matching(text, i + 1, end, b'{', b'}')?;
    Some((&text[i + 1..close], close + 1))
}

/// The position of the delimiter closing a group whose content starts at
/// `from`, honoring escapes, comments, and nested braces.
pub(crate) fn matching(text: &str, from: usize, end: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut braces = 0usize;
    let mut i = from;
    while i < end {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'%' => {
                i = bytes[i..end]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(end, |p| i + p);
                continue;
            }
            b'{' if open != b'{' => braces += 1,
            b'}' if open != b'{' => braces = braces.saturating_sub(1),
            c if c == close && depth == 0 && braces == 0 => return Some(i),
            c if c == open => depth += 1,
            c if c == close && braces == 0 => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Row {
    /// Where an automatic `\tag` goes: the end of the row's content.
    pub end: usize,
    pub notag: bool,
    /// An explicit `\tag{...}`, and whether it was `\tag*`.
    pub tag: Option<String>,
    pub tag_star: bool,
    pub labels: Vec<String>,
    /// Only spaces and comments.
    pub empty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    Remove,
    Ref(RefKind, String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    pub range: Range<usize>,
    pub kind: EditKind,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Analysis {
    pub rows: Vec<Row>,
    pub edits: Vec<Edit>,
    /// Global definitions, such as `\gdef`, for the parser to apply.
    pub globals: Vec<Range<usize>>,
}

/// Scans a formula body. With `split`, rows end at top-level `\\`; `eqnarray`
/// also drops each row's second top-level `&`, reducing `&=&` to `&=`.
pub(crate) fn analyze(body: &str, split: bool, eqnarray: bool) -> Analysis {
    let mut analysis = Analysis::default();
    let mut state = LexState {
        line_start: false,
        at_letter: false,
    };
    let mut row = Row {
        empty: true,
        ..Row::default()
    };
    let mut ampersands = 0;
    let mut braces = 0usize;
    let mut envs = 0usize;
    let mut pos = 0;
    let end = body.len();
    loop {
        let token = lexer::next(body, pos, end, &mut state);
        pos = token.end;
        match token.kind {
            Kind::Eof => break,
            Kind::Space | Kind::Par => continue,
            Kind::Open => braces += 1,
            Kind::Close => braces = braces.saturating_sub(1),
            Kind::Align if braces == 0 && envs == 0 => {
                ampersands += 1;
                if eqnarray && ampersands == 2 {
                    analysis.edits.push(Edit {
                        range: token.start..token.end,
                        kind: EditKind::Remove,
                    });
                }
            }
            Kind::Symbol if &body[token.start..token.end] == "\\\\" => {
                if split && braces == 0 && envs == 0 {
                    row.end = token.start;
                    analysis.rows.push(std::mem::take(&mut row));
                    row.empty = true;
                    ampersands = 0;
                    pos = skip_row_options(body, pos);
                    continue;
                }
            }
            Kind::Word => {
                let name = &body[token.start + 1..token.name_end];
                match name {
                    "begin" | "end" => {
                        if let Some((_, after)) = braced(body, token.end, end) {
                            if name == "begin" {
                                envs += 1;
                            } else {
                                envs = envs.saturating_sub(1);
                            }
                            pos = after;
                        }
                    }
                    "notag" | "nonumber" => row.notag = true,
                    "tag" => {
                        let star = body[pos..].starts_with('*');
                        let at = if star { pos + 1 } else { pos };
                        if let Some((tag, after)) = braced(body, at, end) {
                            row.tag = Some(tag.trim().to_owned());
                            row.tag_star = star;
                            pos = after;
                        }
                    }
                    "label" => {
                        if let Some((label, after)) = braced(body, token.end, end) {
                            row.labels.push(label.trim().to_owned());
                            analysis.edits.push(Edit {
                                range: token.start..after,
                                kind: EditKind::Remove,
                            });
                            pos = after;
                            continue;
                        }
                    }
                    "ref" | "eqref" | "autoref" | "cref" | "Cref" | "pageref" => {
                        if let Some((label, after)) = braced(body, token.end, end) {
                            let kind = match name {
                                "eqref" => RefKind::Eqref,
                                "pageref" => RefKind::Page,
                                "ref" => RefKind::Ref,
                                _ => RefKind::Named,
                            };
                            analysis.edits.push(Edit {
                                range: token.start..after,
                                kind: EditKind::Ref(kind, label.trim().to_owned()),
                            });
                            pos = after;
                        }
                    }
                    "gdef" | "xdef" | "global" => {
                        let after = definition_end(body, pos);
                        analysis.globals.push(token.start..after);
                        pos = after;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        row.empty = false;
    }
    row.end = body.trim_end().len();
    analysis.rows.push(row);
    analysis.edits.sort_by_key(|edit| edit.range.start);
    analysis
}

fn skip_row_options(body: &str, mut pos: usize) -> usize {
    let bytes = body.as_bytes();
    if bytes.get(pos) == Some(&b'*') {
        pos += 1;
    }
    let mut look = pos;
    while look < bytes.len() && matches!(bytes[look], b' ' | b'\t') {
        look += 1;
    }
    if bytes.get(look) == Some(&b'[')
        && let Some(close) = matching(body, look + 1, body.len(), b'[', b']')
    {
        return close + 1;
    }
    pos
}

/// The end of `\gdef\name<params>{body}`, `\global\let\a\b`, and similar.
fn definition_end(body: &str, pos: usize) -> usize {
    let bytes = body.as_bytes();
    let mut i = pos;
    // Skip prefixes and the defining command for `\global\def` or `\global\let`.
    let mut state = LexState {
        line_start: false,
        at_letter: false,
    };
    loop {
        let token = lexer::next(body, i, body.len(), &mut state);
        match token.kind {
            Kind::Open => {
                return matching(body, token.end, body.len(), b'{', b'}')
                    .map_or(body.len(), |c| c + 1);
            }
            Kind::Word | Kind::Symbol | Kind::Param | Kind::Text | Kind::Space => {
                let word = &body[token.start..token.name_end];
                i = token.end;
                // `\global\let\a\b` ends after the second name.
                if word == "\\let" {
                    let first = lexer::next(body, i, body.len(), &mut state);
                    let mut after = first.end;
                    if bytes.get(after) == Some(&b'=') {
                        after += 1;
                    }
                    let second = lexer::next(body, after, body.len(), &mut state);
                    return second.end;
                }
            }
            _ => return token.end,
        }
    }
}

/// Applies edits, resolves references, and inserts automatic tags.
pub(crate) fn prepare(
    body: &str,
    analysis: &Analysis,
    tags: &[Option<String>],
    resolve: impl Fn(RefKind, &str) -> String,
) -> String {
    let mut inserts: Vec<(usize, String)> = analysis
        .rows
        .iter()
        .zip(tags)
        .filter_map(|(row, tag)| {
            tag.as_ref()
                .map(|tag| (row.end, format!(" \\tag*{{{tag}}}")))
        })
        .collect();
    inserts.sort_by_key(|(at, _)| *at);
    let mut out = String::with_capacity(body.len() + 16);
    let mut cursor = 0;
    let mut inserts = inserts.into_iter().peekable();
    for edit in &analysis.edits {
        while let Some((at, _)) = inserts.peek()
            && *at <= edit.range.start
        {
            let (at, text) = inserts.next().unwrap();
            out.push_str(&body[cursor..at]);
            out.push_str(&text);
            cursor = at;
        }
        if edit.range.start < cursor {
            continue;
        }
        out.push_str(&body[cursor..edit.range.start]);
        if let EditKind::Ref(kind, label) = &edit.kind {
            out.push_str(&format!("\\text{{{}}}", resolve(*kind, label)));
        }
        cursor = edit.range.end;
    }
    for (at, text) in inserts {
        if at >= cursor {
            out.push_str(&body[cursor..at]);
            cursor = at;
        }
        out.push_str(&text);
    }
    out.push_str(&body[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_delimiters_past_escapes_comments_and_groups() {
        let text = r"$a\$b % $ comment
 {c$}$ rest";
        assert_eq!(
            find_end(text, 1, text.len(), Delim::Dollar),
            Some(text.len() - 6)
        );
        assert_eq!(find_end("\\(x\\)", 2, 5, Delim::Paren), Some(3));
        assert_eq!(find_end("$a\n\nb$", 1, 6, Delim::Dollar), None);
        let env = r"x \begin{aligned} y \end{aligned} \end{align} z";
        let (body_end, after) = find_env_end(env, 0, env.len(), "align").unwrap();
        assert_eq!(&env[body_end..after], r"\end{align}");
    }

    #[test]
    fn rows_collect_labels_tags_and_notag() {
        let body = r" a &= b \notag \label{p} \\ c &= d \tag{x} \\[2pt] e \begin{aligned} f \\ g \end{aligned} ";
        let analysis = analyze(body, true, false);
        assert_eq!(analysis.rows.len(), 3);
        assert!(analysis.rows[0].notag);
        assert_eq!(analysis.rows[0].labels, ["p"]);
        assert_eq!(analysis.rows[1].tag.as_deref(), Some("x"));
        let prepared = prepare(
            body,
            &analysis,
            &[None, None, Some("(3)".into())],
            |_, l| l.into(),
        );
        assert!(!prepared.contains("\\label"));
        assert!(prepared.trim_end().ends_with(r"\end{aligned} \tag*{(3)}"));
    }

    #[test]
    fn eqnarray_rows_drop_the_second_ampersand_and_refs_resolve() {
        let body = r"a &=& b \eqref{e} \\ c &<& d";
        let analysis = analyze(body, true, true);
        let prepared = prepare(body, &analysis, &[None, None], |kind, label| match kind {
            RefKind::Eqref => format!("({label})"),
            _ => label.into(),
        });
        assert_eq!(prepared, r"a &= b \text{(e)} \\ c &< d");
    }
}
