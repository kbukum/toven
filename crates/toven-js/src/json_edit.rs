//! Format-preserving edits to JSON text.
//!
//! Release rewrites a few string values in `package.json` (the `version` and
//! in-repo dependency ranges). Re-serializing the whole document would reorder
//! keys and change indentation, so this module parses the text into a tree of
//! byte spans and replaces only the edited spans, leaving every other byte
//! untouched. The edited text is re-parsed as a guard before it is returned.

use std::ops::Range;

use rskit_errors::{AppError, AppResult, ErrorCode};

/// Nesting bound, so a hostile manifest cannot exhaust the stack.
const MAX_DEPTH: usize = 128;

/// A parsed JSON value with the byte span it occupies in the source text.
#[derive(Debug)]
pub(crate) enum Node {
    /// An object's members in source order.
    Object(Vec<Member>),
    /// A string with its decoded value.
    String(String, Range<usize>),
    /// Any other value (number, literal, array).
    Other,
}

/// One object member: its key, the span of its key literal, and its value.
#[derive(Debug)]
pub(crate) struct Member {
    /// The decoded key.
    pub(crate) key: String,
    /// The span of the key literal, quotes included.
    pub(crate) key_span: Range<usize>,
    /// The member's value.
    pub(crate) value: Node,
    /// The span of the value.
    pub(crate) value_span: Range<usize>,
}

impl Node {
    /// The members of an object, or `None` for any other value.
    pub(crate) fn members(&self) -> Option<&[Member]> {
        match self {
            Self::Object(members) => Some(members),
            _ => None,
        }
    }

    /// The member named `key` of an object.
    pub(crate) fn get(&self, key: &str) -> Option<&Member> {
        self.members()?.iter().find(|member| member.key == key)
    }

    /// The decoded string value and its span, or `None` for any non-string.
    pub(crate) fn as_str(&self) -> Option<(&str, Range<usize>)> {
        match self {
            Self::String(value, span) => Some((value, span.clone())),
            _ => None,
        }
    }
}

/// Parse `text` into a span tree.
///
/// # Errors
/// [`ErrorCode::InvalidFormat`] when `text` is not one JSON value or nests
/// deeper than the bound.
pub(crate) fn parse(text: &str) -> AppResult<Node> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        text,
        at: 0,
    };
    parser.whitespace();
    let (node, _) = parser.value(0)?;
    parser.whitespace();
    if parser.at != parser.bytes.len() {
        return Err(parser.error("trailing content after the JSON value"));
    }
    Ok(node)
}

/// A pending replacement of one byte span.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct Edit {
    span: Range<usize>,
    replacement: String,
}

impl Edit {
    /// Replace the string literal at `span` with `value`, JSON-encoded.
    pub(crate) fn string(span: Range<usize>, value: &str) -> Self {
        Self {
            span,
            replacement: encode_string(value),
        }
    }

    /// Insert raw `text` at byte offset `at`.
    pub(crate) const fn insert(at: usize, text: String) -> Self {
        Self {
            span: at..at,
            replacement: text,
        }
    }
}

/// Insert the string member `key: value` after the object member `after`,
/// copying the whitespace before `after`'s key and around its colon so the
/// new member matches the file's style.
pub(crate) fn insert_member_after(text: &str, after: &Member, key: &str, value: &str) -> Edit {
    let before_key = &text[..after.key_span.start];
    let indent = &before_key[before_key.trim_end().len()..];
    let colon = &text[after.key_span.end..after.value_span.start];
    Edit::insert(
        after.value_span.end,
        format!(
            ",{indent}{}{colon}{}",
            encode_string(key),
            encode_string(value)
        ),
    )
}

/// Apply `edits` to `text` and re-parse the result as a guard.
///
/// # Errors
/// [`ErrorCode::Internal`] when two edits overlap or the result is not valid
/// JSON — both are bugs in the caller, never a property of the input.
pub(crate) fn apply(text: &str, mut edits: Vec<Edit>) -> AppResult<String> {
    edits.sort_by_key(|edit| (edit.span.start, edit.span.end));
    if edits
        .windows(2)
        .any(|pair| pair[0].span.end > pair[1].span.start)
    {
        return Err(AppError::new(ErrorCode::Internal, "overlapping JSON edits"));
    }
    let mut out = text.to_string();
    for edit in edits.iter().rev() {
        out.replace_range(edit.span.clone(), &edit.replacement);
    }
    parse(&out).map_err(|error| {
        AppError::new(ErrorCode::Internal, "a JSON edit produced invalid JSON").with_cause(error)
    })?;
    Ok(out)
}

