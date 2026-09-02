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
mod pred;
mod realize;
mod tokenizer;

pub use lexicon::Lexicon;
pub use lemma::{lemmatize, singularize_noun};
pub use parser::{parse, parse_text, ParseError};
#[doc(hidden)]
pub use parser::parse_traced;
#[doc(hidden)]
pub use tokenizer::split_sentences;
pub use realize::realize;
