//! Recursive-descent SCE parser. Produces one `Clause` per sentence.
//!
//! The lexicon is open: unknown words are guessed from position and collected
//! in `unknowns`. Never more than one Clause per sentence; ambiguity resolved
//! by fewest guesses then fewer derivation steps.

use std::collections::HashMap;
use spoon_core::types::clause::{
    Act, ArithExpr, Clause, Modal, Pred, Quant, QuestionKind, Referent, Term,
};
use spoon_core::types::value::Value;

use super::arith::ArithParser;
use super::lemma::{lemmatize, number_word, singularize_noun};
use super::lexicon::Lexicon;
use super::pred;
use super::tokenizer::{tokenize, Tok};

/// Error returned when a sentence cannot be parsed.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub position: Option<usize>,
    pub unknown_words: Vec<String>,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// ---------------------------------------------------------------------------
// Intermediate builder
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Parts {
    refs: Vec<Referent>,
    conds: Vec<Pred>,
}

struct Mark {
    pos: usize,
    refs: usize,
    conds: usize,
    cnt: u32,
    unknowns: usize,
    name_map: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser<'lex> {
    toks: Vec<Tok>,
    pos: usize,
    lex: &'lex Lexicon,
    cnt: u32,
    name_map: HashMap<String, String>, // name/var -> var
    unknowns: Vec<String>,
}

impl<'lex> Parser<'lex> {
    fn new(toks: Vec<Tok>, lex: &'lex Lexicon) -> Self {
        Parser { toks, pos: 0, lex, cnt: 0, name_map: HashMap::new(), unknowns: Vec::new() }
    }

    fn save(&self) -> usize { self.pos }
    fn restore(&mut self, p: usize) { self.pos = p; }

    /// Full checkpoint (position plus everything a failed attempt may have
    /// pushed) for speculative paths that must leave no trace on failure.
    fn mark(&self, parts: &Parts) -> Mark {
        Mark {
            pos: self.pos,
            refs: parts.refs.len(),
            conds: parts.conds.len(),
            cnt: self.cnt,
            unknowns: self.unknowns.len(),
            name_map: self.name_map.clone(),
        }
    }

    fn rollback(&mut self, parts: &mut Parts, m: Mark) {
        self.pos = m.pos;
        self.cnt = m.cnt;
        self.unknowns.truncate(m.unknowns);
        self.name_map = m.name_map;
        parts.refs.truncate(m.refs);
        parts.conds.truncate(m.conds);
    }

    fn next_var(&mut self) -> String {
        self.cnt += 1;
        format!("x{}", self.cnt)
    }

    fn name_var(&mut self, name: &str) -> String {
        if let Some(v) = self.name_map.get(name) {
            return v.clone();
        }
        let v = self.next_var();
        self.name_map.insert(name.to_string(), v.clone());
        v
    }

    fn err(&self, msg: &str) -> ParseError {
        ParseError { message: msg.to_string(), position: Some(self.pos), unknown_words: vec![] }
    }

    fn peek(&self) -> Option<&Tok> { self.toks.get(self.pos) }
    fn peek2(&self) -> Option<&Tok> { self.toks.get(self.pos + 1) }
    #[allow(dead_code)]
    fn peek3(&self) -> Option<&Tok> { self.toks.get(self.pos + 2) }

    /// Lowercase word at current position (None if not a Word token).
    fn pw(&self) -> Option<String> {
        match self.peek()? {
            Tok::Word(w) => Some(w.to_lowercase()),
            _ => None,
        }
    }
    /// Raw (case-preserved) word at current position.
    fn praw(&self) -> Option<&str> {
        match self.peek()? {
            Tok::Word(w) => Some(w.as_str()),
            _ => None,
        }
    }
    fn pw2(&self) -> Option<String> {
        match self.peek2()? {
            Tok::Word(w) => Some(w.to_lowercase()),
            _ => None,
        }
    }
    #[allow(dead_code)]
    fn pw3(&self) -> Option<String> {
        match self.peek3()? {
            Tok::Word(w) => Some(w.to_lowercase()),
            _ => None,
        }
    }

    #[allow(dead_code)]
    fn advance(&mut self) -> Option<Tok> {
        if self.pos < self.toks.len() {
            let t = self.toks[self.pos].clone();
            self.pos += 1;
            Some(t)
        } else {
            None
        }
    }

