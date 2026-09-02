//! spoon-lang: SCE grammar/parser, ears (recognizer + normalizer), mouth (realizer).

pub mod ears;
pub mod mouth;
pub mod sce;

pub use sce::{parse, parse_text, realize, lemmatize, Lexicon, ParseError};
