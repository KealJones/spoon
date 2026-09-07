//! SCE tokenizer and sentence splitter.

/// A single token from SCE text.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Word(String),    // any word (case-preserved)
    Number(f64),     // numeric literal
    Quoted(String),  // "quoted string"
    Path(String),    // /path/... or ~/...
    Url(String),     // http(s)://...
    Comma,
    Period,
    Bang,
    Question,
    AposS,   // 's (possessive)
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
}

/// Tokenize a single SCE sentence.
pub fn tokenize(text: &str) -> Vec<Tok> {
    let bytes = text.as_bytes();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        // Skip whitespace
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Decide whether prev token is a "value" (for minus sign vs negative number)
        let prev_is_value = matches!(
            out.last(),
            Some(Tok::Word(_))
                | Some(Tok::Number(_))
                | Some(Tok::RParen)
                | Some(Tok::Quoted(_))
                | Some(Tok::Path(_))
                | Some(Tok::Url(_))
        );

        match b {
            b',' => { out.push(Tok::Comma); i += 1; }
            b'!' => { out.push(Tok::Bang); i += 1; }
            b'?' => { out.push(Tok::Question); i += 1; }
            b'+' => { out.push(Tok::Plus); i += 1; }
            b'%' => { out.push(Tok::Percent); i += 1; }
            b'^' => { out.push(Tok::Caret); i += 1; }
            b'*' => { out.push(Tok::Star); i += 1; }
            b'(' => { out.push(Tok::LParen); i += 1; }
            b')' => { out.push(Tok::RParen); i += 1; }

            b'.' => {
                // Check for decimal in number context: digit . digit
                // That's handled in the digit branch. Here '.' is always a period.
                out.push(Tok::Period);
                i += 1;
            }

            b'"' => {
                i += 1; // skip opening quote
                let start = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                let s = std::str::from_utf8(&bytes[start..i]).unwrap_or("").to_string();
                out.push(Tok::Quoted(s));
                if i < bytes.len() { i += 1; } // skip closing quote
            }

            b'\'' => {
                i += 1;
                if i < bytes.len() && bytes[i] == b's' {
                    // Check next char: if space/punct then it's 's possessive
                    if i + 1 >= bytes.len()
                        || bytes[i + 1].is_ascii_whitespace()
                        || bytes[i + 1] == b','
                        || bytes[i + 1] == b'.'
                        || bytes[i + 1] == b'!'
                        || bytes[i + 1] == b'?'
                    {
                        out.push(Tok::AposS);
                        i += 1;
                    }
                    // else: skip (shouldn't occur in SCE)
                }
            }

            b'-' => {
                if !prev_is_value {
                    // Could be negative number
                    if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                        let start = i;
                        i += 1; // skip '-'
                        while i < bytes.len()
                            && (bytes[i].is_ascii_digit()
                                || (bytes[i] == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
                                || bytes[i] == b'e'
                                || bytes[i] == b'E')
                        {
                            i += 1;
                        }
                        let s = std::str::from_utf8(&bytes[start..i]).unwrap_or("0");
                        if let Ok(n) = s.parse::<f64>() {
                            out.push(Tok::Number(n));
                        } else {
                            out.push(Tok::Minus);
                        }
                    } else {
                        out.push(Tok::Minus);
                        i += 1;
                    }
                } else {
                    out.push(Tok::Minus);
                    i += 1;
                }
            }

            b'/' => {
                // Path if immediately followed by a letter or digit or ~
                let next = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
                if !prev_is_value && (next.is_ascii_alphanumeric() || next == b'~' || next == b'/') {
                    let end = atom_end(bytes, i);
                    out.push(Tok::Path(slice(bytes, i, end)));
                    i = end;
                } else {
                    out.push(Tok::Slash);
                    i += 1;
                }
            }

            b'~' => {
                let end = atom_end(bytes, i);
                out.push(Tok::Path(slice(bytes, i, end)));
                i = end;
            }

            b'0'..=b'9' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                // Check for decimal point followed by digits
                if i < bytes.len()
                    && bytes[i] == b'.'
                    && i + 1 < bytes.len()
                    && bytes[i + 1].is_ascii_digit()
                {
                    i += 1; // consume '.'
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                // Check for scientific notation
                if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                    i += 1;
                    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                        i += 1;
                    }
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let s = std::str::from_utf8(&bytes[start..i]).unwrap_or("0");
                if let Ok(n) = s.parse::<f64>() {
                    out.push(Tok::Number(n));
                }
            }

            _ if url_starts_at(bytes, i) => {
                let end = atom_end(bytes, i);
                out.push(Tok::Url(slice(bytes, i, end)));
                i = end;
            }

            _ if is_alpha_byte(b) => {
                let start = i;
                loop {
                    if i >= bytes.len() { break; }
                    let c = bytes[i];
                    if is_alpha_byte(c) || c.is_ascii_alphanumeric() {
                        i += 1;
                    } else if c == b'-' {
                        // Include hyphen if next char is also word-like
                        if i + 1 < bytes.len()
                            && (is_alpha_byte(bytes[i + 1]) || bytes[i + 1].is_ascii_alphanumeric())
                        {
                            i += 1; // include hyphen, continue
                        } else {
                            break;
                        }
                    } else if c == b'\'' {
                        // Possessive 's: end word here
                        break;
                    } else {
                        break;
                    }
                }
                let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("").to_string();
                // Detect possessive: next is 's
                if i < bytes.len() && bytes[i] == b'\'' && i + 1 < bytes.len() && bytes[i + 1] == b's' {
                    // Check if 's is at end of word (space/punct follows)
                    let after = if i + 2 < bytes.len() { bytes[i + 2] } else { 0 };
                    if after == 0
                        || after.is_ascii_whitespace()
                        || after == b','
                        || after == b'.'
                        || after == b'!'
                        || after == b'?'
                    {
                        out.push(Tok::Word(word));
                        out.push(Tok::AposS);
                        i += 2; // skip 's
                        continue;
                    }
                }
                out.push(Tok::Word(word));
            }

            _ => { i += 1; } // skip unknown characters
        }
    }

    out
}

