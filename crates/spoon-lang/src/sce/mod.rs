//! SCE: Spoon Controlled English parser, realizer, and utilities.
//!
//! Public API:
//! - [`parse`]: parse one SCE sentence
//! - [`parse_text`]: split text into sentences and parse each
//! - [`realize`]: Clause -> SCE string
//! - [`lemmatize`]: normalize inflected verb forms to base
//! - [`Lexicon`]: open-class vocabulary manager

mod arith;
mod lemma;
mod lexicon;
mod parser;
mod realize;
mod tokenizer;

pub use lexicon::Lexicon;
pub use lemma::lemmatize;
pub use parser::{parse, parse_text, ParseError};
pub use realize::realize;
