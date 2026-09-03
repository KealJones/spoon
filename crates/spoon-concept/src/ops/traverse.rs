//! Visiting a term, naming a spot inside it, and rebuilding at that spot.
//!
//! Rewriting needs three things that the bare `Concept` enum does not give
//! you: an order to visit subterms in, a way to say *where* inside a term
//! something is, and a way to produce a new term that differs at exactly that
//! spot. That is this file.
//!
//! Visit order is fixed everywhere: the node itself, then its head, then its
//! arguments left to right. The head comes first because it is what decides
//! the meaning of the compound, so anything scanning for a dispatch target
//! finds it before wading through arguments.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::concept::{Concept, ContentId};

/// One hop from a compound down to a child.
///
/// The head is a separate step rather than `Arg(0)` because a compound's head
/// is not one of its arguments: `((f a) b)` has a compound head, and treating
/// it as an argument would make paths ambiguous about arity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PathStep {
    Head,
    Arg(usize),
}

impl std::fmt::Display for PathStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathStep::Head => f.write_str("head"),
            PathStep::Arg(i) => write!(f, "{i}"),
        }
    }
}

/// The address of a subterm, as the sequence of hops that reaches it.
///
/// A path is stable against cloning and cheap to compare, which is what makes
/// it the right currency for "rewrite here" between the matcher that found a
/// redex and the evaluator that replaces it. Holding a `&Concept` instead
/// would borrow the term you are about to rebuild.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Path(Vec<PathStep>);

impl Path {
    /// The empty path: the whole term itself.
    pub fn root() -> Self {
        Path(Vec::new())
    }

    /// Descend one more step. Used while walking, so the path under
    /// construction is reused instead of reallocated at every node.
    pub fn push(&mut self, step: PathStep) {
        self.0.push(step);
    }

    /// Undo the last [`push`](Path::push).
    pub fn pop(&mut self) -> Option<PathStep> {
        self.0.pop()
    }

    /// A new path one step deeper, leaving this one alone.
    pub fn child(&self, step: PathStep) -> Path {
        let mut next = self.clone();
        next.push(step);
        next
    }

    pub fn steps(&self) -> &[PathStep] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// True when this path addresses the whole term.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<PathStep> for Path {
    fn from_iter<I: IntoIterator<Item = PathStep>>(iter: I) -> Self {
        Path(iter.into_iter().collect())
    }
}

impl From<Vec<PathStep>> for Path {
    fn from(steps: Vec<PathStep>) -> Self {
        Path(steps)
    }
}

/// Renders as `.` for the root and `head/1/0` otherwise, so paths in logs and
/// error messages are readable without a decoder.
impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            return f.write_str(".");
        }
        for (i, step) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("/")?;
            }
            write!(f, "{step}")?;
        }
        Ok(())
    }
}

/// Node, then head, then arguments left to right. See [`pre_order`].
pub struct PreOrder<'a> {
    stack: Vec<&'a Concept>,
}

impl<'a> Iterator for PreOrder<'a> {
    type Item = &'a Concept;

    fn next(&mut self) -> Option<&'a Concept> {
        let node = self.stack.pop()?;
        if let Concept::Compound { head, args } = node {
            // Pushed in reverse so they pop in source order, head first.
            for arg in args.iter().rev() {
                self.stack.push(arg);
            }
            self.stack.push(head);
        }
        Some(node)
    }
}

/// Every subterm, parents before children.
///
/// This is the default order for anything that wants to stop early at the
/// shallowest match: rule matching, dispatch-target scanning, "does this term
/// mention X". Lazy, so `find` on it does not build the whole node list.
pub fn pre_order(term: &Concept) -> PreOrder<'_> {
    PreOrder { stack: vec![term] }
}

enum Step<'a> {
    Descend(&'a Concept),
    Emit(&'a Concept),
}

/// Children before parents. See [`post_order`].
pub struct PostOrder<'a> {
    stack: Vec<Step<'a>>,
}

impl<'a> Iterator for PostOrder<'a> {
    type Item = &'a Concept;

    fn next(&mut self) -> Option<&'a Concept> {
        loop {
            match self.stack.pop()? {
                Step::Emit(node) => return Some(node),
                Step::Descend(node) => {
                    self.stack.push(Step::Emit(node));
                    if let Concept::Compound { head, args } = node {
                        for arg in args.iter().rev() {
                            self.stack.push(Step::Descend(arg));
                        }
                        self.stack.push(Step::Descend(head));
                    }
                }
            }
        }
    }
}

/// Every subterm, children before parents.
///
/// The order innermost-first evaluation needs: by the time a compound is
/// visited, everything it is built from has already been seen. Also the right
/// order for bottom-up cost or size accumulation.
pub fn post_order(term: &Concept) -> PostOrder<'_> {
    PostOrder {
        stack: vec![Step::Descend(term)],
    }
}

