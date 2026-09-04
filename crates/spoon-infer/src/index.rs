//! A discrimination tree over rule conclusions.
//!
//! Backward chaining asks the same question on every step: which rules could
//! conclude this goal? [`ScanIndex`](crate::ScanIndex) answers it by walking
//! every rule and comparing head symbols. That is correct, and it is the wrong
//! shape once a brain holds thousands of rules, because the cost of one
//! derivation step grows with everything the brain has ever learned.
//!
//! A discrimination tree (McCune) answers the same question by walking a trie
//! keyed on a pre-order flattening of the conclusion. Rules that disagree with
//! the goal in their first differing symbol are never visited at all, so the
//! work is proportional to the size of the goal and the number of rules that
//! actually resemble it.
//!
//! # The flattening
//!
//! Each concept becomes a sequence of tokens in pre-order:
//!
//! - `Atomic(Named(s))` becomes `Sym(s)`.
//! - `Atomic(Ground(g))` becomes `Ground(g)`. Ground identity is the value, so
//!   `404` and `500` are different tokens and discriminate against each other.
//! - `Hole(_)` becomes `Star`. The hole's index carries no discriminating
//!   information: `?0` and `?7` both stand for "anything".
//! - `Compound { head, args }` becomes `Apply(arity)` followed by the head's
//!   flattening and then each argument's, in order.
//!
//! Because `Apply(k)` states up front how many subterms follow, the encoding
//! is self-delimiting: no complete flattening is a prefix of another, and the
//! extent of any subterm can be found by counting. Arity discrimination comes
//! out for free, since `Apply(2)` and `Apply(3)` are different tokens.
//!
//! # Wildcards, in both directions
//!
//! Holes appear on both sides of the question, and each side needs its own
//! branching rule. Getting either one wrong makes rules dead: silently never
//! offered, never fired, with nothing to indicate why.
//!
//! - **Hole in the conclusion.** A rule concluding `Ancestor<?0, ?1>` must be
//!   found by the goal `Ancestor<Greg, Keal>`. So whenever the walk is looking
//!   at a goal token, it also follows the node's `Star` edge, and when it does,
//!   it skips the goal's *entire* subterm at that position. The stored hole
//!   stands for one whole subterm, however large that subterm turns out to be.
//! - **Hole in the goal.** A goal `Ancestor<?0, Keal>` must find rules
//!   concluding `Ancestor<Greg, Keal>`. So whenever the goal token is `Star`,
//!   the walk follows *every* edge far enough to consume one complete stored
//!   subterm, then resumes with the next goal token. That is what [`skip`]
//!   does, and it is why the trie stores arity in the token: without it there
//!   would be no way to know where a stored subterm ends.
//!
//! Both branches over-approximate on purpose. Returning a rule that then fails
//! to unify costs one failed unification; failing to return one that would
//! have unified is a correctness bug with no symptom.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use spoon_concept::{Concept, ConceptId, ContentId, Ground, SymbolId};
use spoon_store::Store;

use crate::error::Result;
use crate::rule::{RuleIndex, StoredRule, load_rules};

/// One symbol of a flattened concept.
///
/// `Apply` carries the arity rather than being a bare "compound starts here"
/// marker for two reasons: arity mismatches are excluded without ever looking
/// at the arguments, and a wildcard in the goal can count its way past a
/// stored subterm without needing a separate jump table.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Token {
    /// A hole, on either side. All holes flatten to the same token because the
    /// index is a filter, not a matcher: which hole it was is the unifier's
    /// business.
    Star,
    Sym(SymbolId),
    /// A ground value, kept whole. `HttpStatusMeaning<404, NotFound>` must not
    /// be offered for a goal about 500, and the only thing that separates them
    /// is the value itself.
    Ground(Ground),
    Apply(usize),
}

/// Pre-order flattening of a concept.
///
/// Iterative rather than recursive: a concept arriving from a seed file or the
/// synthesizer can be arbitrarily deep, and a deep recursion here would take
/// the process down with a stack overflow rather than returning an answer.
fn flatten(term: &Concept) -> Vec<Token> {
    let mut out = Vec::with_capacity(term.size());
    let mut stack: Vec<&Concept> = vec![term];
    while let Some(current) = stack.pop() {
        match current {
            Concept::Atomic(ConceptId::Named(symbol)) => out.push(Token::Sym(*symbol)),
            Concept::Atomic(ConceptId::Ground(ground)) => out.push(Token::Ground(ground.clone())),
            Concept::Hole(_) => out.push(Token::Star),
            Concept::Compound { head, args } => {
                out.push(Token::Apply(args.len()));
                // Pushed in reverse so they pop in order, head first.
                for arg in args.iter().rev() {
                    stack.push(arg);
                }
                stack.push(head);
            }
        }
    }
    out
}

