//! The Spoon concept substrate.
//!
//! Everything in Spoon is a [`Concept`]. Concepts come in three shapes:
//! atomic (indivisible, has identity), compound (one concept applied to
//! others), and hole (a gap in a pattern). There is no separate value type,
//! no separate expression type, and no separate IR. Evaluation is term
//! rewriting over this one representation.
//!
//! ```
//! use spoon_concept::{Concept, Ground};
//!
//! // Greg is an atomic concept identified by a name.
//! let greg = Concept::named("Greg");
//!
//! // 42 is an atomic concept identified by its value. Self-describing, so it
//! // needs no store row until something is asserted about it.
//! let answer = Concept::int(42);
//!
//! // FriendWith<Greg, Keal> is a compound: a full citizen, storable, and able
//! // to carry its own relationships.
//! let friendship = Concept::call("friend-with", [greg, Concept::named("Keal")]);
//! assert_eq!(friendship.arity(), 2);
//! assert!(answer.is_ground());
//! ```

mod concept;
mod id;
mod meta;
mod ops;

pub use concept::{Concept, ContentId};
pub use id::{ConceptId, Ground, GroundKind, HoleId, JsonBlob, SymbolId, SymbolTable};
pub use meta::{
    Activation, ConceptMeta, Effect, NativeId, Provenance, Realization, RealizationKind,
    RealizationSpec, Tier,
};
pub use ops::{
    Bindings, Path, PathStep, PostOrder, PreOrder, alpha_equivalent, anti_unify, at_path,
    find_first, flatten_spine, generalizes, hole_count, holes, is_ground_term, map_args, max_hole,
    positions, post_order, pre_order, rename_holes, replace_at, subterms, substitute,
    substitute_positional, try_map_args, walk,
};