/// Distinct subterms, in pre-order, with structural duplicates dropped.
///
/// Consolidation asks "what shapes does this term contain", not "how many
/// times". Repeats are removed by [`ContentId`], so `Add<X, X>` contributes
/// one `X`, not two.
pub fn subterms(term: &Concept) -> Vec<&Concept> {
    let mut seen: BTreeSet<ContentId> = BTreeSet::new();
    pre_order(term)
        .filter(|node| seen.insert(node.content_id()))
        .collect()
}

/// Pre-order walk with depth, where the visitor decides whether to descend.
///
/// The iterators cannot be pruned mid-flight. This can: returning `false` from
/// `visit` skips the node's children, which is how a matcher avoids descending
/// into a subterm it has already claimed, and how depth-limited scans stay
/// cheap on deep terms.
pub fn walk<F>(term: &Concept, visit: &mut F)
where
    F: FnMut(&Concept, usize) -> bool,
{
    walk_at(term, 0, visit);
}

fn walk_at<F>(term: &Concept, depth: usize, visit: &mut F)
where
    F: FnMut(&Concept, usize) -> bool,
{
    if !visit(term, depth) {
        return;
    }
    if let Concept::Compound { head, args } = term {
        walk_at(head, depth + 1, visit);
        for arg in args.iter() {
            walk_at(arg, depth + 1, visit);
        }
    }
}

/// The shallowest, leftmost subterm satisfying `pred`.
///
/// Named rather than left to `pre_order(t).find(p)` at every call site because
/// "leftmost outermost" is a rewriting strategy, not an incidental detail, and
/// callers should be reading it as such.
pub fn find_first<F>(term: &Concept, mut pred: F) -> Option<&Concept>
where
    F: FnMut(&Concept) -> bool,
{
    pre_order(term).find(|node| pred(node))
}

/// Every position in the term, paired with the subterm living there.
///
/// Ordered identically to [`pre_order`], so the two can be zipped. This is
/// what a rewriter enumerates when it wants to try a rule at every site rather
/// than only at the root.
pub fn positions(term: &Concept) -> Vec<(Path, &Concept)> {
    let mut out = Vec::new();
    let mut cursor = Path::root();
    collect_positions(term, &mut cursor, &mut out);
    out
}

fn collect_positions<'a>(term: &'a Concept, cursor: &mut Path, out: &mut Vec<(Path, &'a Concept)>) {
    out.push((cursor.clone(), term));
    if let Concept::Compound { head, args } = term {
        cursor.push(PathStep::Head);
        collect_positions(head, cursor, out);
        cursor.pop();
        for (i, arg) in args.iter().enumerate() {
            cursor.push(PathStep::Arg(i));
            collect_positions(arg, cursor, out);
            cursor.pop();
        }
    }
}

/// The subterm at `path`, or `None` when the path does not fit the term.
///
/// Paths outlive the terms they were computed against (a stored rewrite site,
/// a path sent across a boundary), so a stale path has to be a `None` rather
/// than a panic.
pub fn at_path<'a>(term: &'a Concept, path: &Path) -> Option<&'a Concept> {
    let mut node = term;
    for step in path.steps() {
        node = match (step, node) {
            (PathStep::Head, Concept::Compound { head, .. }) => head,
            (PathStep::Arg(i), Concept::Compound { args, .. }) => args.get(*i)?,
            _ => return None,
        };
    }
    Some(node)
}

/// A copy of `term` with the subterm at `path` swapped for `replacement`.
///
/// Returns `None` for a path that does not fit, matching [`at_path`]. Only the
/// compounds along the path are rebuilt; every sibling subtree keeps its
/// existing `Arc`, so replacing a leaf of a large term costs the depth of the
/// path, not the size of the term.
pub fn replace_at(term: &Concept, path: &Path, replacement: Concept) -> Option<Concept> {
    replace_steps(term, path.steps(), replacement)
}

fn replace_steps(term: &Concept, steps: &[PathStep], replacement: Concept) -> Option<Concept> {
    let Some((step, rest)) = steps.split_first() else {
        return Some(replacement);
    };
    let Concept::Compound { head, args } = term else {
        return None;
    };
    match step {
        PathStep::Head => {
            let new_head = replace_steps(head, rest, replacement)?;
            Some(Concept::Compound {
                head: Arc::new(new_head),
                args: args.clone(),
            })
        }
        PathStep::Arg(i) => {
            let old = args.get(*i)?;
            let new_arg = replace_steps(old, rest, replacement)?;
            let mut rebuilt = args.to_vec();
            rebuilt[*i] = new_arg;
            Some(Concept::Compound {
                head: head.clone(),
                args: Arc::from(rebuilt),
            })
        }
    }
}
