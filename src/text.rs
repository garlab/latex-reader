//! Text-mode conversions: ligatures, accents, and number formats.

use std::borrow::Cow;

/// TeX's text ligatures for dashes and quotes.
pub(crate) fn ligatures(text: &str) -> Cow<'_, str> {
    if !text
        .bytes()
        .any(|b| matches!(b, b'-' | b'`' | b'\'' | b'!' | b'?'))
    {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '-' if chars.peek() == Some(&'-') => {
                chars.next();
                if chars.peek() == Some(&'-') {
                    chars.next();
                    out.push('—');
                } else {
                    out.push('–');
                }
            }
            '`' if chars.peek() == Some(&'`') => {
                chars.next();
                out.push('“');
            }
            '\'' if chars.peek() == Some(&'\'') => {
                chars.next();
                out.push('”');
            }
            '`' => out.push('‘'),
            '\'' => out.push('’'),
            '!' if chars.peek() == Some(&'`') => {
                chars.next();
                out.push('¡');
            }
            '?' if chars.peek() == Some(&'`') => {
                chars.next();
                out.push('¿');
            }
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Combining marks for TeX's accent commands.
pub(crate) fn accent_mark(name: &str) -> Option<char> {
    Some(match name {
        "'" => '\u{301}',
        "`" => '\u{300}',
        "^" => '\u{302}',
        "\"" => '\u{308}',
        "~" => '\u{303}',
        "=" => '\u{304}',
        "." => '\u{307}',
        "u" => '\u{306}',
        "v" => '\u{30c}',
        "H" => '\u{30b}',
        "c" => '\u{327}',
        "k" => '\u{328}',
        "r" => '\u{30a}',
        "d" => '\u{323}',
        "b" => '\u{331}',
        "t" => '\u{361}',
        _ => return None,
    })
}

/// Applies an accent to a base letter, preferring a precomposed character.
pub(crate) fn accent(base: &str, mark: char) -> String {
    let base = match base {
        "\\i" | "i" if mark != '\u{327}' => "ı",
        "\\j" => "ȷ",
        other => other,
    };
    let mut chars = base.chars();
    let (Some(first), rest) = (chars.next(), chars.as_str()) else {
        return mark.to_string();
    };
    let first = if first == 'ı' { 'i' } else { first };
    let composed = PRECOMPOSED
        .iter()
        .find(|(m, letters)| *m == mark && letters.contains(first))
        .and_then(|(_, letters)| {
            let index = letters.chars().position(|c| c == first)?;
            COMPOSED
                .iter()
                .find(|(m, _)| *m == mark)?
                .1
                .chars()
                .nth(index)
        });
    match composed {
        Some(c) => format!("{c}{rest}"),
        None => format!("{first}{mark}{rest}"),
    }
}

// Base letters and their precomposed forms, in the same order.
const PRECOMPOSED: &[(char, &str)] = &[
    ('\u{301}', "aeiouyAEIOUYcCnNsSzZ"),
    ('\u{300}', "aeiouAEIOU"),
    ('\u{302}', "aeiouAEIOUcCgGhHjJsSwWyY"),
    ('\u{308}', "aeiouyAEIOUY"),
    ('\u{303}', "anoANO"),
    ('\u{304}', "aeiouAEIOU"),
    ('\u{307}', "zZcCeEgGI"),
    ('\u{306}', "aAgGuU"),
    ('\u{30c}', "cCsSzZrReEnNdDtT"),
    ('\u{30b}', "oOuU"),
    ('\u{327}', "cCsStT"),
    ('\u{328}', "aAeE"),
    ('\u{30a}', "aAuU"),
];
const COMPOSED: &[(char, &str)] = &[
    ('\u{301}', "áéíóúýÁÉÍÓÚÝćĆńŃśŚźŹ"),
    ('\u{300}', "àèìòùÀÈÌÒÙ"),
    ('\u{302}', "âêîôûÂÊÎÔÛĉĈĝĜĥĤĵĴŝŜŵŴŷŶ"),
    ('\u{308}', "äëïöüÿÄËÏÖÜŸ"),
    ('\u{303}', "ãñõÃÑÕ"),
    ('\u{304}', "āēīōūĀĒĪŌŪ"),
    ('\u{307}', "żŻċĊėĖġĠİ"),
    ('\u{306}', "ăĂğĞŭŬ"),
    ('\u{30c}', "čČšŠžŽřŘěĚňŇďĎťŤ"),
    ('\u{30b}', "őŐűŰ"),
    ('\u{327}', "çÇşŞţŢ"),
    ('\u{328}', "ąĄęĘ"),
    ('\u{30a}', "åÅůŮ"),
];

pub(crate) fn roman(mut n: u32, upper: bool) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut out = String::new();
    for (value, digits) in [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ] {
        while n >= value {
            out.push_str(digits);
            n -= value;
        }
    }
    if upper { out.to_uppercase() } else { out }
}

/// `a`, `b`, …, `z`, then numbers, as LaTeX's `\alph` errors past 26.
pub(crate) fn alph(n: u32, upper: bool) -> String {
    match n {
        1..=26 => {
            let base = if upper { b'A' } else { b'a' };
            char::from(base + (n - 1) as u8).to_string()
        }
        _ => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(ligatures("a--b---c ``q'' it's"), "a–b—c “q” it’s");
        assert!(matches!(ligatures("plain"), Cow::Borrowed(_)));
        assert_eq!(accent("e", '\u{301}'), "é");
        assert_eq!(accent("\\i", '\u{308}'), "ï");
        assert_eq!(accent("q", '\u{301}'), "q\u{301}");
        assert_eq!(roman(14, true), "XIV");
        assert_eq!(alph(2, false), "b");
    }
}
