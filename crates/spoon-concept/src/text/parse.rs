//! Parsing the angle-bracket notation back into a [`Concept`].
//!
//! A tokenizer followed by a recursive descent parser. Both carry byte offsets
//! the whole way, because a parse error that cannot point at the character it
//! choked on is barely better than no error at all.
//!
//! The parser is deliberately the exact inverse of the renderer rather than a
//! generous superset. `A<1><2>` is rejected, for example, because the renderer
//! writes that concept as `(A<1>)<2>`: accepting both spellings would mean two
//! texts for one concept, and the notation is used as a fixture format where
//! that ambiguity costs more than it buys.

use chrono::{DateTime, Utc};

use crate::concept::Concept;
use crate::id::{Ground, SymbolId, SymbolTable};

/// Everything that can go wrong reading the notation.
///
/// Every variant carries the byte offset where the problem starts, so callers
/// can underline the exact spot in a REPL line or a seed file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("empty input: expected a concept")]
    Empty,

    #[error("expected {expected} at offset {offset}, found {found}")]
    Unexpected {
        offset: usize,
        expected: &'static str,
        found: String,
    },

    #[error("trailing input at offset {offset}: found {found} after a complete concept")]
    Trailing { offset: usize, found: String },

    #[error("unterminated text literal starting at offset {offset}: missing closing '\"'")]
    UnterminatedText { offset: usize },

    #[error("unknown escape '\\{ch}' at offset {offset}: expected one of \\\" \\\\ \\n \\r \\t")]
    BadEscape { offset: usize, ch: char },

    #[error("unterminated {kind} literal starting at offset {offset}: missing closing '{close}'")]
    UnterminatedJson {
        offset: usize,
        kind: &'static str,
        close: char,
    },

    #[error("invalid json literal at offset {offset}: {reason}")]
    BadJson { offset: usize, reason: String },

    #[error("invalid number literal \"{text}\" at offset {offset}: {reason}")]
    BadNumber {
        offset: usize,
        text: String,
        reason: &'static str,
    },

    #[error("invalid bytes literal \"{text}\" at offset {offset}: {reason}")]
    BadBytes {
        offset: usize,
        text: String,
        reason: &'static str,
    },

    #[error("invalid hole at offset {offset}: {reason}")]
    BadHole { offset: usize, reason: &'static str },

    #[error("invalid symbol literal \"{text}\" at offset {offset}: expected '#' and 16 hex digits")]
    BadSymbol { offset: usize, text: String },

    #[error("unexpected character '{ch}' at offset {offset}")]
    BadChar { offset: usize, ch: char },
}

impl ParseError {
    /// Byte offset into the input where the problem starts.
    ///
    /// Exposed as one accessor so callers can build a caret line without
    /// matching on every variant.
    pub fn offset(&self) -> usize {
        match self {
            ParseError::Empty => 0,
            ParseError::Unexpected { offset, .. }
            | ParseError::Trailing { offset, .. }
            | ParseError::UnterminatedText { offset }
            | ParseError::BadEscape { offset, .. }
            | ParseError::UnterminatedJson { offset, .. }
            | ParseError::BadJson { offset, .. }
            | ParseError::BadNumber { offset, .. }
            | ParseError::BadBytes { offset, .. }
            | ParseError::BadHole { offset, .. }
            | ParseError::BadSymbol { offset, .. }
            | ParseError::BadChar { offset, .. } => *offset,
        }
    }
}

/// Read a concept from the angle-bracket notation.
///
/// Names encountered along the way are interned into `table`, so a concept
/// read from a seed file or a REPL line can immediately be printed back with
/// its names intact. Interning is the only mutation: parsing never consults
/// the table for identity, since [`SymbolId`] is a pure function of the name.
pub fn parse(input: &str, table: &SymbolTable) -> Result<Concept, ParseError> {
    let tokens = lex(input)?;
    if tokens.len() == 1 {
        return Err(ParseError::Empty);
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        table,
    };
    let concept = parser.term()?;
    let tok = parser.here();
    if !matches!(tok.kind, TokKind::Eof) {
        return Err(ParseError::Trailing {
            offset: tok.offset,
            found: tok.found(),
        });
    }
    Ok(concept)
}