fn is_alpha_byte(b: u8) -> bool {
    b.is_ascii_alphabetic() || b >= 0x80 // include UTF-8 continuation bytes
}

/// True when an absolute http(s) URL starts at `i`.
fn url_starts_at(bytes: &[u8], i: usize) -> bool {
    let rest = &bytes[i..];
    let scheme = |s: &[u8]| rest.len() > s.len() && rest[..s.len()].eq_ignore_ascii_case(s);
    scheme(b"http://") || scheme(b"https://")
}

/// End of a URL or path token that starts at `start`. The atom runs to the
/// next space, comma, or quote; sentence terminators sitting at the very end
/// belong to the sentence, so `.../todos!` yields the URL without the `!` and
/// the caller tokenizes the `!` on the next pass. A `?` inside a query string
/// (`.../a?b=1`) is not at the end, so it stays.
fn atom_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len()
        && !bytes[end].is_ascii_whitespace()
        && bytes[end] != b','
        && bytes[end] != b'"'
    {
        end += 1;
    }
    while end > start && matches!(bytes[end - 1], b'.' | b'!' | b'?') {
        end -= 1;
    }
    end
}

fn slice(bytes: &[u8], start: usize, end: usize) -> String {
    std::str::from_utf8(&bytes[start..end]).unwrap_or("").to_string()
}

fn char_url_starts_at(chars: &[char], i: usize) -> bool {
    let scheme = |s: &str| {
        let n = s.chars().count();
        chars.len() > i + n && chars[i..i + n].iter().collect::<String>().eq_ignore_ascii_case(s)
    };
    scheme("http://") || scheme("https://")
}

/// `atom_end` over a char slice, for the sentence splitter.
fn char_atom_end(chars: &[char], start: usize) -> usize {
    let mut end = start;
    while end < chars.len() && !chars[end].is_whitespace() && chars[end] != ',' && chars[end] != '"' {
        end += 1;
    }
    while end > start && matches!(chars[end - 1], '.' | '!' | '?') {
        end -= 1;
    }
    end
}

/// Split text into individual SCE sentences on `.`, `!`, `?` terminators.
/// Respects quoted strings and does not split on decimal points in numbers.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        let c = chars[i];
        if c == '"' {
            in_quote = !in_quote;
            current.push(c);
            i += 1;
            continue;
        }
        // A URL carries its own dots and question marks: `typicode.com/a?b=1`
        // is one token, not three sentences.
        if !in_quote && char_url_starts_at(&chars, i) {
            let end = char_atom_end(&chars, i);
            current.extend(&chars[i..end]);
            i = end;
            continue;
        }
        if !in_quote && (c == '.' || c == '!' || c == '?') {
            // For '.': check this is not a decimal point
            if c == '.' {
                let prev_is_digit = i > 0 && chars[i - 1].is_ascii_digit();
                let next_is_digit = i + 1 < n && chars[i + 1].is_ascii_digit();
                if prev_is_digit && next_is_digit {
                    // Decimal point in a number: keep in current sentence
                    current.push(c);
                    i += 1;
                    continue;
                }
            }
            current.push(c);
            i += 1;
            // Skip whitespace after terminator
            while i < n && chars[i].is_whitespace() {
                i += 1;
            }
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current = String::new();
        } else {
            current.push(c);
            i += 1;
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        sentences.push(trimmed);
    }

    sentences
}

