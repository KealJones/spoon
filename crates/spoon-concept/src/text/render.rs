//! Rendering a [`Concept`] into the angle-bracket notation.
//!
//! # Why there is no `impl Display for Concept`
//!
//! A named concept carries a [`SymbolId`], which is a digest, not a name. The
//! name lives in a [`SymbolTable`]. `Display::fmt` receives only `&self`, so a
//! direct impl on `Concept` could never print more than `#a3f1...` for every
//! name, which is exactly the output this notation exists to avoid. Binding
//! the concept and the table together in [`ConceptDisplay`] keeps the
//! allocation-free `{}` path, and makes the dependency on the table visible at
//! every call site instead of hiding it behind a global.

use std::fmt::{self, Write};

use chrono::SecondsFormat;

use crate::concept::Concept;
use crate::id::{ConceptId, Ground, SymbolId, SymbolTable};

/// Words the notation spells as literals, so they cannot also be names.
///
/// Compared case-insensitively because [`SymbolId`] identity is
/// case-insensitive: if `True` were allowed to render as `True`, it would read
/// back as the boolean and the round trip would silently change the concept.
/// A concept whose name collides with one of these renders in the `#hex` form
/// instead, which is ugly but honest. `json` is deliberately absent: a JSON
/// scalar is written `json(...)`, and a name is never followed by `(`, so
/// `Json<Payload>` stays readable.
pub(crate) const RESERVED_WORDS: [&str; 4] = ["true", "false", "inf", "nan"];

/// Bytes shown before compact mode elides the rest.
const COMPACT_BYTE_LIMIT: usize = 16;

/// Characters of JSON shown before compact mode elides the rest.
const COMPACT_JSON_LIMIT: usize = 64;

/// A concept bound to the table that can name its symbols, ready to print.
///
/// Cheap to build and `Copy`, so it is fine to construct one inline inside a
/// log macro. Nothing is rendered until it is formatted.
#[derive(Debug, Clone, Copy)]
pub struct ConceptDisplay<'a> {
    concept: &'a Concept,
    table: &'a SymbolTable,
    max_depth: Option<usize>,
}

impl<'a> ConceptDisplay<'a> {
    pub fn new(concept: &'a Concept, table: &'a SymbolTable) -> Self {
        ConceptDisplay {
            concept,
            table,
            max_depth: None,
        }
    }

    /// Elide everything nested deeper than `max_depth` with `...`.
    ///
    /// A log line has a budget; a twelve-level term blows it and buries the
    /// part that mattered. The result is meant to be read, not parsed.
    pub fn truncated(mut self, max_depth: usize) -> Self {
        self.max_depth = Some(max_depth);
        self
    }
}

impl fmt::Display for ConceptDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut writer = Writer {
            out: f,
            table: self.table,
            max_depth: self.max_depth,
        };
        writer.root(self.concept)
    }
}

/// Bind a concept to a symbol table for `{}` formatting.
///
/// This is the free-function form of [`ConceptDisplay::new`], meant to read
/// naturally inside a format string: `tracing::debug!("{}", display(&c, &t))`.
pub fn display<'a>(concept: &'a Concept, table: &'a SymbolTable) -> ConceptDisplay<'a> {
    ConceptDisplay::new(concept, table)
}

/// Render a concept in full, resolving names through `table`.
///
/// The exact inverse of [`crate::parse`]: anything this produces reads back as
/// the same concept.
pub fn render(concept: &Concept, table: &SymbolTable) -> String {
    ConceptDisplay::new(concept, table).to_string()
}

/// Render a concept, eliding anything nested deeper than `max_depth` with
/// `...`, and truncating long byte and JSON payloads.
///
/// For logs and error messages, where a readable approximation beats a
/// complete term nobody will read. The output is not guaranteed to parse.
pub fn render_compact(concept: &Concept, table: &SymbolTable, max_depth: usize) -> String {
    ConceptDisplay::new(concept, table)
        .truncated(max_depth)
        .to_string()
}

/// True when `name` can be written bare in the notation.
///
/// A name is bare-writable when it starts with a letter, continues with
/// letters, digits, `-`, `_`, `.` or `:`, and does not collide with a reserved
/// literal. Anything else (a name with a space, a name interned from an
/// external source) falls back to `#hex` so that rendering always produces
/// something the parser can read back.
pub(crate) fn is_plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_alphabetic() {
        return false;
    }
    if !chars.all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')) {
        return false;
    }
    let lower = name.to_lowercase();
    !RESERVED_WORDS.contains(&lower.as_str())
}

struct Writer<'w, W> {
    out: &'w mut W,
    table: &'w SymbolTable,
    max_depth: Option<usize>,
}

