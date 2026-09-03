//! What one turn carries between its synchronous half and its async half,
//! plus the two small readings of a turn that both halves want.

use spoon_core::types::*;

use crate::discourse::Grounded;
use crate::dispatch::Dispatched;

use super::lookup;

/// What the synchronous part of a turn produced.
pub(super) struct SyncTurnResult {
    pub sce: String,
    pub clauses: Vec<Clause>,
    pub unknown_words: Vec<String>,
    pub ears_path: EarsPath,
    pub plan: ResponsePlan,
    pub trace: Vec<String>,
    pub flags: TurnFlags,
    /// Set when an unknown verb should be taken to the teacher.
    pub teacher: Option<TeacherRequest>,
    /// Set when the offline and network lookup sources found nothing and the
    /// Teacher seat is the last thing left to try.
    pub lookup: Option<lookup::Topic>,
    pub llm_before: (u32, u32, u32, u32),
}

/// What happened this turn, for `TurnMetrics`. Filled by the brain submodules.
#[derive(Default)]
pub(crate) struct TurnFlags {
    pub synthesis_attempted: bool,
    pub synthesis_succeeded: bool,
    pub teacher_fallback: bool,
    /// A plan ran an action with a non-Kernel tier.
    pub used_learned_action: bool,
    pub plan_steps: usize,
}

pub(super) struct TeacherRequest {
    pub verb: String,
    pub sce: String,
    pub signals: Vec<Signal>,
}

impl SyncTurnResult {
    pub(super) fn new(llm_before: (u32, u32, u32, u32)) -> Self {
        SyncTurnResult {
            sce: String::new(),
            clauses: vec![],
            unknown_words: vec![],
            ears_path: EarsPath::Direct,
            plan: ResponsePlan::new(vec![]),
            trace: vec![],
            flags: TurnFlags::default(),
            teacher: None,
            lookup: None,
            llm_before,
        }
    }

    pub(super) fn reply(mut self, plan: ResponsePlan) -> Self {
        self.plan = plan;
        self
    }
}

/// The entity an assertion was about, so "User means Mary." can retarget it:
/// the subject if it is a proper name, else any proper name in the clause.
pub(super) fn assert_target(gs: &[Grounded]) -> Option<String> {
    gs.iter().filter(|g| matches!(g.clause.act, Act::Assert)).find_map(|g| {
        let named = |var: &str| {
            g.clause.referents.iter().find(|r| r.var == var).and_then(|r| match &r.quant {
                Quant::Named(n) if n != "User" && n != "Assistant" && n != "It" => Some(n.clone()),
                _ => None,
            })
        };
        let subject = match g.clause.conditions.first().and_then(|p| p.args.first()) {
            Some(Term::Var { var }) => named(var),
            _ => None,
        };
        subject.or_else(|| g.clause.referents.iter().find_map(|r| named(&r.var)))
    })
}

pub(super) fn dispatch_variant_name(d: &Dispatched) -> &'static str {
    match d {
        Dispatched::Moves(_) => "Moves",
        Dispatched::Plan { .. } => "Plan",
        Dispatched::UnknownCapability { .. } => "UnknownCapability",
        Dispatched::NeedsTeacher { .. } => "NeedsTeacher",
    }
}