// ---- tokens ----

#[derive(Debug, Clone)]
enum TokKind {
    Open,
    Close,
    Comma,
    LParen,
    RParen,
    Name(String),
    Symbol(SymbolId),
    Hole(u32),
    Value(Ground),
    Eof,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: TokKind,
    offset: usize,
    lexeme: String,
}

impl Tok {
    /// How this token appears in an error message. Real tokens are quoted so
    /// `found "Keal"` reads unambiguously; end of input is not.
    fn found(&self) -> String {
        match self.kind {
            TokKind::Eof => "end of input".to_string(),
            _ => format!("{:?}", self.lexeme),
        }
    }
}

// ---- lexer ----

struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer {
            src,
            chars: src.char_indices().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).map(|(_, c)| *c)
    }

    /// Byte offset of the next character, or the end of input.
    fn offset(&self) -> usize {
        match self.chars.get(self.pos) {
            Some((off, _)) => *off,
            None => self.src.len(),
        }
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn take_while(&mut self, pred: impl Fn(char) -> bool) -> &'a str {
        let start = self.offset();
        while matches!(self.peek(), Some(c) if pred(c)) {
            self.pos += 1;
        }
        &self.src[start..self.offset()]
    }
}

fn lex(src: &str) -> Result<Vec<Tok>, ParseError> {
    let mut lexer = Lexer::new(src);
    let mut out = Vec::new();
    loop {
        lexer.skip_whitespace();
        let start = lexer.offset();
        let Some(ch) = lexer.peek() else {
            out.push(Tok {
                kind: TokKind::Eof,
                offset: start,
                lexeme: String::new(),
            });
            return Ok(out);
        };
        let kind = match ch {
            '<' => {
                lexer.bump();
                TokKind::Open
            }
            '>' => {
                lexer.bump();
                TokKind::Close
            }
            ',' => {
                lexer.bump();
                TokKind::Comma
            }
            '(' => {
                lexer.bump();
                TokKind::LParen
            }
            ')' => {
                lexer.bump();
                TokKind::RParen
            }
            '?' => lex_hole(&mut lexer)?,
            '"' => lex_text(&mut lexer)?,
            '#' => lex_symbol(&mut lexer)?,
            '{' | '[' => lex_json_structure(&mut lexer)?,
            c if c.is_ascii_digit() || c == '-' || c == '+' => lex_numeric(&mut lexer)?,
            c if c.is_alphabetic() => lex_word(&mut lexer)?,
            c => {
                return Err(ParseError::BadChar {
                    offset: start,
                    ch: c,
                });
            }
        };
        out.push(Tok {
            kind,
            offset: start,
            lexeme: src[start..lexer.offset()].to_string(),
        });
    }
}

fn lex_hole(lexer: &mut Lexer<'_>) -> Result<TokKind, ParseError> {
    let start = lexer.offset();
    lexer.bump();
    let digits = lexer.take_while(|c| c.is_ascii_digit());
    if digits.is_empty() {
        return Err(ParseError::BadHole {
            offset: start,
            reason: "expected an index after '?', as in ?0",
        });
    }
    match digits.parse::<u32>() {
        Ok(index) => Ok(TokKind::Hole(index)),
        Err(_) => Err(ParseError::BadHole {
            offset: start,
            reason: "hole index does not fit in 32 bits",
        }),
    }
}

fn lex_text(lexer: &mut Lexer<'_>) -> Result<TokKind, ParseError> {
    let start = lexer.offset();
    lexer.bump();
    let mut value = String::new();
    loop {
        let escape_at = lexer.offset();
        match lexer.bump() {
            None => return Err(ParseError::UnterminatedText { offset: start }),
            Some('"') => break,
            Some('\\') => match lexer.bump() {
                None => return Err(ParseError::UnterminatedText { offset: start }),
                Some('"') => value.push('"'),
                Some('\\') => value.push('\\'),
                Some('n') => value.push('\n'),
                Some('r') => value.push('\r'),
                Some('t') => value.push('\t'),
                Some(other) => {
                    return Err(ParseError::BadEscape {
                        offset: escape_at,
                        ch: other,
                    });
                }
            },
            Some(other) => value.push(other),
        }
    }
    Ok(TokKind::Value(Ground::text(value)))
}

