//! The angle-bracket notation: the one textual form of a [`Concept`].
//!
//! Design docs, logs, error messages, seed files and test fixtures all write
//! concepts the same way, and this module is what makes that notation real in
//! both directions. [`render`] and [`parse`] are inverses: parsing what the
//! renderer produced gives back the concept it started from, for every ground
//! variant, every nesting depth, and every hole.
//!
//! ```
//! use spoon_concept::{Concept, SymbolTable, parse, render};
//!
//! let table = SymbolTable::new();
//! let concept = parse("FriendWith<Greg, Keal>", &table).unwrap();
//! assert_eq!(concept.arity(), 2);
//! assert_eq!(render(&concept, &table), "FriendWith<Greg, Keal>");
//! ```
//!
//! # Grammar
//!
//! ```text
//! term      := '(' term ')' args | atom args?
//! args      := '<' '>' | '<' term (',' term)* '>'
//! atom      := name | symbol | hole | ground
//!
//! name      := letter (letter | digit | '-' | '_' | '.' | ':')*
//! symbol    := '#' hex{16}
//! hole      := '?' digit+
//!
//! ground    := int | float | text | bool | bytes | datetime | json
//! int       := '-'? digit+
//! float     := <Rust f64 literal, always with '.' or exponent> | 'inf' | '-inf' | 'nan'
//! text      := '"' (char | '\"' | '\\' | '\n' | '\r' | '\t')* '"'
//! bool      := 'true' | 'false'
//! bytes     := '0x' hex*                        (even number of digits)
//! datetime  := RFC3339 timestamp
//! json      := '{' ... '}' | '[' ... ']' | 'json(' scalar ')'
//! ```
//!
//! Whitespace, including newlines, is insignificant between tokens.
//!
//! # Design notes
//!
//! **A hole can be a head.** `?0<?1, ?2>` is a pattern that matches any binary
//! application, which is how rules quantify over relations.
//!
//! **Zero-arg compounds keep their brackets.** `Raining<>` is an assertion of
//! `Raining` in context and is a different concept from the bare atomic
//! `Raining`, with a different content id. Dropping the brackets would merge
//! them.
//!
//! **A compound head is parenthesized.** `(Sort<Descending>)<Friends>` says
//! which application owns which bracket pair; `Sort<Descending><Friends>`
//! would not.
//!
//! **`42`, `42.0` and `"42"` are three concepts.** Int, float and text are
//! separate ground identities, so the notation keeps them separate too: a
//! float always carries a decimal point or an exponent, and text is always
//! quoted.
//!
//! **Names are case-insensitive but casing is preserved.** [`crate::SymbolId`]
//! lowercases before hashing, so `FriendWith` and `friendwith` are one
//! concept, while `FriendWith` and `friend-with` are two: separators are not
//! collapsed, because doing so would merge `co-op` with `coop`. Whichever
//! spelling reaches the [`crate::SymbolTable`] first is the one that prints.

mod parse;
mod render;

pub use parse::{ParseError, parse};
pub use render::{ConceptDisplay, display, render, render_compact};