    /// Consume current token if it matches `w` (case-insensitive).
    fn eat_word(&mut self, w: &str) -> bool {
        if self.pw().as_deref() == Some(w) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_word(&mut self, w: &str) -> Result<(), ParseError> {
        if self.eat_word(w) { Ok(()) } else {
            Err(self.err(&format!("Expected '{w}'", )))
        }
    }

    fn eat_tok(&mut self, tok: &Tok) -> bool {
        if self.peek() == Some(tok) { self.pos += 1; true } else { false }
    }

    fn is_terminator(&self) -> bool {
        matches!(self.peek(), Some(Tok::Period) | Some(Tok::Bang) | Some(Tok::Question))
    }

    /// Terminator (or end of input) at an absolute token index.
    fn is_terminator_at(&self, pos: usize) -> bool {
        match self.toks.get(pos) {
            Some(Tok::Period) | Some(Tok::Bang) | Some(Tok::Question) | None => true,
            _ => false,
        }
    }

    /// True when the current position starts a count/quantifier phrase that
    /// should be parsed as an NP rather than a PP ("at least N", "at most N",
    /// "more than N", "exactly N", a bare number, etc.).
    fn is_count_det_start(&self) -> bool {
        let w = match self.pw() {
            Some(w) => w,
            None => return false,
        };
        match w.as_str() {
            "at" => matches!(self.pw2().as_deref(), Some("least") | Some("most")),
            "more" => self.pw2().as_deref() == Some("than"),
            "exactly" => true,
            _ => {
                if number_word(&w).is_some() { return true; }
                matches!(self.peek(), Some(Tok::Number(_)))
            }
        }
    }

    /// Lenient breaker check for "the N of NP" noun head: only rejects core verbs,
    /// so corpus-seeded nouns like "wellbeing" or "double" are accepted.
    fn is_the_of_np_breaker(&self, w: &str) -> bool {
        if Lexicon::is_function_word(w) { return true; }
        if Lexicon::is_prep(w) { return true; }
        let lm = lemmatize(w);
        if self.lex.is_core_verb(w) || self.lex.is_core_verb(&lm) { return true; }
        false
    }

    /// True if the word at pos could start a new VP (is verb-like).
    fn looks_like_verb(&self, w: &str) -> bool {
        self.lex.is_verb(w) || self.lex.is_verb(&lemmatize(w))
    }

    /// Is current token a variable? (single uppercase letter, optionally followed by digits)
    fn is_var(s: &str) -> bool {
        let mut chars = s.chars();
        match chars.next() {
            Some(c) if c.is_uppercase() => chars.all(|c| c.is_ascii_digit()),
            _ => false,
        }
    }

    /// Is this word a proper name in context?
    ///
    /// - Reserved names (`User`, `Assistant`) are always names.
    /// - Hyphenated with capital after first hyphen (`Object-X`, `New-York-City`) are names.
    /// - Capitalized words at any position: name if NOT a function word (this handles
    ///   sentence-initial names like "John" vs "The"/"Every"/"There").
    fn is_name(raw: &str, _is_sentence_initial: bool) -> bool {
        if raw == "User" || raw == "Assistant" { return true; }
        // Hyphenated with capital after hyphen: Object-X, File-A, New-York-City
        if raw.contains('-') {
            if let Some(after) = raw.splitn(2, '-').nth(1) {
                if after.starts_with(|c: char| c.is_uppercase()) { return true; }
            }
        }
        // Capitalized, non-variable, and not a function word
        if raw.starts_with(|c: char| c.is_uppercase()) && !Self::is_var(raw) {
            let lw = raw.to_lowercase();
            return !Lexicon::is_function_word(&lw)
                && !matches!(lw.as_str(), "there" | "it");
        }
        false
    }

    /// Is current position the sentence-initial word?
    fn is_initial(&self) -> bool {
        // Initial if pos == 0 or all previous tokens were whitespace-skipped
        // In our token stream, the first Word is at pos 0 (or after some punct on prior sentences)
        // We track this by checking if any prior Word/Name exists
        !self.toks[..self.pos].iter().any(|t| matches!(t, Tok::Word(_)))
    }

    // -----------------------------------------------------------------------
    // Top-level dispatch
    // -----------------------------------------------------------------------

    pub fn parse_sentence(&mut self, sce: &str) -> Result<Clause, ParseError> {
        // Check for "It is possible/false that S."
        if self.pw().as_deref() == Some("it") {
            return self.parse_it_sentence(sce);
        }
        // Check for Rule: "If ..." or "For every ..."
        if self.pw().as_deref() == Some("if") {
            return self.parse_rule_sentence(sce, false);
        }
        if self.pw().as_deref() == Some("for") && self.pw2().as_deref() == Some("every") {
            return self.parse_for_every_sentence(sce);
        }
        // Check for Command: "Assistant ,"
        if self.pw().as_deref() == Some("assistant") && matches!(self.peek2(), Some(Tok::Comma)) {
            return self.parse_command_sentence(sce);
        }
        // Check for Question
        if let Some(q_start) = self.question_keyword() {
            return self.parse_question_sentence(sce, &q_start);
        }
        // Check for Existential: "There is/are ..."
        if self.pw().as_deref() == Some("there") {
            return self.parse_there_sentence(sce);
        }
        // Default: Declarative
        self.parse_decl_sentence(sce)
    }

    fn question_keyword(&self) -> Option<String> {
        match self.pw().as_deref()? {
            "who" | "what" | "which" | "where" | "when" | "how" | "does" | "is" | "are"
            | "can" | "should" | "must" | "may" => Some(self.pw().unwrap()),
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // "It is possible/false that S."
    // -----------------------------------------------------------------------

    fn parse_it_sentence(&mut self, sce: &str) -> Result<Clause, ParseError> {
        self.expect_word("it")?;
        self.expect_word("is")?;
        let attr = match self.pw().as_deref() {
            Some("possible") => { self.pos += 1; "possible".to_string() }
            Some("false") => { self.pos += 1; "false".to_string() }
            Some(w) => { let a = w.to_string(); self.pos += 1; a }
            None => return Err(self.err("Expected adjective after 'It is'")),
        };
        // Parse optional "that S" or just S
        let has_that = self.eat_word("that");
        let _ = has_that;
        let inner = self.parse_inner_sentence()?;
        let mut parts = Parts::default();
        let it_var = self.name_var("It");
        parts.refs.push(Referent {
            var: it_var.clone(),
            noun: None,
            quant: Quant::Named("It".to_string()),
            mods: vec![],
            owner: None,
            span: None,
        });
        parts.conds.push(Pred {
            pred: "be".to_string(),
            args: vec![Term::Var { var: it_var }, Term::Sub { clause: Box::new(inner) }],
            negated: false,
            modal: None,
            adjuncts: vec![],
            attr: Some(attr),
        });
        self.eat_terminator();
        Ok(Clause { act: Act::Assert, referents: parts.refs, conditions: parts.conds, then: vec![], then_referents: vec![], sce: sce.to_string() })
    }

    fn parse_inner_sentence(&mut self) -> Result<Clause, ParseError> {
        // Parse an embedded sentence (without its own terminator handling)
        let saved_cnt = self.cnt;
        let saved_map = self.name_map.clone();
        let mut inner_parts = Parts::default();
        // Try to parse as a declarative
        let res = self.parse_decl_into(&mut inner_parts);
        match res {
            Ok(_) => {}
            Err(e) => {
                self.cnt = saved_cnt;
                self.name_map = saved_map;
                return Err(e);
            }
        }
        Ok(Clause {
            act: Act::Assert,
            referents: inner_parts.refs,
            conditions: inner_parts.conds,
            then: vec![], then_referents: vec![],
            sce: String::new(),
        })
    }

    // -----------------------------------------------------------------------
    // Declarative sentence
    // -----------------------------------------------------------------------

    fn parse_decl_sentence(&mut self, sce: &str) -> Result<Clause, ParseError> {
        let mut parts = Parts::default();
        self.parse_decl_into(&mut parts)?;
        let term = self.eat_terminator_type();
        if term.is_none() && !self.at_end() {
            return Err(self.leftover_err());
        }
        Ok(Clause { act: Act::Assert, referents: parts.refs, conditions: parts.conds, then: vec![], then_referents: vec![], sce: sce.to_string() })
    }

    /// Error for content the grammar could not attach: names the first
    /// unconsumed token and its position.
    fn leftover_err(&self) -> ParseError {
        let tok = self.toks.get(self.pos).map(|t| t.to_string()).unwrap_or_default();
        self.err(&format!("unexpected '{}' at {}", tok, self.pos))
    }

    fn parse_decl_into(&mut self, parts: &mut Parts) -> Result<(), ParseError> {
        // "There is/are NP"
        if self.pw().as_deref() == Some("there") {
            self.pos += 1;
            if self.pw().as_deref() == Some("is") || self.pw().as_deref() == Some("are") {
                self.pos += 1;
            } else {
                return Err(self.err("Expected 'is' or 'are' after 'There'"));
            }
            let _var = self.parse_np(parts)?;
            return Ok(());
        }
        // NP VP ('and' VP)*
        let subj = self.parse_np(parts)?;
        self.parse_vp_list(&subj, parts, false)?;
        Ok(())
    }

    fn parse_there_sentence(&mut self, sce: &str) -> Result<Clause, ParseError> {
        let mut parts = Parts::default();
        self.expect_word("there")?;
        if !self.eat_word("is") && !self.eat_word("are") {
            return Err(self.err("Expected 'is' or 'are' after 'There'"));
        }
        // Parse NP(s) potentially joined by 'and'
        let _var = self.parse_np(&mut parts)?;
        while self.eat_word("and") {
            self.parse_np(&mut parts)?;
        }
        self.eat_terminator();
        Ok(Clause { act: Act::Assert, referents: parts.refs, conditions: parts.conds, then: vec![], then_referents: vec![], sce: sce.to_string() })
    }

    // -----------------------------------------------------------------------
    // VP list (declarative): VP ('and' VP)*
    // -----------------------------------------------------------------------

    fn parse_vp_list(&mut self, subj: &str, parts: &mut Parts, in_then: bool) -> Result<(), ParseError> {
        self.parse_vp(subj, parts, in_then, false)?;
        loop {
            // Save BEFORE consuming "and" so we can backtrack fully on failure.
            let saved = self.save();
            if !self.eat_word("and") { break; }
            if self.parse_vp(subj, parts, in_then, false).is_err() {
                self.restore(saved);
                break;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // VP
    // -----------------------------------------------------------------------

    fn parse_vp(&mut self, subj: &str, parts: &mut Parts, _in_then: bool, base: bool) -> Result<(), ParseError> {
        let _ = base;
        // "is not" / "are not" / copula variants
        if self.pw().as_deref() == Some("is") || self.pw().as_deref() == Some("are") {
            return self.parse_copula(subj, parts);
        }
        // "does not Verb"
        if self.pw().as_deref() == Some("does") && self.pw2().as_deref() == Some("not") {
            self.pos += 2;
            return self.parse_verb_pred(subj, parts, false, true, None);
        }
        // "does" (for questions) - shouldn't appear in plain VP but handle it
        // Modal
        if let Some(modal) = self.parse_modal() {
            let negated = self.eat_word("not");
            return self.parse_verb_pred(subj, parts, false, negated, Some(modal));
        }
        // Plain verb (3sg or base)
        self.parse_verb_pred(subj, parts, true, false, None)
    }

    fn parse_copula(&mut self, subj: &str, parts: &mut Parts) -> Result<(), ParseError> {
        self.pos += 1; // consume 'is'/'are'
        let negated = self.eat_word("not");
        self.parse_copula_body(subj, parts, negated)
    }

    /// Everything after `is`/`are` (and an optional `not`). Shared with the
    /// `Is NP ...?` question so a declarative and its question parse alike.
    fn parse_copula_body(&mut self, subj: &str, parts: &mut Parts, negated: bool) -> Result<(), ParseError> {
        // passive: "is Verbpp by NP" (heuristic: word after 'is' ends in -ed/special + 'by')
        if !negated {
            let saved = self.save();
            if let Ok(()) = self.try_passive(subj, parts) {
                return Ok(());
            }
            self.restore(saved);
        }

        // "is Adj-er than NP" comparative
        if let Some(adj) = self.pw() {
            if adj.ends_with("er") || adj.ends_with("-than") || adj.contains('-') {
                let saved = self.save();
                if let Ok(()) = self.try_comparative(subj, parts, negated) {
                    return Ok(());
                }
                self.restore(saved);
            }
        }

        // "is north of NP" / "is to the left of NP" - a two-place relation.
        // Tried before the is-a reading so a lexicon that happens to know
        // "north" as a noun cannot turn the relation into a typing.
        if self.try_relation_of(subj, parts, negated).is_ok() {
            return Ok(());
        }

        // "is NP" (is-a)
        {
            let saved = self.save();
            if let Ok(()) = self.try_copula_np(subj, parts, negated) {
                return Ok(());
            }
            self.restore(saved);
        }

        // "is in the garden" - a locative complement states the subject's
        // location, so it becomes `be(the location of subj, place)`.
        if let Some(prep) = self.pw().filter(|w| pred::is_locative_prep(w)) {
            let m = self.mark(parts);
            self.pos += 1;
            match self.parse_np(parts) {
                Ok(place) => {
                    let prop = self.push_location_property(parts, subj, &prep);
                    let adjuncts = self.parse_pps(parts)?;
                    parts.conds.push(Pred {
                        pred: "be".to_string(),
                        args: vec![Term::Var { var: prop }, Term::Var { var: place }],
                        negated,
                        modal: None,
                        adjuncts,
                        attr: None,
                    });
                    return Ok(());
                }
                Err(_) => self.rollback(parts, m),
            }
        }

        // "is PP" - other state copula: "is with Mary", "is about death"
        if Lexicon::is_prep(self.pw().as_deref().unwrap_or("")) {
            let adjuncts = self.parse_pps(parts)?;
            parts.conds.push(Pred {
                pred: "be".to_string(),
                args: vec![Term::Var { var: subj.to_string() }],
                negated,
                modal: None,
                adjuncts,
                attr: None,
            });
            return Ok(());
        }

        // "is Adj (PP*)" copula adjective
        self.parse_copula_adj(subj, parts, negated)
    }

    /// The definite `location` referent owned by `subj`, carrying the
    /// preposition that was used as its only modifier.
    fn push_location_property(&mut self, parts: &mut Parts, subj: &str, prep: &str) -> String {
        let var = self.next_var();
        parts.refs.push(Referent {
            var: var.clone(),
            noun: Some(pred::LOCATION_NOUN.to_string()),
            quant: Quant::Def,
            mods: vec![prep.to_string()],
            owner: Some(subj.to_string()),
            span: None,
        });
        var
    }

    /// `X is north of Y`, `X is to the left of Y`, `X is afraid of Y` ->
    /// `Pred { pred: "<phrase>-of", args: [X, Y] }`.
    ///
    /// The phrase must not start with a determiner: that is what keeps
    /// `the double of 3` a property NP and `a member of the team` an is-a.
    fn try_relation_of(&mut self, subj: &str, parts: &mut Parts, negated: bool) -> Result<(), ParseError> {
        let no = || self.err("not a relational complement");
        // Peek the phrase first so a miss costs nothing.
        let mut words: Vec<String> = vec![];
        loop {
            let Some(Tok::Word(w)) = self.toks.get(self.pos + words.len()) else {
                return Err(no());
            };
            let lw = w.to_lowercase();
            if lw == "of" {
                break;
            }
            if words.is_empty() && (self.starts_determiner() || Self::is_name(w, false)) {
                return Err(no());
            }
            if words.len() == pred::MAX_RELATION_WORDS {
                return Err(no());
            }
            words.push(lw);
        }
        let Some(name) = pred::relation_of_name(&words) else {
            return Err(no());
        };
        let m = self.mark(parts);
        self.pos += words.len() + 1; // phrase + 'of'
        let obj = self.parse_prep_object(parts);
        if obj.is_empty() {
            self.rollback(parts, m);
            return Err(self.err("Expected a noun phrase after 'of'"));
        }
        let adjuncts = self.parse_pps(parts)?;
        parts.conds.push(Pred {
            pred: name,
            args: vec![Term::Var { var: subj.to_string() }, Term::Var { var: obj }],
            negated,
            modal: None,
            adjuncts,
            attr: None,
        });
        Ok(())
    }

    /// Does the current position start a determiner or count phrase? Peek
    /// only; `try_parse_det` consumes.
    fn starts_determiner(&self) -> bool {
        if matches!(self.peek(), Some(Tok::Number(_))) {
            return true;
        }
        match self.pw() {
            Some(w) => matches!(
                w.as_str(),
                "a" | "an" | "the" | "every" | "some" | "no" | "not" | "exactly" | "at" | "more"
            ) || number_word(&w).is_some(),
            None => true,
        }
    }

    fn try_passive(&mut self, subj: &str, parts: &mut Parts) -> Result<(), ParseError> {
        // "is Verb-pp by NP"
        let verb_raw = match self.praw() {
            Some(v) => v.to_string(),
            None => return Err(self.err("Expected past participle")),
        };
        // Check if next-next token is 'by'
        if self.pw2().as_deref() != Some("by") {
            return Err(self.err("Not passive"));
        }
        self.pos += 1; // consume verb
        self.pos += 1; // consume 'by'
        let agent_var = self.parse_np(parts)?;
        let lemma = lemmatize(&verb_raw);
        parts.conds.push(Pred {
            pred: lemma,
            args: vec![Term::Var { var: agent_var }, Term::Var { var: subj.to_string() }],
            negated: false,
            modal: None,
            adjuncts: vec![],
            attr: None,
        });
        Ok(())
    }

    fn try_comparative(&mut self, subj: &str, parts: &mut Parts, negated: bool) -> Result<(), ParseError> {
        let adj_raw = match self.praw() {
            Some(a) => a.to_string(),
            None => return Err(self.err("Expected adjective")),
        };
        self.pos += 1;
        // Could be "smarter than NP" or "smarter-than NP"
        let adj_key = if self.eat_word("than") {
            format!("{}-than", adj_raw.to_lowercase())
        } else if adj_raw.to_lowercase().ends_with("-than") {
            adj_raw.to_lowercase()
        } else {
            return Err(self.err("Expected 'than'"));
        };
        let cmp_var = self.parse_np(parts)?;
        parts.conds.push(Pred {
            pred: "be".to_string(),
            args: vec![Term::Var { var: subj.to_string() }, Term::Var { var: cmp_var }],
            negated,
            modal: None,
            adjuncts: vec![],
            attr: Some(adj_key),
        });
        Ok(())
    }

    fn try_copula_np(&mut self, subj: &str, parts: &mut Parts, negated: bool) -> Result<(), ParseError> {
        let saved = self.save();
        // Try to parse an NP. If it succeeds and nothing weird follows, it's copula-NP.
        if let Ok(obj_var) = self.parse_np(parts) {
            // Must be followed by terminator or 'and' or PP or end-of-relevant-span
            // Also allow verb-starting word (main clause VP after a relative clause copula).
            let next_is_verb = self.pw().as_deref()
                .map(|w| self.looks_like_verb(w))
                .unwrap_or(false);
            if self.is_terminator() || self.pw().as_deref() == Some("and")
                || self.pw().as_deref() == Some("then")
                || self.at_end() || Lexicon::is_prep(self.pw().as_deref().unwrap_or(""))
                || self.pw().as_deref() == Some("that") || next_is_verb
            {
                let adjuncts = self.parse_pps(parts)?;
                parts.conds.push(Pred {
                    pred: "be".to_string(),
                    args: vec![Term::Var { var: subj.to_string() }, Term::Var { var: obj_var }],
                    negated,
                    modal: None,
                    adjuncts,
                    attr: None,
                });
                return Ok(());
            }
        }
        self.restore(saved);
        Err(self.err("Not copula-NP"))
    }

    fn parse_copula_adj(&mut self, subj: &str, parts: &mut Parts, negated: bool) -> Result<(), ParseError> {
        // Adj (maybe "very"/"quite"/etc. prefix)
        let mut adj_parts = vec![];
        // Skip adverbs like "very", "quite"
        while let Some(w) = self.pw() {
            if matches!(w.as_str(), "very"|"quite"|"rather"|"somewhat"|"extremely"|"highly"|"deeply"|"truly") {
                adj_parts.push(w);
                self.pos += 1;
            } else {
                break;
            }
        }
        // In copula position, any word that isn't a clear function word/prep/var/name
        // can be an adjective (even if the corpus lexicon seeded it as a verb).
        let adj = match self.pw() {
            Some(a) if !Lexicon::is_function_word(&a) && !Lexicon::is_prep(&a) && !Self::is_var(&a) => {
                let a = a.clone();
                adj_parts.push(a.clone());
                self.pos += 1;
                adj_parts.join("-")
            }
            _ => {
                if adj_parts.is_empty() {
                    return Err(self.err("Expected adjective in copula"));
                }
                adj_parts.join("-")
            }
        };
        let adjuncts = self.parse_pps(parts)?;
        parts.conds.push(Pred {
            pred: "be".to_string(),
            args: vec![Term::Var { var: subj.to_string() }],
            negated,
            modal: None,
            adjuncts,
            attr: Some(adj),
        });
        Ok(())
    }

    fn parse_modal(&mut self) -> Option<Modal> {
        match self.pw().as_deref()? {
            "can" => { self.pos += 1; Some(Modal::Can) }
            "cannot" => { self.pos += 1; Some(Modal::Can) } // handled as can + negated
            "should" => { self.pos += 1; Some(Modal::Should) }
            "must" => { self.pos += 1; Some(Modal::Must) }
            "may" => { self.pos += 1; Some(Modal::May) }
            _ => None,
        }
    }

    fn parse_verb_pred(
        &mut self,
        subj: &str,
        parts: &mut Parts,
        third_sg: bool,
        negated: bool,
        modal: Option<Modal>,
    ) -> Result<(), ParseError> {
        // Handle "cannot" as can + negated
        let (negated, modal) = if self.pw().as_deref() == Some("cannot") && modal.is_none() {
            self.pos += 1;
            (true, Some(Modal::Can))
        } else {
            (negated, modal)
        };

        // Skip adverbs before verb (usually, repeatedly, etc.)
        self.skip_adverbs();

        // Verb
        let verb_raw = match self.praw() {
            Some(v) => v.to_string(),
            None => return Err(self.err("Expected verb")),
        };
        let lw = verb_raw.to_lowercase();
        let known_verb = self.lex.is_verb(&lw) || self.lex.is_verb(&lemmatize(&lw));
        if Lexicon::is_function_word(&lw) && !known_verb {
            return Err(self.err(&format!("'{}' is not a verb", verb_raw)));
        }
        // A capitalized name in verb position means a clause boundary was
        // missed ("John owns a dog and Mary owns a cat."), not an unknown verb.
        if Self::is_name(&verb_raw, false) && !known_verb {
            return Err(self.err(&format!("'{}' is a name, not a verb", verb_raw)));
        }
        self.pos += 1;

        let lemma = lemmatize(&verb_raw);

        // Check for unknown verb
        if !self.lex.is_verb(&lemma) && !Lexicon::is_function_word(&lemma) {
            self.unknowns.push(lemma.clone());
        }

        // Skip adverbs after verb
        self.skip_adverbs();

        // "calculate"/"compute"/"evaluate" takes an arithmetic expression
        if matches!(lemma.as_str(), "calculate" | "compute" | "evaluate") {
            let expr = self.parse_arith_expr()?;
            let adjuncts = self.parse_pps(parts)?;
            parts.conds.push(Pred {
                pred: lemma,
                args: vec![Term::Var { var: subj.to_string() }, Term::Arith { expr }],
                negated,
                modal,
                adjuncts,
                attr: None,
            });
            return Ok(());
        }

        // "tell X that S" special form
        if lemma == "tell" {
            return self.parse_tell_pred(subj, parts, negated, modal);
        }

        // Embedding verbs: says that S, believes that S, etc.
        let embed_verbs = ["say","believe","think","know","report","want","ask","mean","deny","assert","claim"];
        let is_embed = embed_verbs.contains(&lemma.as_str());

        let mut args = vec![Term::Var { var: subj.to_string() }];

        // Try to parse object NP (optional)
        if !self.is_terminator() && !self.at_end() && self.pw().as_deref() != Some("and") {
            // Check if "that" follows -> embedded clause
            if is_embed && self.pw().as_deref() == Some("that") {
                self.pos += 1;
                let sub = self.parse_sub_clause()?;
                args.push(Term::Sub { clause: Box::new(sub) });
            } else if Lexicon::is_prep(self.pw().as_deref().unwrap_or(""))
                && !self.is_count_det_start()
            {
                // No object, just PPs
            } else if self.pw().as_deref() == Some("that") && !is_embed {
                // Relative clause on object? Skip for now
            } else {
                // Try to parse object NP
                let saved = self.save();
                match self.parse_np(parts) {
                    Ok(obj_var) => {
                        args.push(Term::Var { var: obj_var });
                        // Skip adverbs after object
                        self.skip_adverbs();
                        // Check for "that S" (embedded)
                        if is_embed && self.eat_word("that") {
                            let sub = self.parse_sub_clause()?;
                            args.push(Term::Sub { clause: Box::new(sub) });
                        }
                    }
                    Err(_) => { self.restore(saved); }
                }
            }
        }

        // Gerund complement: "stops retrying the task" / "stops decreasing Y"
        // When no object NP was captured AND next word is V-ing, parse it as a
        // complementary VP with the same subject, then push the main pred as-is.
        if args.len() == 1 && !self.is_terminator() && !self.at_end()
            && self.pw().as_deref() != Some("and")
        {
            if let Some(raw) = self.praw().map(str::to_string) {
                let lw = raw.to_lowercase();
                if lw.ends_with("ing") && lw.len() > 4
                    && !Lexicon::is_function_word(&lw) && !Lexicon::is_prep(&lw)
                {
                    let ger_saved = self.save();
                    if self.parse_verb_pred(subj, parts, false, false, None).is_ok() {
                        // Gerund VP consumed; fall through to push the main pred.
                    } else {
                        self.restore(ger_saved);
                    }
                }
            }
        }

        let adjuncts = self.parse_pps(parts)?;

        // "that S" at end for embedding verbs not yet consumed
        let has_sub = args.iter().any(|a| matches!(a, Term::Sub { .. }));
        if is_embed && !has_sub && self.eat_word("that") {
            let sub = self.parse_sub_clause()?;
            args.push(Term::Sub { clause: Box::new(sub) });
        }

        parts.conds.push(Pred {
            pred: lemma,
            args,
            negated,
            modal,
            adjuncts,
            attr: None,
        });
        let _ = third_sg;
        Ok(())
    }

    fn parse_tell_pred(&mut self, subj: &str, parts: &mut Parts, negated: bool, modal: Option<Modal>) -> Result<(), ParseError> {
        // tell X that S  OR  tell X NP (tell X a report)
        let mut args = vec![Term::Var { var: subj.to_string() }];
        // Recipient NP
        if !self.is_terminator() && !self.at_end() {
            let saved = self.save();
            match self.parse_np(parts) {
                Ok(rec_var) => { args.push(Term::Var { var: rec_var }); }
                Err(_) => { self.restore(saved); }
            }
        }
        // "that S" or object NP
        if self.eat_word("that") {
            let sub = self.parse_sub_clause()?;
            args.push(Term::Sub { clause: Box::new(sub) });
        } else if !self.is_terminator() && !self.at_end() && self.pw().as_deref() != Some("and") {
            if !Lexicon::is_prep(self.pw().as_deref().unwrap_or("")) {
                let saved = self.save();
                match self.parse_np(parts) {
                    Ok(obj_var) => { args.push(Term::Var { var: obj_var }); }
                    Err(_) => { self.restore(saved); }
                }
            }
        }
        let adjuncts = self.parse_pps(parts)?;
        parts.conds.push(Pred { pred: "tell".to_string(), args, negated, modal, adjuncts, attr: None });
        Ok(())
    }

    fn parse_sub_clause(&mut self) -> Result<Clause, ParseError> {
        // Parse a sentence-like structure (no terminator) as a sub-clause
        let mut inner = Parts::default();
        // Save name map state (inner clause can introduce new names, but outer variables persist)
        let saved_map = self.name_map.clone();
        // Handle "it is possible that" inside sub-clause
        if self.pw().as_deref() == Some("it") && self.pw2().as_deref() == Some("is") {
            let sce_tmp = String::new();
            let it_clause = self.parse_it_sentence(&sce_tmp)?;
            // Strip the terminator that parse_it_sentence may have consumed
            return Ok(it_clause);
        }
        let res = self.parse_decl_into(&mut inner);
        if res.is_err() {
            self.name_map = saved_map;
            return res.map(|_| unreachable!());
        }
        Ok(Clause { act: Act::Assert, referents: inner.refs, conditions: inner.conds, then: vec![], then_referents: vec![], sce: String::new() })
    }

    fn parse_arith_expr(&mut self) -> Result<ArithExpr, ParseError> {
        let mut ap = ArithParser::new(&self.toks, self.pos);
        match ap.parse_expr() {
            Ok(e) => {
                self.pos = ap.pos;
                Ok(e)
            }
            Err(msg) => Err(self.err(&msg)),
        }
    }

    fn skip_adverbs(&mut self) {
        // Skip common adverbs that don't affect parse structure
        while let Some(w) = self.pw() {
            if matches!(w.as_str(),
                "usually"|"often"|"always"|"never"|"sometimes"|"repeatedly"|"frequently"|
                "generally"|"typically"|"normally"|"currently"|"already"|"still"|"yet"|
                "soon"|"quickly"|"slowly"|"carefully"|"gradually"|"suddenly"|"also"|
                "further"|"now"|"immediately"|"eventually"|"temporarily"
            ) {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    // -----------------------------------------------------------------------
    // PP parsing
    // -----------------------------------------------------------------------

    fn parse_pps(&mut self, parts: &mut Parts) -> Result<Vec<(String, Term)>, ParseError> {
        let mut adjuncts = vec![];
        loop {
            let prep = match self.pw() {
                Some(w) if Lexicon::is_prep(&w) => {
                    // Stranded preposition ("... give the apple to?"): leave it
                    // for the wh rule, which owns its object.
                    if self.is_terminator_at(self.pos + 1) { break; }
                    let p = w.clone(); self.pos += 1; p
                }
                _ => break,
            };
            let np_var = self.parse_prep_object(parts);
            if np_var.is_empty() { break; }
            adjuncts.push((prep.clone(), Term::Var { var: np_var }));
            // "moves to the kitchen or the hallway": every alternative is an
            // adjunct of the same preposition.
            loop {
                let saved = self.save();
                if !self.eat_word("or") { break; }
                match self.parse_np(parts) {
                    Ok(alt) => adjuncts.push((prep.clone(), Term::Var { var: alt })),
                    Err(_) => { self.restore(saved); break; }
                }
            }
        }
        Ok(adjuncts)
    }

    /// The object of a preposition: a full NP, else a bare noun ("at home",
    /// "about death", "afraid of mice"). Unknown bare nouns are singularized
    /// and reported. Empty string when nothing can be read.
    fn parse_prep_object(&mut self, parts: &mut Parts) -> String {
        if let Ok(v) = self.parse_np(parts) {
            return v;
        }
        let Some(raw) = self.praw().map(str::to_string) else { return String::new() };
        let lw = raw.to_lowercase();
        if Lexicon::is_function_word(&lw) || Lexicon::is_prep(&lw) || Self::is_var(&raw)
            || self.is_terminator()
        {
            return String::new();
        }
        let singular = singularize_noun(&lw);
        self.pos += 1;
        let var = self.next_var();
        if !self.lex.is_noun(&lw) {
            self.unknowns.push(singular.clone());
        }
        parts.refs.push(Referent {
            var: var.clone(), noun: Some(singular),
            quant: Quant::Indef, mods: vec![], owner: None, span: None,
        });
        var
    }

    /// `Who does John give the apple to?`: the trailing preposition takes the
    /// wh-referent as its object.
    fn attach_stranded_prep(&mut self, parts: &mut Parts, wh_var: &str) {
        let Some(prep) = self.pw().filter(|w| Lexicon::is_prep(w)) else { return };
        if !self.is_terminator_at(self.pos + 1) { return; }
        self.pos += 1;
        if let Some(last) = parts.conds.last_mut() {
            last.adjuncts.push((prep, Term::Var { var: wh_var.to_string() }));
        }
    }

    // -----------------------------------------------------------------------
    // NP parsing
    // -----------------------------------------------------------------------

    fn parse_np(&mut self, parts: &mut Parts) -> Result<String, ParseError> {
        // "the Noun of NP" - property access
        if self.pw().as_deref() == Some("the") {
            let saved = self.save();
            if let Ok(v) = self.try_the_of_np(parts) {
                return Ok(v);
            }
            self.restore(saved);
        }

        // Determiner-headed NP. Save/restore so a spurious number-as-count-det
        // (e.g. "3" in "the double of 3 is 6") doesn't permanently advance pos.
        {
            let det_saved = self.save();
            if let Some((quant, explicit_var)) = self.try_parse_det() {
                match self.finish_det_np(parts, quant, explicit_var) {
                    Ok(v) => return Ok(v),
                    Err(_) => self.restore(det_saved),
                }
            }
        }

        // Name or Variable (capitalized word)
        if let Some(raw) = self.praw().map(str::to_string) {
            let is_init = self.is_initial();
            if Self::is_var(&raw) {
                self.pos += 1;
                // Variable (resolve existing or introduce new)
                let var_name = if let Some(existing) = self.name_map.get(&raw).cloned() {
                    existing
                } else {
                    self.name_map.insert(raw.clone(), raw.clone());
                    parts.refs.push(Referent {
                        var: raw.clone(), noun: None,
                        quant: Quant::Named(raw.clone()),
                        mods: vec![], owner: None, span: None,
                    });
                    raw.clone()
                };
                // Possessive: Var 's Noun -> new Indef referent owned by Var
                if matches!(self.peek(), Some(Tok::AposS)) {
                    self.pos += 1;
                    let (mods, noun) = self.parse_adj_noun();
                    let poss_var = self.next_var();
                    parts.refs.push(Referent {
                        var: poss_var.clone(), noun: Some(noun),
                        quant: Quant::Indef, mods, owner: Some(var_name), span: None,
                    });
                    return Ok(poss_var);
                }
                return Ok(var_name);
            }
            if Self::is_name(&raw, is_init) {
                self.pos += 1;
                let var = self.name_var(&raw);
                // Add referent if new
                if !parts.refs.iter().any(|r| r.var == var) {
                    parts.refs.push(Referent {
                        var: var.clone(), noun: None,
                        quant: Quant::Named(raw.clone()),
                        mods: vec![], owner: None, span: None,
                    });
                }
                // Possessive continuation: Name 's Noun
                if matches!(self.peek(), Some(Tok::AposS)) {
                    self.pos += 1;
                    let (mods, noun) = self.parse_adj_noun();
                    let poss_var = self.next_var();
                    parts.refs.push(Referent {
                        var: poss_var.clone(), noun: Some(noun),
                        quant: Quant::Indef, mods, owner: Some(var), span: None,
                    });
                    return Ok(poss_var);
                }
                return Ok(var);
            }
        }

        // Indefinite pronouns: something, someone, somebody, anything, anyone, everybody, etc.
        if let Some(w) = self.pw() {
            let indef_match: Option<(Quant, &str)> = match w.as_str() {
                "something" | "anything" => Some((Quant::Indef, "thing")),
                "someone" | "somebody" | "anyone" | "anybody" => Some((Quant::Indef, "person")),
                "everything" | "everybody" | "everyone" => Some((Quant::Every, "thing")),
                "nothing" | "nobody" | "no-one" => Some((Quant::No, "thing")),
                _ => None,
            };
            if let Some((quant, noun)) = indef_match {
                self.pos += 1;
                let var = self.next_var();
                parts.refs.push(Referent {
                    var: var.clone(), noun: Some(noun.to_string()),
                    quant, mods: vec![], owner: None, span: None,
                });
                return Ok(var);
            }
        }

        // Quoted string, number, path, url
        match self.peek() {
            Some(Tok::Quoted(s)) => {
                let s = s.clone(); self.pos += 1;
                let var = self.next_var();
                parts.refs.push(Referent { var: var.clone(), noun: None, quant: Quant::Literal(Value::Text(s)), mods: vec![], owner: None, span: None });
                return Ok(var);
            }
            Some(Tok::Number(n)) => {
                let n = *n; self.pos += 1;
                return Ok(self.push_number(parts, n));
            }
            // The tokenizer reads "-" after a word as an operator, so a negative
            // literal in NP position ("of -2") arrives as Minus, Number.
            Some(Tok::Minus) if matches!(self.peek2(), Some(Tok::Number(_))) => {
                self.pos += 1;
                let n = match self.peek() { Some(Tok::Number(n)) => *n, _ => unreachable!() };
                self.pos += 1;
                return Ok(self.push_number(parts, -n));
            }
            Some(Tok::Path(s)) => {
                let s = s.clone(); self.pos += 1;
                let var = self.next_var();
                parts.refs.push(Referent { var: var.clone(), noun: None, quant: Quant::Literal(Value::Path(s)), mods: vec![], owner: None, span: None });
                return Ok(var);
            }
            Some(Tok::Url(s)) => {
                let s = s.clone(); self.pos += 1;
                let var = self.next_var();
                parts.refs.push(Referent { var: var.clone(), noun: None, quant: Quant::Literal(Value::Url(s)), mods: vec![], owner: None, span: None });
                return Ok(var);
            }
            _ => {}
        }

        // Bare noun (no determiner): known noun that is not also a (full) verb.
        // Use is_verb (not is_core_verb) to exclude corpus-seeded verb lookalikes.
        // Last resort - only when unambiguously a noun.
        if let Some(raw) = self.praw().map(str::to_string) {
            let lw = raw.to_lowercase();
            let lemma = lemmatize(&lw);
            let is_noun = self.lex.is_noun(&lw) || self.lex.is_noun(&lemma);
            let is_verb = self.lex.is_verb(&lw) || self.lex.is_verb(&lemma);
            let is_func = Lexicon::is_function_word(&lw) || Lexicon::is_prep(&lw);
            if !is_func && !Self::is_var(&raw) && is_noun && !is_verb {
                self.pos += 1;
                let singular = singularize_noun(&lw);
                let var = self.next_var();
                parts.refs.push(Referent {
                    var: var.clone(), noun: Some(singular),
                    quant: Quant::Indef, mods: vec![], owner: None, span: None,
                });
                return Ok(var);
            }
        }

        Err(self.err("Expected noun phrase"))
    }

    fn push_number(&mut self, parts: &mut Parts, n: f64) -> String {
        let var = self.next_var();
        let val = if n.fract() == 0.0 { Value::Int(n as i64) } else { Value::Float(n) };
        parts.refs.push(Referent { var: var.clone(), noun: None, quant: Quant::Literal(val), mods: vec![], owner: None, span: None });
        var
    }

    fn try_the_of_np(&mut self, parts: &mut Parts) -> Result<String, ParseError> {
        self.pos += 1; // consume 'the'
        // Parse optional adjectives + noun using lenient core-verb check.
        // This allows "the main city of New-York" and "the wellbeing of Assistant".
        let mut candidates: Vec<String> = vec![];
        while let Some(w) = self.pw() {
            if self.is_the_of_np_breaker(&w) { break; }
            // Stop if next token is "of" (that's the separator, not the noun)
            if w.as_str() == "of" { break; }
            candidates.push(w);
            self.pos += 1;
        }
        if candidates.is_empty() {
            return Err(self.err("Expected noun after 'the'"));
        }
        let noun = candidates.last().cloned().unwrap_or_default();
        let mods: Vec<String> = candidates[..candidates.len() - 1].to_vec();
        if !self.eat_word("of") {
            return Err(self.err("Expected 'of' in 'the N of NP'"));
        }
        let of_var = self.parse_np(parts)?;
        let var = self.next_var();
        parts.refs.push(Referent {
            var: var.clone(), noun: Some(noun), quant: Quant::Def,
            mods, owner: Some(of_var), span: None,
        });
        Ok(var)
    }

    fn try_parse_det(&mut self) -> Option<(Quant, Option<String>)> {
        // Bare number token: "10 apples" -> Count(10) NP
        if let Some(Tok::Number(n)) = self.peek() {
            // Only treat as a count determiner if followed by a word (noun)
            if matches!(self.peek2(), Some(Tok::Word(_))) {
                let n = *n as u32;
                self.pos += 1;
                return Some((Quant::Count(n), None));
            }
        }
        let w = self.pw()?;
        match w.as_str() {
            "a" => {
                self.pos += 1;
                // "an unknown" special det
                if self.pw().as_deref() == Some("unknown") {
                    self.pos += 1;
                    return Some((Quant::Indef, None));
                }
                Some((Quant::Indef, None))
            }
            "an" => {
                self.pos += 1;
                if self.pw().as_deref() == Some("unknown") {
                    self.pos += 1;
                }
                Some((Quant::Indef, None))
            }
            "the" => { self.pos += 1; Some((Quant::Def, None)) }
            "every" => { self.pos += 1; Some((Quant::Every, None)) }
            "some" => { self.pos += 1; Some((Quant::Indef, None)) } // some->Indef
            "no" => { self.pos += 1; Some((Quant::No, None)) }
            "not" if self.pw2().as_deref() == Some("every") => {
                // "Not every" -> use Every with negated predicate (see design notes)
                self.pos += 2;
                Some((Quant::Every, None))
            }
            "at" => {
                if self.pw2().as_deref() == Some("least") {
                    self.pos += 2;
                    if let Some(n) = self.parse_small_number() {
                        return Some((Quant::AtLeast(n), None));
                    }
                    return Some((Quant::AtLeast(1), None));
                }
                if self.pw2().as_deref() == Some("most") {
                    self.pos += 2;
                    if let Some(n) = self.parse_small_number() {
                        return Some((Quant::AtMost(n), None));
                    }
                    return Some((Quant::AtMost(1), None));
                }
                None
            }
            "exactly" => {
                self.pos += 1;
                let n = self.parse_small_number().unwrap_or(1);
                Some((Quant::Exactly(n), None))
            }
            "more" if self.pw2().as_deref() == Some("than") => {
                self.pos += 2;
                let n = self.parse_small_number().unwrap_or(0);
                Some((Quant::AtLeast(n + 1), None))
            }
            _ => {
                // Number word or bare number: "10 apples", "five dogs"
                if let Some(n) = number_word(&w) {
                    self.pos += 1;
                    return Some((Quant::Count(n), None));
                }
                if let Some(Tok::Number(n)) = self.peek() {
                    let n = *n as u32;
                    self.pos += 1;
                    return Some((Quant::Count(n), None));
                }
                None
            }
        }
    }

    fn parse_small_number(&mut self) -> Option<u32> {
        if let Some(w) = self.pw() {
            if let Some(n) = number_word(&w) {
                self.pos += 1;
                return Some(n);
            }
        }
        if let Some(Tok::Number(n)) = self.peek() {
            let n = *n as u32;
            self.pos += 1;
            return Some(n);
        }
        None
    }

    fn finish_det_np(&mut self, parts: &mut Parts, quant: Quant, _explicit_var: Option<String>) -> Result<String, ParseError> {
        // Parse adj* noun
        let (mods, noun) = self.parse_adj_noun();
        if noun.is_empty() {
            return Err(self.err("Expected noun after determiner"));
        }
        // Check for explicit variable after noun: "a file X"
        let var = if let Some(raw) = self.praw().map(str::to_string) {
            if Self::is_var(&raw) {
                self.pos += 1;
                // Use the capital-letter variable as the var name
                self.name_map.insert(raw.clone(), raw.clone());
                raw
            } else {
                self.next_var()
            }
        } else {
            self.next_var()
        };

        let r = Referent { var: var.clone(), noun: Some(noun), quant, mods, owner: None, span: None };

        // Parse inline PP on NP (attaches as Pred rather than adjunct to keep NP clean)
        // "the file in the folder" -> NP file with PP as condition
        // We do this after building the referent
        parts.refs.push(r.clone());

        // Parse optional "that VP" relative clause
        if self.pw().as_deref() == Some("that") || self.pw().as_deref() == Some("who") {
            self.pos += 1;
            // Parse the relative VP
            self.parse_rel_clause(&var, parts)?;
        }

        // Inline PP directly after the NP noun (before relative clause)
        // Already handled above; inline PPs on noun become conditions
        // "the file in the folder" - we check for prep after relative clause
        // Actually, we want to handle PPs BEFORE "that" relative clause
        // But our current order is: noun -> var -> that VP. Let's handle PP here too.
        // For simplicity, we just return the var; PP on verb will be adjuncts.

        Ok(var)
    }

    fn parse_adj_noun(&mut self) -> (Vec<String>, String) {
        // Scan ahead to collect candidate adj/noun words.
        // Stop at: function words, preps, vars, names, terminator, non-word toks,
        //          and words that are pure verbs (verb but NOT noun in lexicon).
        let mut candidates: Vec<(usize, String)> = vec![];
        let mut scan = self.pos;
        loop {
            if scan >= self.toks.len() { break; }
            match &self.toks[scan] {
                Tok::Word(w) => {
                    let lw = w.to_lowercase();
                    let raw = w.as_str();
                    if Lexicon::is_function_word(&lw) || Lexicon::is_prep(&lw) { break; }
                    if Self::is_var(raw) || Self::is_name(raw, false) { break; }
                    // A word directly before "than" is a comparative adjective
                    // ("bigger than"), never an NP head, whatever the lexicon says.
                    if !candidates.is_empty()
                        && matches!(self.toks.get(scan + 1), Some(Tok::Word(n)) if n.eq_ignore_ascii_case("than"))
                    {
                        break;
                    }
                    // Pure verb (not also a noun) stops the NP.
                    // For words that are BOTH noun and verb, stop only if the word is
                    // clearly acting as a verb form:
                    //   - Unambiguously verbal suffix: -ed, -ing, -ies
                    //   - OR: the word itself (not just its lemma) is in the verb set
                    //     AND lw != lemma (meaning the verb set explicitly tracks this
                    //     inflected form, e.g. DEFAULT_VERBS has "sleeps" explicitly).
                    let is_noun = self.lex.is_noun(&lw);
                    let lemma = lemmatize(&lw);
                    let is_verb_direct = self.lex.is_verb(&lw);
                    let is_verb = is_verb_direct || self.lex.is_verb(&lemma);
                    // A word is a "clear verb form" (stops the NP scan) if:
                    // - It ends in -ed/-ing/-ies (unambiguously verbal)
                    // - OR it is directly in the verb set as an inflected form
                    // - OR it ends in -s, its lemma is a known verb, and the lemma is NOT
                    //   a known noun (distinguishes "owns"->verb from "cards"->plural-noun)
                    let is_clear_verb_form = lw != lemma && (
                        lw.ends_with("ed") || lw.ends_with("ing") || lw.ends_with("ies")
                        || is_verb_direct
                        // 3sg -s: verb if lemma is verb AND lemma is not a CORE (default) noun.
                        // (seeded "own" appears as noun from corpus phrases, but "card" is a core noun.)
                        || (lw.ends_with('s') && self.lex.is_verb(&lemma) && !self.lex.is_core_noun(&lemma))
                    );
                    if is_verb && (!is_noun || is_clear_verb_form) { break; }
                    candidates.push((scan, lw));
                    scan += 1;
                }
                _ => break,
            }
        }
        if candidates.is_empty() {
            return (vec![], String::new());
        }
        // Head noun selection (rightmost-first priority):
        // 1. Rightmost CORE noun (DEFAULT_NOUNS) that is not a core verb.
        //    Prevents corpus-seeded comparative adjectives ("bigger") from
        //    stealing the head from an earlier core noun ("dog").
        // 2. Rightmost known noun that is not a core verb.
        // 3. Rightmost known noun (even if also a core verb).
        // 4. Last candidate word.
        let noun_idx = candidates.iter()
            .rposition(|(_, w)| {
                let lm = lemmatize(w);
                self.lex.is_core_noun(w) && !self.lex.is_core_verb(w) && !self.lex.is_core_verb(&lm)
            })
            .or_else(|| candidates.iter().rposition(|(_, w)| {
                let lm = lemmatize(w);
                self.lex.is_noun(w) && !self.lex.is_core_verb(w) && !self.lex.is_core_verb(&lm)
            }))
            .or_else(|| candidates.iter().rposition(|(_, w)| self.lex.is_noun(w)))
            .unwrap_or(candidates.len() - 1);
        let noun = candidates[noun_idx].1.clone();
        let mods: Vec<String> = candidates[..noun_idx].iter().map(|(_, w)| w.clone()).collect();
        // Unknown words
        if !self.lex.is_noun(&noun) { self.unknowns.push(noun.clone()); }
        for m in &mods {
            if !self.lex.is_adjective(m) && !Lexicon::is_function_word(m) {
                self.unknowns.push(m.clone());
            }
        }
        self.pos = candidates[noun_idx].0 + 1;
        (mods, noun)
    }

    fn parse_rel_clause(&mut self, np_var: &str, parts: &mut Parts) -> Result<(), ParseError> {
        // Two patterns:
        // 1. Subject relative: "a call that rings" -> np_var is the VP subject
        // 2. Object relative: "a call that Mary makes" -> explicit NP subject, np_var is object
        let saved = self.save();

        // Try object relative first: if next looks like an NP (not a verb), parse as new subject
        // Use praw() (original case) for is_name check.
        let next_is_verb = self.praw()
            .map(|raw| {
                let lw = raw.to_lowercase();
                let lem = lemmatize(&lw);
                !Lexicon::is_function_word(&lw) && (self.lex.is_verb(&lw) || self.lex.is_verb(&lem))
                    && !Self::is_name(raw, false) && !self.lex.is_core_noun(&lw)
            })
            .unwrap_or(false);

        if !next_is_verb {
            // Try object relative: NP VP (where np_var is the object)
            if let Ok(rel_subj) = self.parse_np(parts) {
                let subj_saved = self.save();
                if self.parse_vp(&rel_subj, parts, false, false).is_ok() {
                    // Add np_var as the final object arg of the last predicate
                    if let Some(last_pred) = parts.conds.last_mut() {
                        last_pred.args.push(Term::Var { var: np_var.to_string() });
                    }
                    return Ok(());
                }
                self.restore(subj_saved);
            }
        }
        self.restore(saved);

        // Subject relative: VP with np_var as subject
        self.parse_vp(np_var, parts, false, false)
    }

    // -----------------------------------------------------------------------
    // Command sentence
    // -----------------------------------------------------------------------

    fn parse_command_sentence(&mut self, sce: &str) -> Result<Clause, ParseError> {
        self.expect_word("assistant")?;
        if !self.eat_tok(&Tok::Comma) { return Err(self.err("Expected ',' after 'Assistant'")); }

        let mut parts = Parts::default();
        // "do not VP"
        let negated = if self.pw().as_deref() == Some("do") && self.pw2().as_deref() == Some("not") {
            self.pos += 2; true
        } else {
            false
        };
        // Assistant referent
        let ass_var = self.name_var("Assistant");
        if !parts.refs.iter().any(|r| r.var == ass_var) {
            parts.refs.push(Referent { var: ass_var.clone(), noun: None, quant: Quant::Named("Assistant".to_string()), mods: vec![], owner: None, span: None });
        }
        // VPimp
        self.parse_command_vp(&ass_var, &mut parts, negated)?;
        // "and VPimp" continuation
        while self.eat_word("and") {
            self.parse_command_vp(&ass_var, &mut parts, false)?;
        }
        let term = self.eat_terminator_type();
        let _ = term;
        Ok(Clause { act: Act::Command, referents: parts.refs, conditions: parts.conds, then: vec![], then_referents: vec![], sce: sce.to_string() })
    }

    fn parse_command_vp(&mut self, subj: &str, parts: &mut Parts, negated: bool) -> Result<(), ParseError> {
        self.parse_verb_pred(subj, parts, false, negated, None)
    }

    // -----------------------------------------------------------------------
    // Question sentence
    // -----------------------------------------------------------------------

    fn parse_question_sentence(&mut self, sce: &str, kw: &str) -> Result<Clause, ParseError> {
        let mut parts = Parts::default();
        let kind = match kw {
            "who" => self.parse_who_question(&mut parts)?,
            "what" => self.parse_what_question(&mut parts)?,
            "which" => self.parse_which_question(&mut parts)?,
            "where" => self.parse_where_question(&mut parts)?,
            "when" => self.parse_when_question(&mut parts)?,
            "how" => self.parse_how_question(&mut parts)?,
            "does" => self.parse_does_question(&mut parts)?,
            "is" | "are" => self.parse_is_question(&mut parts)?,
            "can" | "should" | "must" | "may" => self.parse_modal_question(kw, &mut parts)?,
            _ => return Err(self.err(&format!("Unknown question type '{kw}'"))),
        };
        self.eat_terminator();
        Ok(Clause { act: Act::Question { kind }, referents: parts.refs, conditions: parts.conds, then: vec![], then_referents: vec![], sce: sce.to_string() })
    }

    fn parse_who_question(&mut self, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        self.expect_word("who")?;
        // "Who is NP?" or "Who VP?"
        let wh_var = self.next_var();
        parts.refs.push(Referent { var: wh_var.clone(), noun: Some("person".to_string()), quant: Quant::Wh, mods: vec![], owner: None, span: None });
        if self.pw().as_deref() == Some("is") {
            self.pos += 1;
            // "Who is John?" -> condition: be(Wh, John)
            let saved = self.save();
            if let Ok(np_var) = self.parse_np(parts) {
                parts.conds.push(Pred { pred: "be".to_string(), args: vec![Term::Var { var: wh_var.clone() }, Term::Var { var: np_var }], negated: false, modal: None, adjuncts: vec![], attr: None });
            } else { self.restore(saved); }
        } else if matches!(self.pw().as_deref(), Some("does") | Some("do")) {
            // "Who does John give the apple to?" -> give(John, apple) to Wh
            self.pos += 1;
            let subj = self.parse_np(parts)?;
            self.parse_vp(&subj, parts, false, true)?;
            self.attach_stranded_prep(parts, &wh_var);
        } else {
            self.parse_vp(&wh_var, parts, false, false)?;
        }
        Ok(QuestionKind::Who { focus: wh_var })
    }

    fn parse_what_question(&mut self, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        self.expect_word("what")?;
        // "What color is every wolf?" -> the property of the named NP.
        if !matches!(self.pw().as_deref(), Some("is") | Some("are") | Some("does") | Some("do")) {
            let m = self.mark(parts);
            match self.try_what_property(parts) {
                Ok(kind) => return Ok(kind),
                Err(_) => self.rollback(parts, m),
            }
        }
        // "What is NP?" -> What{focus = NP var}
        if self.pw().as_deref() == Some("is") || self.pw().as_deref() == Some("are") {
            self.pos += 1;
            // "What is south of the office?" -> south-of(Wh, office)
            let m = self.mark(parts);
            let rel_wh = self.next_var();
            parts.refs.push(Referent { var: rel_wh.clone(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None });
            if self.try_relation_of(&rel_wh, parts, false).is_ok() {
                return Ok(QuestionKind::What { focus: rel_wh });
            }
            self.rollback(parts, m);
            // The wh-referent is pushed last so this parse and the
            // "What <noun> is NP?" one order their referents alike.
            let np_var = self.parse_np(parts)?;
            let wh_var = self.next_var();
            parts.refs.push(Referent { var: wh_var.clone(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None });
            parts.conds.push(Pred { pred: "be".to_string(), args: vec![Term::Var { var: wh_var }, Term::Var { var: np_var.clone() }], negated: false, modal: None, adjuncts: vec![], attr: None });
            return Ok(QuestionKind::What { focus: np_var });
        }
        // "What does NP VP?" or "What VP?"
        let wh_var = self.next_var();
        parts.refs.push(Referent { var: wh_var.clone(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None });
        if self.pw().as_deref() == Some("does") {
            self.pos += 1;
            let subj = self.parse_np(parts)?;
            self.parse_vp(&subj, parts, false, true)?;
            self.attach_stranded_prep(parts, &wh_var);
        } else {
            self.parse_vp(&wh_var, parts, false, false)?;
        }
        Ok(QuestionKind::What { focus: wh_var })
    }

    /// `What color is every wolf?` == `What is the color of every wolf?`:
    /// the asked noun is a definite property of the following NP.
    fn try_what_property(&mut self, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        let (mods, noun) = self.parse_adj_noun();
        if noun.is_empty() {
            return Err(self.err("Expected a noun after 'What'"));
        }
        if !self.eat_word("is") && !self.eat_word("are") {
            return Err(self.err("Expected 'is' after 'What <noun>'"));
        }
        let owner = self.parse_np(parts)?;
        let prop_var = self.next_var();
        parts.refs.push(Referent {
            var: prop_var.clone(),
            noun: Some(noun),
            quant: Quant::Def,
            mods,
            owner: Some(owner),
            span: None,
        });
        let wh_var = self.next_var();
        parts.refs.push(Referent { var: wh_var.clone(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None });
        parts.conds.push(Pred {
            pred: "be".to_string(),
            args: vec![Term::Var { var: wh_var }, Term::Var { var: prop_var.clone() }],
            negated: false,
            modal: None,
            adjuncts: vec![],
            attr: None,
        });
        Ok(QuestionKind::What { focus: prop_var })
    }

    fn parse_which_question(&mut self, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        self.expect_word("which")?;
        let (mods, noun) = self.parse_adj_noun();
        let wh_var = self.next_var();
        parts.refs.push(Referent { var: wh_var.clone(), noun: if noun.is_empty() { None } else { Some(noun) }, quant: Quant::Wh, mods, owner: None, span: None });
        // "Which N should/can/must/may Subject Verb?" -- wh-referent is the object.
        // Speculative: "Which person should own the task?" has no explicit
        // subject, so on any failure roll back and treat the wh-referent as subject.
        let m = self.mark(parts);
        if let Some(modal) = self.parse_modal() {
            let negated = self.eat_word("not");
            match self.try_which_object(parts, &wh_var, modal, negated) {
                Ok(()) => return Ok(QuestionKind::Which { focus: wh_var }),
                Err(_) => self.rollback(parts, m),
            }
        }
        // "Which Noun VP?" -- wh-referent is the subject
        self.parse_vp(&wh_var, parts, false, false)?;
        Ok(QuestionKind::Which { focus: wh_var })
    }

    fn try_which_object(&mut self, parts: &mut Parts, wh_var: &str, modal: Modal, negated: bool) -> Result<(), ParseError> {
        let subj = self.parse_np(parts)?;
        let verb_raw = match self.praw() {
            Some(v) if !Lexicon::is_function_word(&v.to_lowercase()) => v.to_string(),
            _ => return Err(self.err("Expected verb in which-question")),
        };
        self.pos += 1;
        let verb_lemma = lemmatize(&verb_raw);
        if !self.lex.is_verb(&verb_lemma) { self.unknowns.push(verb_lemma.clone()); }
        let adjuncts = self.parse_pps(parts)?;
        if !self.is_terminator() {
            return Err(self.err("Expected end of which-question"));
        }
        parts.conds.push(Pred {
            pred: verb_lemma,
            args: vec![Term::Var { var: subj }, Term::Var { var: wh_var.to_string() }],
            negated,
            modal: Some(modal),
            adjuncts,
            attr: None,
        });
        Ok(())
    }

    fn parse_where_question(&mut self, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        self.expect_word("where")?;
        if self.eat_word("is") || self.eat_word("are") {}
        let subj = self.parse_np(parts)?;
        let wh_var = self.next_var();
        parts.refs.push(Referent { var: wh_var.clone(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None });
        parts.conds.push(Pred { pred: "be".to_string(), args: vec![Term::Var { var: subj.clone() }, Term::Var { var: wh_var.clone() }], negated: false, modal: None, adjuncts: vec![], attr: None });
        Ok(QuestionKind::Where { focus: wh_var })
    }

    fn parse_when_question(&mut self, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        self.expect_word("when")?;
        if self.eat_word("does") {}
        let subj = self.parse_np(parts)?;
        self.parse_vp(&subj, parts, false, true)?;
        let wh_var = self.next_var();
        parts.refs.push(Referent { var: wh_var.clone(), noun: None, quant: Quant::Wh, mods: vec![], owner: None, span: None });
        Ok(QuestionKind::When { focus: wh_var })
    }

    fn parse_how_question(&mut self, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        self.expect_word("how")?;
        if !self.eat_word("many") {
            return Err(self.err("Only 'How many' is supported"));
        }
        // "How many Noun does NP VP?" or "How many Noun VP?"
        let (mods, noun) = self.parse_adj_noun();
        let wh_var = self.next_var();
        parts.refs.push(Referent { var: wh_var.clone(), noun: if noun.is_empty() { None } else { Some(noun) }, quant: Quant::Wh, mods, owner: None, span: None });
        if self.eat_word("does") || self.eat_word("do") {
            let subj = self.parse_np(parts)?;
            self.parse_vp(&subj, parts, false, true)?;
        } else {
            self.parse_vp(&wh_var, parts, false, false)?;
        }
        Ok(QuestionKind::HowMany { focus: wh_var })
    }

    fn parse_does_question(&mut self, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        self.expect_word("does")?;
        let subj = self.parse_np(parts)?;
        self.parse_vp(&subj, parts, false, true)?;
        Ok(QuestionKind::YesNo)
    }

    fn parse_is_question(&mut self, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        self.eat_word("is"); self.eat_word("are");
        let subj = self.parse_np(parts)?;
        // "Is NP not NP?" handle negated
        let negated = self.eat_word("not");
        // The complement grammar is the declarative one: NP, adjective,
        // comparative, relation-of, or a locative PP.
        if !self.is_terminator() && !self.at_end() {
            let m = self.mark(parts);
            if self.parse_copula_body(&subj, parts, negated).is_err() {
                self.rollback(parts, m);
            }
        }
        Ok(QuestionKind::YesNo)
    }

    fn parse_modal_question(&mut self, kw: &str, parts: &mut Parts) -> Result<QuestionKind, ParseError> {
        let modal = match kw {
            "can" => Modal::Can, "should" => Modal::Should,
            "must" => Modal::Must, "may" => Modal::May,
            _ => return Err(self.err("Unknown modal in question")),
        };
        self.pos += 1; // consume modal word
        let subj = self.parse_np(parts)?;
        let negated = self.eat_word("not");
        self.parse_verb_pred(&subj, parts, false, negated, Some(modal.clone()))?;
        Ok(if kw == "should" { QuestionKind::Should } else { QuestionKind::YesNo })
    }

    // -----------------------------------------------------------------------
    // Rule sentence
    // -----------------------------------------------------------------------

    fn parse_rule_sentence(&mut self, sce: &str, _not_every_negate: bool) -> Result<Clause, ParseError> {
        self.expect_word("if")?;
        let mut ant = Parts::default();
        // Parse antecedent: Decl ('and' Decl)*
        self.parse_decl_into(&mut ant)?;
        loop {
            let saved = self.save();
            if !self.eat_word("and") { break; }
            if self.parse_decl_into(&mut ant).is_err() {
                self.restore(saved);
                break;
            }
        }
        self.expect_word("then")?;
        let mut con = Parts::default();
        self.parse_decl_into(&mut con)?;
        loop {
            let saved = self.save();
            if !self.eat_word("and") { break; }
            if self.parse_decl_into(&mut con).is_err() {
                self.restore(saved);
                break;
            }
        }
        self.eat_terminator();
        Ok(Clause {
            act: Act::Rule,
            referents: ant.refs,
            conditions: ant.conds,
            then_referents: con.refs,
            then: con.conds,
            sce: sce.to_string(),
        })
    }

    fn parse_for_every_sentence(&mut self, sce: &str) -> Result<Clause, ParseError> {
        self.expect_word("for")?;
        self.expect_word("every")?;
        let mut ant = Parts::default();
        // Parse "every Noun X" - the universally quantified NP
        let (mods, noun) = self.parse_adj_noun();
        let bound_var = if let Some(raw) = self.praw().map(str::to_string) {
            if Self::is_var(&raw) {
                self.pos += 1;
                self.name_map.insert(raw.clone(), raw.clone());
                raw
            } else { self.next_var() }
        } else { self.next_var() };
        ant.refs.push(Referent { var: bound_var.clone(), noun: if noun.is_empty() { None } else { Some(noun) }, quant: Quant::Every, mods, owner: None, span: None });
        // "if Cond+ then Cond+"
        if self.eat_word("if") {
            self.parse_decl_into(&mut ant)?;
            loop {
                let saved = self.save();
                if !self.eat_word("and") { break; }
                if self.parse_decl_into(&mut ant).is_err() { self.restore(saved); break; }
            }
            self.expect_word("then")?;
        }
        let mut con = Parts::default();
        self.parse_decl_into(&mut con)?;
        loop {
            let saved = self.save();
            if !self.eat_word("and") { break; }
            if self.parse_decl_into(&mut con).is_err() { self.restore(saved); break; }
        }
        self.eat_terminator();
        Ok(Clause { act: Act::Rule, referents: ant.refs, conditions: ant.conds, then_referents: con.refs, then: con.conds, sce: sce.to_string() })
    }

    // -----------------------------------------------------------------------
    // Terminator helpers
    // -----------------------------------------------------------------------

    fn eat_terminator(&mut self) {
        match self.peek() {
            Some(Tok::Period) | Some(Tok::Bang) | Some(Tok::Question) => { self.pos += 1; }
            _ => {}
        }
    }

    fn eat_terminator_type(&mut self) -> Option<char> {
        match self.peek() {
            Some(Tok::Period) => { self.pos += 1; Some('.') }
            Some(Tok::Bang) => { self.pos += 1; Some('!') }
            Some(Tok::Question) => { self.pos += 1; Some('?') }
            _ => None,
        }
    }

    fn at_end(&self) -> bool { self.pos >= self.toks.len() }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse one SCE sentence into a Clause.
pub fn parse(sentence: &str, lex: &Lexicon) -> Result<(Clause, Vec<String>), ParseError> {
    parse_traced(sentence, lex).map(|(clause, unknowns, _)| (clause, unknowns))
}

/// `parse` plus `(consumed, total)` token counts. Test hook for the
/// no-silent-drop invariant; not part of the public contract.
#[doc(hidden)]
pub fn parse_traced(sentence: &str, lex: &Lexicon) -> Result<(Clause, Vec<String>, (usize, usize)), ParseError> {
    // Validate: must end in . ! ?
    let trimmed = sentence.trim();
    if !matches!(trimmed.chars().last(), Some('.') | Some('!') | Some('?')) {
        return Err(ParseError {
            message: "Sentence must end with '.', '!', or '?'".to_string(),
            position: None,
            unknown_words: vec![],
        });
    }
    // Reject pronouns (he, she, it, they, him, her, them, his, its, their, my, your, our)
    // but allow "It" as sentence-initial in "It is possible that..."
    {
        let toks = tokenize(trimmed);
        for (i, tok) in toks.iter().enumerate() {
            if let Tok::Word(w) = tok {
                let lw = w.to_lowercase();
                if matches!(lw.as_str(), "he"|"she"|"they"|"him"|"her"|"them"|"his"|"their"|"my"|"your"|"our"|"we"|"i"|"me"|"us") {
                    return Err(ParseError {
                        message: format!("Pronouns are not allowed in SCE. Found pronoun '{}'. Use a name, variable, or definite NP instead.", w),
                        position: Some(i),
                        unknown_words: vec![],
                    });
                }
                // "its" is also a pronoun. "it" is a function word and is left
                // to the parser, so the dummy subject of "it is possible that S"
                // works and any other "it" surfaces as a leftover token.
                if lw == "its" {
                    return Err(ParseError {
                        message: "Pronoun 'its' is not allowed in SCE. Use a variable or named referent.".to_string(),
                        position: Some(i),
                        unknown_words: vec![],
                    });
                }
            }
        }
    }
    let toks = tokenize(trimmed);
    let mut p = Parser::new(toks, lex);
    match p.parse_sentence(trimmed) {
        Ok(clause) => {
            // Strict: every token before/at the terminator must be consumed.
            // If tokens remain, the parse silently dropped content - fail instead.
            if !p.at_end() {
                let mut e = p.leftover_err();
                e.unknown_words = p.unknowns;
                return Err(e);
            }
            let consumed = (p.pos, p.toks.len());
            let unknowns = p.unknowns;
            Ok((clause, unknowns, consumed))
        }
        Err(mut e) => {
            e.unknown_words = p.unknowns;
            Err(e)
        }
    }
}

/// Parse text containing multiple SCE sentences.
pub fn parse_text(text: &str, lex: &Lexicon) -> Result<(Vec<Clause>, Vec<String>), (usize, ParseError)> {
    use super::tokenizer::split_sentences;
    let sentences = split_sentences(text);
    let mut clauses = Vec::new();
    let mut all_unknowns = Vec::new();
    for (idx, sent) in sentences.iter().enumerate() {
        match parse(sent, lex) {
            Ok((clause, unknowns)) => {
                clauses.push(clause);
                all_unknowns.extend(unknowns);
            }
            Err(e) => return Err((idx, e)),
        }
    }
    Ok((clauses, all_unknowns))
}