/// `#0123456789abcdef` is a symbol whose name this process never learned.
///
/// The renderer emits it whenever a [`SymbolId`] is missing from the table, so
/// the parser has to read it back, otherwise rendering a concept from a fresh
/// process would produce text that no longer parses.
fn lex_symbol(lexer: &mut Lexer<'_>) -> Result<TokKind, ParseError> {
    let start = lexer.offset();
    lexer.bump();
    let digits = lexer.take_while(|c| c.is_ascii_hexdigit());
    if digits.len() != 16 {
        return Err(ParseError::BadSymbol {
            offset: start,
            text: format!("#{digits}"),
        });
    }
    let raw = u64::from_str_radix(digits, 16).map_err(|_| ParseError::BadSymbol {
        offset: start,
        text: format!("#{digits}"),
    })?;
    Ok(TokKind::Symbol(SymbolId(raw)))
}

/// Scan a balanced `{...}` or `[...]` span and parse it as JSON.
///
/// String contents are skipped so a brace inside a JSON string cannot close
/// the literal early.
fn lex_json_structure(lexer: &mut Lexer<'_>) -> Result<TokKind, ParseError> {
    let start = lexer.offset();
    let open = lexer.peek().unwrap_or('{');
    let (close, kind) = if open == '{' {
        ('}', "json object")
    } else {
        (']', "json array")
    };
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    loop {
        let Some(ch) = lexer.bump() else {
            return Err(ParseError::UnterminatedJson {
                offset: start,
                kind,
                close,
            });
        };
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    let text = &lexer.src[start..lexer.offset()];
    json_token(text, start)
}

/// Scan `json(...)`, the wrapper that keeps a scalar JSON value distinct from
/// the ground concept that shares its spelling.
fn lex_json_scalar(lexer: &mut Lexer<'_>) -> Result<TokKind, ParseError> {
    let start = lexer.offset();
    lexer.bump();
    let inner_start = lexer.offset();
    let mut in_string = false;
    let mut escaped = false;
    let inner_end;
    loop {
        let at = lexer.offset();
        let Some(ch) = lexer.bump() else {
            return Err(ParseError::UnterminatedJson {
                offset: start,
                kind: "json",
                close: ')',
            });
        };
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
        } else if ch == ')' {
            inner_end = at;
            break;
        }
    }
    json_token(&lexer.src[inner_start..inner_end], inner_start)
}

fn json_token(text: &str, offset: usize) -> Result<TokKind, ParseError> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => Ok(TokKind::Value(Ground::json(value))),
        Err(err) => Err(ParseError::BadJson {
            offset,
            reason: err.to_string(),
        }),
    }
}