/// Source-like text of a token, for error messages.
impl std::fmt::Display for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tok::Word(w) | Tok::Path(w) | Tok::Url(w) => f.write_str(w),
            Tok::Number(n) => write!(f, "{n}"),
            Tok::Quoted(s) => write!(f, "\"{s}\""),
            Tok::Comma => f.write_str(","),
            Tok::Period => f.write_str("."),
            Tok::Bang => f.write_str("!"),
            Tok::Question => f.write_str("?"),
            Tok::AposS => f.write_str("'s"),
            Tok::Plus => f.write_str("+"),
            Tok::Minus => f.write_str("-"),
            Tok::Star => f.write_str("*"),
            Tok::Slash => f.write_str("/"),
            Tok::Percent => f.write_str("%"),
            Tok::Caret => f.write_str("^"),
            Tok::LParen => f.write_str("("),
            Tok::RParen => f.write_str(")"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_basic() {
        let s = "John owns a dog. Mary likes cats!";
        let sents = split_sentences(s);
        assert_eq!(sents, vec!["John owns a dog.", "Mary likes cats!"]);
    }

    #[test]
    fn split_preserves_decimal() {
        let s = "There are 3.5 cats.";
        let sents = split_sentences(s);
        assert_eq!(sents.len(), 1);
    }

    #[test]
    fn tokenize_basic() {
        let toks = tokenize("John owns a dog.");
        assert!(matches!(toks[0], Tok::Word(ref w) if w == "John"));
        assert!(matches!(toks.last(), Some(Tok::Period)));
    }

    #[test]
    fn tokenize_possessive() {
        let toks = tokenize("Mary's dog");
        assert_eq!(toks.len(), 3);
        assert!(matches!(&toks[1], Tok::AposS));
    }

    fn url_and_tail(text: &str) -> (String, Vec<Tok>) {
        let toks = tokenize(text);
        match toks.split_first() {
            Some((Tok::Url(u), rest)) => (u.clone(), rest.to_vec()),
            other => panic!("expected a Url token first, got {other:?}"),
        }
    }

    #[test]
    fn url_keeps_its_query_and_drops_the_sentence_terminator() {
        assert_eq!(
            url_and_tail("https://jsonplaceholder.typicode.com/todos!"),
            ("https://jsonplaceholder.typicode.com/todos".into(), vec![Tok::Bang])
        );
        assert_eq!(
            url_and_tail("https://jsonplaceholder.typicode.com/todos?"),
            ("https://jsonplaceholder.typicode.com/todos".into(), vec![Tok::Question])
        );
        assert_eq!(
            url_and_tail("https://example.com/a.json."),
            ("https://example.com/a.json".into(), vec![Tok::Period])
        );
        assert_eq!(
            url_and_tail("https://x.com/a?b=1!"),
            ("https://x.com/a?b=1".into(), vec![Tok::Bang])
        );
    }

    #[test]
    fn url_inside_quotes_keeps_its_terminator() {
        let toks = tokenize("Assistant, http-get-json \"https://x.com/a!\"!");
        assert!(
            toks.contains(&Tok::Quoted("https://x.com/a!".into())),
            "quoted URL must be untouched, got {toks:?}"
        );
        assert_eq!(toks.last(), Some(&Tok::Bang));
    }

    #[test]
    fn path_keeps_its_extension() {
        assert_eq!(
            tokenize("/tmp/a.json."),
            vec![Tok::Path("/tmp/a.json".into()), Tok::Period]
        );
    }

    #[test]
    fn url_does_not_split_a_sentence() {
        assert_eq!(
            split_sentences("Assistant, get https://jsonplaceholder.typicode.com/todos! Who is John?"),
            vec!["Assistant, get https://jsonplaceholder.typicode.com/todos!", "Who is John?"]
        );
    }

    #[test]
    fn tokenize_arith() {
        let toks = tokenize("(3 + 5) * 2");
        // LParen Number Plus Number RParen Star Number
        assert!(matches!(toks[0], Tok::LParen));
        assert!(matches!(toks[1], Tok::Number(n) if n == 3.0));
    }
}