impl<W: Write> Writer<'_, W> {
    fn root(&mut self, concept: &Concept) -> fmt::Result {
        if self.max_depth == Some(0) {
            return self.out.write_str("...");
        }
        self.concept(concept, 1)
    }

    fn compact(&self) -> bool {
        self.max_depth.is_some()
    }

    fn concept(&mut self, concept: &Concept, depth: usize) -> fmt::Result {
        match concept {
            Concept::Atomic(ConceptId::Named(symbol)) => self.symbol(*symbol),
            Concept::Atomic(ConceptId::Ground(ground)) => self.ground(ground),
            Concept::Hole(hole) => write!(self.out, "?{}", hole.as_u32()),
            Concept::Compound { head, args } => self.compound(head, args, depth),
        }
    }

    /// A compound renders its head, then its arguments in angle brackets.
    ///
    /// Two shapes need care. A head that is itself a compound is parenthesized
    /// (`(Sort<Descending>)<Friends>`) so the reader can tell which `<...>`
    /// belongs to which application. A zero-argument compound keeps its empty
    /// brackets (`Raining<>`), because asserting `Raining` in context is a
    /// different concept from the bare atomic `Raining`.
    fn compound(&mut self, head: &Concept, args: &[Concept], depth: usize) -> fmt::Result {
        let truncated = self.max_depth.is_some_and(|limit| depth >= limit);

        if head.is_compound() {
            self.out.write_char('(')?;
            self.concept(head, depth + 1)?;
            self.out.write_char(')')?;
        } else {
            self.concept(head, depth + 1)?;
        }

        if truncated && !args.is_empty() {
            return self.out.write_str("<...>");
        }

        self.out.write_char('<')?;
        for (index, arg) in args.iter().enumerate() {
            if index > 0 {
                self.out.write_str(", ")?;
            }
            self.concept(arg, depth + 1)?;
        }
        self.out.write_char('>')
    }

    fn symbol(&mut self, symbol: SymbolId) -> fmt::Result {
        match self.table.resolve(symbol) {
            Some(name) if is_plain_name(&name) => self.out.write_str(&name),
            _ => write!(self.out, "#{:016x}", symbol.as_u64()),
        }
    }

    fn ground(&mut self, ground: &Ground) -> fmt::Result {
        match ground {
            Ground::Bool(b) => self.out.write_str(if *b { "true" } else { "false" }),
            Ground::Int(i) => write!(self.out, "{i}"),
            Ground::Float(f) => self.float(*f),
            Ground::Text(t) => self.text(t),
            Ground::Bytes(b) => self.bytes(b),
            Ground::DateTime(dt) => self
                .out
                .write_str(&dt.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
            Ground::Json(blob) => self.json(blob.value()),
        }
    }

    /// Floats always carry a decimal point or an exponent, so `1.0` can never
    /// be misread as the integer `1`: they are different concepts.
    ///
    /// The three non-finite values get the reserved spellings `inf`, `-inf`
    /// and `nan`. `Ground` treats all NaN bit patterns as one value, so `nan`
    /// reads back as the same concept it was written from.
    fn float(&mut self, value: f64) -> fmt::Result {
        if value.is_nan() {
            return self.out.write_str("nan");
        }
        if value.is_infinite() {
            return self.out.write_str(if value > 0.0 { "inf" } else { "-inf" });
        }
        // `{:?}` on f64 is the shortest representation that round-trips
        // exactly, which is precisely the guarantee this notation needs.
        let text = format!("{value:?}");
        self.out.write_str(&text)?;
        if !text.contains(['.', 'e', 'E']) {
            self.out.write_str(".0")?;
        }
        Ok(())
    }

    fn text(&mut self, value: &str) -> fmt::Result {
        self.out.write_char('"')?;
        for ch in value.chars() {
            match ch {
                '"' => self.out.write_str("\\\"")?,
                '\\' => self.out.write_str("\\\\")?,
                '\n' => self.out.write_str("\\n")?,
                '\r' => self.out.write_str("\\r")?,
                '\t' => self.out.write_str("\\t")?,
                _ => self.out.write_char(ch)?,
            }
        }
        self.out.write_char('"')
    }

    fn bytes(&mut self, value: &[u8]) -> fmt::Result {
        self.out.write_str("0x")?;
        let elide = self.compact() && value.len() > COMPACT_BYTE_LIMIT;
        let shown = if elide {
            &value[..COMPACT_BYTE_LIMIT]
        } else {
            value
        };
        for byte in shown {
            write!(self.out, "{byte:02x}")?;
        }
        if elide {
            self.out.write_str("...")?;
        }
        Ok(())
    }

    /// JSON objects and arrays render bare, because `{"a":1}` and `[1,2]`
    /// cannot be confused with anything else in the notation.
    ///
    /// Scalar JSON gets a `json(...)` wrapper. Without it, `Json(42)` would
    /// render as `42` and read back as `Int(42)`: a different concept with a
    /// different content id. The wrapper is the price of keeping the round
    /// trip exact for every ground variant.
    fn json(&mut self, value: &serde_json::Value) -> fmt::Result {
        let structural = matches!(
            value,
            serde_json::Value::Object(_) | serde_json::Value::Array(_)
        );
        let text = serde_json::to_string(value).map_err(|_| fmt::Error)?;
        if !structural {
            self.out.write_str("json(")?;
        }
        if self.compact() && text.chars().count() > COMPACT_JSON_LIMIT {
            for ch in text.chars().take(COMPACT_JSON_LIMIT) {
                self.out.write_char(ch)?;
            }
            self.out.write_str("...")?;
        } else {
            self.out.write_str(&text)?;
        }
        if !structural {
            self.out.write_char(')')?;
        }
        Ok(())
    }
}