/// Numbers, datetimes and byte strings all begin with a digit or a sign, so
/// they are scanned as one run and classified afterwards.
///
/// The alternative, deciding on the first character, cannot work:
/// `2024-01-15T10:30:00Z` and `2024` start identically.
fn lex_numeric(lexer: &mut Lexer<'_>) -> Result<TokKind, ParseError> {
    let start = lexer.offset();
    let text = lexer.take_while(|c| c.is_alphanumeric() || matches!(c, '+' | '-' | '.' | ':'));

    if let Some(hex) = text.strip_prefix("0x") {
        if hex.len() % 2 != 0 {
            return Err(ParseError::BadBytes {
                offset: start,
                text: text.to_string(),
                reason: "a bytes literal needs an even number of hex digits",
            });
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for pair in hex.as_bytes().chunks(2) {
            let hi = (pair[0] as char).to_digit(16);
            let lo = (pair[1] as char).to_digit(16);
            match (hi, lo) {
                (Some(hi), Some(lo)) => bytes.push(((hi << 4) | lo) as u8),
                _ => {
                    return Err(ParseError::BadBytes {
                        offset: start,
                        text: text.to_string(),
                        reason: "expected hex digits after 0x",
                    });
                }
            }
        }
        return Ok(TokKind::Value(Ground::bytes(bytes)));
    }

    if let Ok(int) = text.parse::<i64>() {
        return Ok(TokKind::Value(Ground::Int(int)));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Ok(TokKind::Value(Ground::DateTime(dt.with_timezone(&Utc))));
    }
    if let Ok(float) = text.parse::<f64>() {
        return Ok(TokKind::Value(Ground::Float(float)));
    }
    Err(ParseError::BadNumber {
        offset: start,
        text: text.to_string(),
        reason: "expected an integer, a float, an RFC3339 datetime, or 0x bytes",
    })
}

/// A bare word is a name, unless it is one of the reserved literals.
///
/// `true`, `false`, `inf` and `nan` are matched case-insensitively because
/// symbol identity is case-insensitive: allowing `True` as a name would give
/// one concept two spellings, one of which reads back as a boolean.
fn lex_word(lexer: &mut Lexer<'_>) -> Result<TokKind, ParseError> {
    let word = lexer.take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'));
    let lowered = word.to_lowercase();
    match lowered.as_str() {
        "true" => Ok(TokKind::Value(Ground::Bool(true))),
        "false" => Ok(TokKind::Value(Ground::Bool(false))),
        "inf" => Ok(TokKind::Value(Ground::Float(f64::INFINITY))),
        "nan" => Ok(TokKind::Value(Ground::Float(f64::NAN))),
        "json" if lexer.peek() == Some('(') => lex_json_scalar(lexer),
        _ => Ok(TokKind::Name(word.to_string())),
    }
}

// ---- parser ----

struct Parser<'a> {
    tokens: Vec<Tok>,
    pos: usize,
    table: &'a SymbolTable,
}

impl Parser<'_> {
    fn here(&self) -> &Tok {
        // The lexer always appends Eof, so this index is always in range.
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn at(&self, kind: &TokKind) -> bool {
        std::mem::discriminant(&self.here().kind) == std::mem::discriminant(kind)
    }

    fn advance(&mut self) {
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
    }

    fn unexpected(&self, expected: &'static str) -> ParseError {
        let tok = self.here();
        ParseError::Unexpected {
            offset: tok.offset,
            expected,
            found: tok.found(),
        }
    }

    fn expect(&mut self, kind: &TokKind, expected: &'static str) -> Result<(), ParseError> {
        if self.at(kind) {
            self.advance();
            Ok(())
        } else {
            Err(self.unexpected(expected))
        }
    }

    /// A term is a head with at most one argument list. Currying is written
    /// with parentheses, so `(A<1>)<2>` is the only spelling of a compound
    /// applied to further arguments.
    fn term(&mut self) -> Result<Concept, ParseError> {
        let head = if self.at(&TokKind::LParen) {
            self.advance();
            let inner = self.term()?;
            self.expect(&TokKind::RParen, "')' to close the head")?;
            if !self.at(&TokKind::Open) {
                return Err(self.unexpected("'<' after a parenthesized head"));
            }
            inner
        } else {
            self.atom()?
        };

        if self.at(&TokKind::Open) {
            let args = self.args()?;
            Ok(Concept::apply(head, args))
        } else {
            Ok(head)
        }
    }

    fn atom(&mut self) -> Result<Concept, ParseError> {
        let concept = match &self.here().kind {
            TokKind::Name(name) => {
                let symbol = self.table.intern(name);
                Concept::symbol(symbol)
            }
            TokKind::Symbol(symbol) => Concept::symbol(*symbol),
            TokKind::Hole(index) => Concept::hole(*index),
            TokKind::Value(ground) => Concept::ground(ground.clone()),
            _ => return Err(self.unexpected("a concept")),
        };
        self.advance();
        Ok(concept)
    }

    fn args(&mut self) -> Result<Vec<Concept>, ParseError> {
        // Caller checked that the current token opens the list.
        self.advance();
        let mut args = Vec::new();
        if self.at(&TokKind::Close) {
            self.advance();
            return Ok(args);
        }
        loop {
            args.push(self.term()?);
            if self.at(&TokKind::Comma) {
                self.advance();
                if self.at(&TokKind::Close) {
                    return Err(self.unexpected("a concept after ','"));
                }
                continue;
            }
            if self.at(&TokKind::Close) {
                self.advance();
                return Ok(args);
            }
            return Err(self.unexpected("',' or '>'"));
        }
    }
}