fn encode_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                // Controls are below 0x20, so the escape is `\u00` plus two hex digits.
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let code = u32::from(control) as usize;
                out.push_str("\\u00");
                out.push(char::from(HEX[code >> 4]));
                out.push(char::from(HEX[code & 0xf]));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> AppError {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("invalid JSON at byte {}: {message}", self.at),
        )
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> AppResult<()> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", char::from(byte))))
        }
    }

    fn value(&mut self, depth: usize) -> AppResult<(Node, Range<usize>)> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting exceeds the depth bound"));
        }
        let start = self.at;
        let node = match self.peek() {
            Some(b'{') => self.object(depth)?,
            Some(b'[') => {
                self.array(depth)?;
                Node::Other
            }
            Some(b'"') => {
                let (value, span) = self.string()?;
                Node::String(value, span)
            }
            Some(b'-' | b'0'..=b'9') => {
                self.number()?;
                Node::Other
            }
            Some(b't') => self.literal("true")?,
            Some(b'f') => self.literal("false")?,
            Some(b'n') => self.literal("null")?,
            _ => return Err(self.error("expected a JSON value")),
        };
        Ok((node, start..self.at))
    }

    fn object(&mut self, depth: usize) -> AppResult<Node> {
        self.expect(b'{')?;
        let mut members = Vec::new();
        self.whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Node::Object(members));
        }
        loop {
            self.whitespace();
            let (key, key_span) = self.string()?;
            self.whitespace();
            self.expect(b':')?;
            self.whitespace();
            let (value, value_span) = self.value(depth + 1)?;
            members.push(Member {
                key,
                key_span,
                value,
                value_span,
            });
            self.whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Node::Object(members));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> AppResult<()> {
        self.expect(b'[')?;
        self.whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(());
        }
        loop {
            self.whitespace();
            self.value(depth + 1)?;
            self.whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(());
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn string(&mut self) -> AppResult<(String, Range<usize>)> {
        let start = self.at;
        self.expect(b'"')?;
        let mut value = String::new();
        loop {
            let rest = &self.text[self.at..];
            let Some(offset) = rest.find(['"', '\\']) else {
                return Err(self.error("unterminated string"));
            };
            let chunk = &rest[..offset];
            if chunk.bytes().any(|byte| byte < 0x20) {
                return Err(self.error("control character in string"));
            }
            value.push_str(chunk);
            self.at += offset;
            if self.peek() == Some(b'"') {
                self.at += 1;
                return Ok((value, start..self.at));
            }
            self.at += 1;
            let escape = self
                .peek()
                .ok_or_else(|| self.error("unterminated escape"))?;
            self.at += 1;
            match escape {
                b'"' => value.push('"'),
                b'\\' => value.push('\\'),
                b'/' => value.push('/'),
                b'b' => value.push('\u{8}'),
                b'f' => value.push('\u{c}'),
                b'n' => value.push('\n'),
                b'r' => value.push('\r'),
                b't' => value.push('\t'),
                b'u' => value.push(self.unicode_escape()?),
                _ => return Err(self.error("invalid escape")),
            }
        }
    }

    fn hex4(&mut self) -> AppResult<u32> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .ok_or_else(|| self.error("short unicode escape"))?;
        let code = u32::from_str_radix(digits, 16).map_err(|_| self.error("bad unicode escape"))?;
        self.at += 4;
        Ok(code)
    }

    fn unicode_escape(&mut self) -> AppResult<char> {
        let high = self.hex4()?;
        if (0xD800..0xDC00).contains(&high) {
            if self.text.get(self.at..self.at + 2) != Some("\\u") {
                return Err(self.error("unpaired surrogate"));
            }
            self.at += 2;
            let low = self.hex4()?;
            if !(0xDC00..0xE000).contains(&low) {
                return Err(self.error("unpaired surrogate"));
            }
            let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
            return char::from_u32(code).ok_or_else(|| self.error("bad unicode escape"));
        }
        char::from_u32(high).ok_or_else(|| self.error("unpaired surrogate"))
    }

    fn number(&mut self) -> AppResult<()> {
        let start = self.at;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.at += 1;
        }
        let token = &self.text[start..self.at];
        if token.parse::<f64>().is_err() {
            return Err(self.error("invalid number"));
        }
        Ok(())
    }

    fn literal(&mut self, word: &str) -> AppResult<Node> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(Node::Other)
        } else {
            Err(self.error("invalid literal"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Edit, apply, insert_member_after, parse};

    #[test]
    fn an_inserted_member_copies_the_neighbour_style() {
        let pretty = "{\n  \"name\": \"a\",\n  \"private\": true\n}\n";
        let name = parse(pretty).unwrap();
        let edit = insert_member_after(pretty, name.get("name").unwrap(), "version", "1.0.0");
        assert_eq!(
            apply(pretty, vec![edit]).unwrap(),
            "{\n  \"name\": \"a\",\n  \"version\": \"1.0.0\",\n  \"private\": true\n}\n"
        );
        let compact = "{\"name\":\"a\"}";
        let name = parse(compact).unwrap();
        let edit = insert_member_after(compact, name.get("name").unwrap(), "version", "1.0.0");
        assert_eq!(
            apply(compact, vec![edit]).unwrap(),
            "{\"name\":\"a\",\"version\":\"1.0.0\"}"
        );
    }

    const MANIFEST: &str = "{\n  \"name\": \"@acme/app\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\n    \"@acme/core\": \"^1.0.0\",\n    \"left-pad\":   \"1.3.0\"\n  },\n  \"files\": [\"dist\", 1, true, null, -2.5e3]\n}\n";

    #[test]
    fn spans_address_the_exact_literals() {
        let tree = parse(MANIFEST).expect("parse");
        let (version, span) = tree.get("version").unwrap().value.as_str().unwrap();
        assert_eq!(version, "1.0.0");
        assert_eq!(&MANIFEST[span], "\"1.0.0\"");
        let deps = &tree.get("dependencies").unwrap().value;
        let (range, span) = deps.get("@acme/core").unwrap().value.as_str().unwrap();
        assert_eq!(range, "^1.0.0");
        assert_eq!(&MANIFEST[span], "\"^1.0.0\"");
        assert!(tree.get("files").unwrap().value.as_str().is_none());
    }

    #[test]
    fn edits_preserve_every_other_byte() {
        let tree = parse(MANIFEST).unwrap();
        let version = tree.get("version").unwrap().value.as_str().unwrap().1;
        let core = tree
            .get("dependencies")
            .unwrap()
            .value
            .get("@acme/core")
            .unwrap();
        let range = core.value.as_str().unwrap().1;
        let edited = apply(
            MANIFEST,
            vec![
                Edit::string(range, "^1.1.0"),
                Edit::string(version, "1.1.0"),
            ],
        )
        .unwrap();
        assert_eq!(
            edited,
            MANIFEST
                .replace("\"version\": \"1.0.0\"", "\"version\": \"1.1.0\"")
                .replace("\"^1.0.0\"", "\"^1.1.0\"")
        );
    }

    #[test]
    fn decodes_escapes_and_encodes_on_write() {
        let text = r#"{"a": "x\"y\u00e9\ud83d\ude00\/"}"#;
        let tree = parse(text).unwrap();
        let (value, span) = tree.get("a").unwrap().value.as_str().unwrap();
        assert_eq!(value, "x\"y\u{e9}\u{1f600}/");
        let edited = apply(text, vec![Edit::string(span, "q\"\n")]).unwrap();
        assert_eq!(edited, r#"{"a": "q\"\n"}"#);
    }

    #[test]
    fn rejects_malformed_and_deep_input() {
        for bad in [
            "",
            "{",
            "{\"a\" 1}",
            "{\"a\": 1,}",
            "[1 2]",
            "{} x",
            "\"\u{1}\"",
            "nul",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let error = parse(&deep).unwrap_err();
        assert!(error.to_string().contains("depth"), "{error}");
    }

    #[test]
    fn overlapping_edits_are_a_bug() {
        let error = apply(
            "\"abc\"",
            vec![Edit::string(0..5, "x"), Edit::string(1..2, "y")],
        )
        .unwrap_err();
        assert_eq!(error.code(), rskit_errors::ErrorCode::Internal);
    }
}