/// For each token position, the position just past the subterm starting there.
///
/// A stored hole swallows a whole goal subterm, so the walk needs to know
/// where that subterm ends without re-parsing. Computed right to left: a leaf
/// token spans one position, and an `Apply(k)` spans its head plus `k`
/// arguments, each of whose extents is already known.
fn subterm_ends(tokens: &[Token]) -> Vec<usize> {
    let n = tokens.len();
    let mut ends = vec![0usize; n];
    for i in (0..n).rev() {
        match &tokens[i] {
            Token::Apply(arity) => {
                let mut cursor = i + 1;
                // Head plus arguments.
                for _ in 0..arity.saturating_add(1) {
                    // Well-formed flattenings never run off the end. The guard
                    // is here so a hand-built token sequence cannot panic.
                    if cursor >= n {
                        cursor = n;
                        break;
                    }
                    cursor = ends[cursor];
                }
                ends[i] = cursor.min(n);
            }
            _ => ends[i] = i + 1,
        }
    }
    ends
}

/// One trie node. Rules live at the node their conclusion's flattening ends
/// on, held as indices into the tree's rule vector so a rule reachable by
/// several wildcard paths is cheap to deduplicate.
#[derive(Debug, Default)]
struct Node {
    edges: HashMap<Token, Node>,
    rules: Vec<u32>,
}

/// What ordering a rule gets in the results.
///
/// Derived entirely from rule content, never from insertion order, because two
/// brains that learned the same rules in different orders have to try them in
/// the same order or their derivations stop being reproducible. Rules that
/// agree on all five components are interchangeable for derivation: same
/// conclusion, same antecedent, same condition, same target.
type RuleKey = (Arc<str>, ContentId, ContentId, Option<ContentId>, ContentId);

fn rule_key(rule: &StoredRule) -> RuleKey {
    (
        rule.name.clone(),
        rule.produce.content_id(),
        rule.pattern.content_id(),
        rule.condition.as_ref().map(|c| c.content_id()),
        rule.target.content_id(),
    )
}

/// How much work a retrieval actually did.
///
/// The point of the index is that it does not look at most of the rules. That
/// claim is only worth making if it can be measured, and a wall-clock number
/// at this size is noise, so the tree counts instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetrievalStats {
    /// Trie nodes popped off the search worklist, including those visited
    /// while counting past a stored subterm for a wildcard goal.
    pub nodes_visited: usize,
    /// Rule slots touched at terminal nodes, counted before deduplication.
    /// This is the number of rules the tree so much as looked at.
    pub rules_examined: usize,
}

/// Rule lookup by a trie over flattened conclusions.
///
/// Drop-in for [`ScanIndex`](crate::ScanIndex): same three methods, same
/// over-approximating contract, same stable ordering. It differs in what it
/// discriminates on. `ScanIndex` compares head symbol and arity, so a brain
/// holding two thousand rules about `HttpStatusMeaning` offers all two
/// thousand for any goal with that head. This tree discriminates all the way
/// down the conclusion, so a goal about 404 sees only the rules that mention
/// 404 or hold a hole in that position.
#[derive(Debug, Default)]
pub struct DiscriminationTree {
    rules: Vec<Arc<StoredRule>>,
    keys: Vec<RuleKey>,
    root: Node,
}

impl DiscriminationTree {
    pub fn new(rules: Vec<Arc<StoredRule>>) -> Self {
        let mut tree = DiscriminationTree::default();
        for rule in rules {
            tree.insert(rule);
        }
        tree
    }

    /// Build from every backward-usable rule in the store.
    pub fn from_store(store: &Store) -> Result<Self> {
        Ok(DiscriminationTree::new(load_rules(store)?))
    }

    /// Add a rule after construction.
    ///
    /// A rule learned mid-session has to be usable on the next derivation step
    /// without rebuilding the index, or learning would mean a stall
    /// proportional to everything already known.
    pub fn insert(&mut self, rule: Arc<StoredRule>) {
        let index = self.rules.len();
        // More rules than a u32 can index is not a brain, it is a bug
        // elsewhere; refuse rather than truncate the index and corrupt lookup.
        let Ok(slot) = u32::try_from(index) else {
            return;
        };
        let tokens = flatten(&rule.produce);
        let mut node = &mut self.root;
        for token in tokens {
            node = node.edges.entry(token).or_default();
        }
        node.rules.push(slot);
        self.keys.push(rule_key(&rule));
        self.rules.push(rule);
    }

