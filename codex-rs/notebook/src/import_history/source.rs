//! Static-literal inventory, matching Pi's deliberately non-evaluating scanner.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::MAX_SPECIFIER_BYTES;

static SPECIFIER: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r#"^npm:(?:@[^/\s]+/[^@/\s]+|[^@/\s]+)@[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?(?:/[^\s]+)?$"#,
    )
});
static IMPORT_PREFIX: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^A-Za-z0-9_$.])import\s*(?:\(\s*)?$|(?:^|[^A-Za-z0-9_$])from\s*$")
});
static REGEX_PREFIX: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[^A-Za-z0-9_$])(return|throw|case|delete|void|typeof|instanceof|in|of|yield|await|else|do)$",
    )
});

fn pattern(regex: &Result<Regex, regex::Error>) -> Result<&Regex, String> {
    regex
        .as_ref()
        .map_err(|error| format!("Invalid notebook import inventory pattern: {error}"))
}

pub(super) fn exact_specifier(value: &str) -> Result<bool, String> {
    Ok(value.len() <= MAX_SPECIFIER_BYTES
        && !value.chars().any(|c| matches!(c, '\'' | '"' | '`'))
        && pattern(&SPECIFIER)?.is_match(value))
}

pub(crate) fn extract(source: &str) -> Result<BTreeSet<String>, String> {
    let import_prefix = pattern(&IMPORT_PREFIX)?;
    let regex_prefix = pattern(&REGEX_PREFIX)?;
    let bytes = source.as_bytes();
    let mut masked = bytes.to_vec();
    let mut imports = BTreeSet::new();
    let mut index = 0;
    while index < bytes.len() {
        let current = bytes[index];
        let next = bytes.get(index + 1).copied();
        let stop = if current == b'/' && next == Some(b'/') {
            bytes[index..]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |end| index + end)
        } else if current == b'/' && next == Some(b'*') {
            bytes[index + 2..]
                .windows(2)
                .position(|b| b == b"*/")
                .map_or(bytes.len(), |end| index + end + 4)
        } else if current == b'/' && regex_start(source, index, regex_prefix) {
            regex_end(bytes, index)
        } else if matches!(current, b'\'' | b'"' | b'`') {
            let (stop, value) = literal(source, index, current);
            if let Some(value) = value
                && exact_specifier(&value)?
                && std::str::from_utf8(&masked[..index])
                    .is_ok_and(|prefix| import_prefix.is_match(prefix.trim_end()))
            {
                imports.insert(value);
            }
            stop
        } else {
            index += 1;
            continue;
        };
        for byte in &mut masked[index..stop] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        index = stop;
    }
    Ok(imports)
}

fn regex_start(source: &str, index: usize, regex_prefix: &Regex) -> bool {
    let prefix = source[..index].trim_end();
    prefix.is_empty()
        || prefix.ends_with(|c| "([{:;,=!?&|+-*%^~<>".contains(c))
        || regex_prefix.is_match(prefix)
}

fn regex_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 1;
    let mut class = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'[' => {
                class = true;
                index += 1;
            }
            b']' => {
                class = false;
                index += 1;
            }
            b'/' if !class => {
                index += 1;
                break;
            }
            _ => index += 1,
        }
    }
    while bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
        index += 1;
    }
    index
}

fn literal(source: &str, start: usize, delimiter: u8) -> (usize, Option<String>) {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    let mut dynamic = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            c if c == delimiter => {
                return (
                    index + 1,
                    if dynamic {
                        None
                    } else {
                        decode(&source[start + 1..index])
                    },
                );
            }
            b'$' if delimiter == b'`' && bytes.get(index + 1) == Some(&b'{') => {
                dynamic = true;
                index += 1;
            }
            b'\n' | b'\r' if delimiter != b'`' => {
                dynamic = true;
                index += 1;
            }
            _ => index += 1,
        }
    }
    (bytes.len(), None)
}

fn decode(raw: &str) -> Option<String> {
    let mut chars = raw.chars().peekable();
    let mut value = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            value.push(c);
            continue;
        }
        match chars.next()? {
            '\n' => {}
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            'b' => value.push('\u{8}'),
            'f' => value.push('\u{c}'),
            'n' => value.push('\n'),
            'r' => value.push('\r'),
            't' => value.push('\t'),
            'v' => value.push('\u{b}'),
            '0' if !chars.peek().is_some_and(char::is_ascii_digit) => value.push('\0'),
            '0'..='9' => return None,
            'x' => value.push(hex_point(&mut chars, 2)?),
            'u' if chars.peek() == Some(&'{') => {
                chars.next();
                let mut hex = String::new();
                loop {
                    match chars.next()? {
                        '}' => break,
                        c if c.is_ascii_hexdigit() && hex.len() < 6 => hex.push(c),
                        _ => return None,
                    }
                }
                value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            'u' => value.push(hex_point(&mut chars, 4)?),
            c => value.push(c),
        }
    }
    Some(value)
}

fn hex_point(chars: &mut impl Iterator<Item = char>, count: usize) -> Option<char> {
    let mut point = 0;
    for _ in 0..count {
        let c = chars.next()?;
        if !c.is_ascii_hexdigit() {
            return None;
        }
        point = point * 16 + c.to_digit(16)?;
    }
    char::from_u32(point)
}
