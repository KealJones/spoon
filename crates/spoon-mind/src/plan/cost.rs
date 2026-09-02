//! Cost model for plan nodes.
//!
//! Lower cost = preferred by the planner. The formula rewards frequently-used
//! actions (high ACT-R activation) and penalises unproven (Provisional) ones.
//! Placeholders carry a large fixed penalty so the planner always prefers a
//! real producer, but plans still exist when one is genuinely missing.

use spoon_core::{Action, Can, Tier};

/// Fixed cost charged for each missing required input (becomes a user prompt).
pub const PLACEHOLDER_COST: f64 = 10.0;

/// Extra cost per Map node (applying an action element-wise over a list).
pub const MAP_PENALTY: f64 = 0.05;

/// Cost to include `action` in a plan.
///
/// Formula: 1.0 - 0.2 * clamp(activation, -2, 2) / 2 + 0.3 if Provisional.
/// Kernel actions without history have activation clamped to -2, giving cost
/// ~1.2 in a fresh CAN. Consolidated actions with usage history approach 0.8.
pub fn action_cost(can: &Can, action: &Action) -> f64 {
    let act = can.activation(&action.id).clamp(-2.0, 2.0);
    let base = 1.0 - 0.2 * act / 2.0;
    if action.tier == Tier::Provisional { base + 0.3 } else { base }
}
