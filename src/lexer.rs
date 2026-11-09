//! Tokens with byte ranges, following TeX's default category codes. Comments,
//! blank lines, and spaces after control words behave as in TeX, so token
//! boundaries match what a reader of the source expects.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A run of ordinary characters.
    Text,
    /// Spaces, tabs, and at most one line end.
    Space,
    /// A blank line, which ends a paragraph.
    Par,
    /// `\name`: the name ends at `name_end`; `end` includes skipped spaces.
    Word,
    /// `\` followed by one character.
    Symbol,
    Open,
    Close,
    LBracket,
    RBracket,
    Math,
    Align,
    Param,
    Super,
    Sub,
    Tilde,
    Eof,
}

/// Lexer state that survives between tokens, and so between checkpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LexState {
    /// Only spaces have been read since the last line end.
    pub line_start: bool,
    /// `@` is a letter, after `\makeatletter`.
    pub at_letter: bool,
}

impl Default for LexState {
    fn default() -> Self {
        Self {
            line_start: true,
            at_letter: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    pub name_end: usize,
}

impl Token {
    fn new(kind: Kind, start: usize, end: usize) -> Self {
        Self {
            kind,
            start,
            end,
            name_end: end,
        }
    }
}

const SPECIAL: [bool; 256] = {
    let mut table = [false; 256];
    let specials = b"\\{}[]$&#^_~% \t\r\n";
    let mut i = 0;
    while i < specials.len() {
        table[specials[i] as usize] = true;
        i += 1;
    }
    table
};

fn is_letter(byte: u8, at_letter: bool) -> bool {
    byte.is_ascii_alphabetic() || (at_letter && byte == b'@')
}

fn utf8_len(lead: u8) -> usize {
    match lead {
        0xf0.. => 4,
        0xe0.. => 3,
        0xc0.. => 2,
        _ => 1,
    }
}

/// Reads the next token of `text[..end]` starting at `pos`.
pub(crate) fn next(text: &str, mut pos: usize, end: usize, state: &mut LexState) -> Token {
    let bytes = text.as_bytes();
    loop {
        if pos >= end {
            return Token::new(Kind::Eof, end, end);
        }
        let start = pos;
        let kind = match bytes[pos] {
            b'%' => {
                pos = bytes[pos..end]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(end, |i| pos + i + 1);
                state.line_start = true;
                while pos < end && matches!(bytes[pos], b' ' | b'\t' | b'\r') {
                    pos += 1;
                }
                continue;
            }
            b' ' | b'\t' | b'\r' | b'\n' => {
                let was_line_start = state.line_start;
                let mut blank = false;
                let mut newline = false;
                while pos < end {
                    match bytes[pos] {
                        b' ' | b'\t' | b'\r' => pos += 1,
                        b'\n' => {
                            blank |= state.line_start;
                            state.line_start = true;
                            newline = true;
                            pos += 1;
                        }
                        _ => break,
                    }
                }
                if blank {
                    Kind::Par
                } else if !newline && was_line_start {
                    // Spaces at the start of a line are ignored.
                    continue;
                } else {
                    Kind::Space
                }
            }
            b'\\' => {
                state.line_start = false;
                pos += 1;
                if pos >= end {
                    return Token::new(Kind::Text, start, end);
                }
                if is_letter(bytes[pos], state.at_letter) {
                    while pos < end && is_letter(bytes[pos], state.at_letter) {
                        pos += 1;
                    }
                    let name_end = pos;
                    while pos < end && matches!(bytes[pos], b' ' | b'\t' | b'\r') {
                        pos += 1;
                    }
                    if pos < end && bytes[pos] == b'\n' {
                        pos += 1;
                        state.line_start = true;
                        while pos < end && matches!(bytes[pos], b' ' | b'\t' | b'\r') {
                            pos += 1;
                        }
                    }
                    return Token {
                        kind: Kind::Word,
                        start,
                        end: pos,
                        name_end,
                    };
                }
                pos = (pos + utf8_len(bytes[pos])).min(end);
                return Token::new(Kind::Symbol, start, pos);
            }
            byte => {
                state.line_start = false;
                pos += 1;
                match byte {
                    b'{' => Kind::Open,
                    b'}' => Kind::Close,
                    b'[' => Kind::LBracket,
                    b']' => Kind::RBracket,
                    b'$' => Kind::Math,
                    b'&' => Kind::Align,
                    b'#' => Kind::Param,
                    b'^' => Kind::Super,
                    b'_' => Kind::Sub,
                    b'~' => Kind::Tilde,
                    _ => {
                        while pos < end && !SPECIAL[bytes[pos] as usize] {
                            pos += 1;
                        }
                        Kind::Text
                    }
                }
            }
        };
        return Token::new(kind, start, pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(Kind, &str)> {
        let mut state = LexState::default();
        let mut pos = 0;
        let mut out = Vec::new();
        loop {
            let token = next(text, pos, text.len(), &mut state);
            if token.kind == Kind::Eof {
                return out;
            }
            out.push((token.kind, &text[token.start..token.end]));
            pos = token.end;
        }
    }

    #[test]
    fn comments_blank_lines_and_control_word_spaces_follow_tex() {
        use Kind::*;
        assert_eq!(kinds("a%c\nb"), [(Text, "a"), (Text, "b")]);
        assert_eq!(
            kinds("a\n%c\n\nb"),
            [(Text, "a"), (Space, "\n"), (Par, "\n"), (Text, "b")]
        );
        assert_eq!(
            kinds("a\n  \n b"),
            [(Text, "a"), (Par, "\n  \n "), (Text, "b")]
        );
        assert_eq!(kinds("\\LaTeX is"), [(Word, "\\LaTeX "), (Text, "is")]);
        assert_eq!(
            kinds("\\foo\n\nx"),
            [(Word, "\\foo\n"), (Par, "\n"), (Text, "x")]
        );
        assert_eq!(
            kinds("\\%x\\\\"),
            [(Symbol, "\\%"), (Text, "x"), (Symbol, "\\\\")]
        );
        assert_eq!(
            kinds("é[a]"),
            [(Text, "é"), (LBracket, "["), (Text, "a"), (RBracket, "]")]
        );
    }
}