    /// Candidates plus a count of the work it took to find them.
    ///
    /// Same answer as [`RuleIndex::candidates`]; the stats are what make the
    /// "does not touch most of the rules" claim testable.
    pub fn candidates_with_stats(&self, goal: &Concept) -> (Vec<Arc<StoredRule>>, RetrievalStats) {
        let mut stats = RetrievalStats::default();
        let hits = self.search(goal, &mut stats);
        (self.materialize(hits), stats)
    }

    /// Walk the trie against the goal's flattening, collecting rule slots.
    fn search(&self, goal: &Concept, stats: &mut RetrievalStats) -> Vec<u32> {
        let tokens = flatten(goal);
        let ends = subterm_ends(&tokens);

        let mut hits: Vec<u32> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        // Explicit worklist rather than recursion: the branching is unbounded
        // in the depth of the goal, and a deep goal must not overflow the
        // stack.
        let mut work: Vec<(&Node, usize)> = vec![(&self.root, 0)];
        // A goal holding several holes can reach the same node at the same
        // goal position along different branches. Without this the search
        // redoes the identical subsearch each time, which is exponential in
        // the number of holes for no extra answers.
        let mut visited: HashSet<(usize, usize)> = HashSet::new();

        while let Some((node, pos)) = work.pop() {
            if !visited.insert((node as *const Node as usize, pos)) {
                continue;
            }
            stats.nodes_visited += 1;
            let Some(token) = tokens.get(pos) else {
                // The goal is fully consumed, so this node terminates a stored
                // conclusion of exactly the same extent. Anything filed here
                // is a candidate.
                for &slot in &node.rules {
                    stats.rules_examined += 1;
                    if seen.insert(slot) {
                        hits.push(slot);
                    }
                }
                continue;
            };

            if matches!(token, Token::Star) {
                // Hole in the goal. It can be filled by any stored subterm, so
                // follow every edge just far enough to consume one complete
                // subterm and pick the goal up at the next position.
                skip(node, 1, pos + 1, &mut work, stats);
                continue;
            }

            // Exact agreement on this symbol. For `Apply(k)` this also settles
            // the arity, so mismatched arities are never explored.
            if let Some(child) = node.edges.get(token) {
                work.push((child, pos + 1));
            }
            // Hole in the conclusion. It stands for this entire goal subterm,
            // not just the token under the cursor, so the goal resumes after
            // the subterm ends.
            if let Some(child) = node.edges.get(&Token::Star) {
                let resume = ends.get(pos).copied().unwrap_or(pos + 1);
                work.push((child, resume));
            }
        }

        hits
    }

    /// Turn rule slots into rules, in the canonical order.
    fn materialize(&self, mut hits: Vec<u32>) -> Vec<Arc<StoredRule>> {
        hits.sort_by(|a, b| {
            let (ai, bi) = (*a as usize, *b as usize);
            match (self.keys.get(ai), self.keys.get(bi)) {
                (Some(ak), Some(bk)) => ak.cmp(bk).then(ai.cmp(&bi)),
                _ => ai.cmp(&bi),
            }
        });
        hits.iter()
            .filter_map(|slot| self.rules.get(*slot as usize).cloned())
            .collect()
    }
}

/// Follow every edge out of `node` until exactly `owed` complete subterms have
/// been consumed, then queue what is left with the goal resuming at `resume`.
///
/// This is the wildcard-in-the-goal case, and it is the reason `Apply` carries
/// an arity. Consuming a leaf token settles one subterm. Consuming `Apply(k)`
/// settles the compound itself but opens `k + 1` more, so the debt moves by
/// `k` rather than down by one.
fn skip<'a>(
    node: &'a Node,
    owed: usize,
    resume: usize,
    out: &mut Vec<(&'a Node, usize)>,
    stats: &mut RetrievalStats,
) {
    let mut stack: Vec<(&Node, usize)> = vec![(node, owed)];
    while let Some((node, owed)) = stack.pop() {
        stats.nodes_visited += 1;
        if owed == 0 {
            out.push((node, resume));
            continue;
        }
        for (token, child) in node.edges.iter() {
            let next = match token {
                Token::Apply(arity) => owed.saturating_add(*arity),
                _ => owed - 1,
            };
            stack.push((child, next));
        }
    }
}

impl RuleIndex for DiscriminationTree {
    fn candidates(&self, goal: &Concept) -> Vec<Arc<StoredRule>> {
        let mut stats = RetrievalStats::default();
        let hits = self.search(goal, &mut stats);
        self.materialize(hits)
    }

    fn all(&self) -> Vec<Arc<StoredRule>> {
        let hits: Vec<u32> = (0..self.rules.len())
            .filter_map(|i| u32::try_from(i).ok())
            .collect();
        self.materialize(hits)
    }

    fn len(&self) -> usize {
        self.rules.len()
    }
}
