//! Reader for the annotated hex golden files.
//!
//! Storing goldens as opaque `.bin` blobs makes a byte change unreviewable in a diff.
//! These files are hex with comments instead, so a pull request shows exactly which
//! printer command moved.
//!
//! Grammar: `1B 40` raw bytes, `"text"` ASCII runs, `20*22` a repeated byte,
//! `#` comment to end of line.

use std::path::PathBuf;

pub fn load(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()));
    parse(&text).unwrap_or_else(|e| panic!("parse golden {}: {e}", path.display()))
}

pub fn assert_bytes_eq(expected: &[u8], actual: &[u8]) {
    if expected == actual {
        return;
    }
    let at = expected
        .iter()
        .zip(actual)
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| expected.len().min(actual.len()));
    panic!(
        "byte streams differ at offset {at}\n  expected: {:02X?}\n    actual: {:02X?}\n\
         expected len {}, actual len {}",
        window(expected, at),
        window(actual, at),
        expected.len(),
        actual.len()
    );
}

fn window(bytes: &[u8], at: usize) -> &[u8] {
    let start = at.saturating_sub(8);
    let end = (at + 8).min(bytes.len());
    &bytes[start..end]
}

fn parse(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = strip_comment(raw);
        for token in tokenize(line) {
            append(&mut out, &token).map_err(|e| format!("line {}: {e}", lineno + 1))?;
        }
    }
    Ok(out)
}

/// Strips a trailing `#` comment, leaving `#` inside a quoted string alone.
fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn append(out: &mut Vec<u8>, token: &str) -> Result<(), String> {
    if let Some(inner) = token.strip_prefix('"') {
        let inner = inner
            .strip_suffix('"')
            .ok_or_else(|| format!("unterminated string {token}"))?;
        out.extend_from_slice(inner.as_bytes());
        return Ok(());
    }
    if let Some((byte, count)) = token.split_once('*') {
        let byte = hex(byte)?;
        let count: usize = count
            .parse()
            .map_err(|_| format!("bad repeat count in {token}"))?;
        out.extend(std::iter::repeat_n(byte, count));
        return Ok(());
    }
    out.push(hex(token)?);
    Ok(())
}

fn hex(token: &str) -> Result<u8, String> {
    u8::from_str_radix(token, 16).map_err(|_| format!("bad hex byte {token}"))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn the_golden_grammar_reads_bytes_strings_repeats_and_comments() {
        let parsed = parse("1B 40 # init\n\"Hi\" 20*3 0A\n").expect("parse");

        assert_eq!(parsed, vec![0x1B, 0x40, b'H', b'i', 0x20, 0x20, 0x20, 0x0A]);
    }

    #[test]
    fn a_hash_inside_a_string_is_data_not_a_comment() {
        assert_eq!(parse("\"#1\" 0A").expect("parse"), vec![b'#', b'1', 0x0A]);
    }
}
