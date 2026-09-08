//! Link user rejection to the episode, reading and assertions it rejects.

use super::*;

/// A rejection without a new task asks us to revisit the last answer.
/// Explanations after "that's wrong" belong in the Teacher's feedback context.
pub(super) fn retries_request(text: &str) -> bool {
    let text = text.trim().to_lowercase();
    ["that's wrong", "thats wrong", "that is wrong", "wrong", "nope", "not what i said", "not that"]
        .iter().any(|marker| text == *marker || text.strip_prefix(marker).is_some_and(|rest| {
            rest.starts_with(|c: char| c.is_whitespace() || c == ',' || c == '.')
        }))
        || crate::correct::split_repair(&text).is_some_and(|r| r.before.is_empty() && r.after.is_empty())
}

impl Brain {
    pub(super) fn feedback_target(&self, session: &str, text: &str) -> spoon_store::Result<Option<Episode>> {
        if !crate::correct::is_correction(text) { return Ok(None); }
        if !retries_request(text)
            && crate::correct::split_repair(text).is_some_and(|r| !r.before.is_empty()) {
            return Ok(None);
        }
        let raw = self.store.recent_episodes(32, Some(session))?;
        for json in raw {
            let e: Episode = serde_json::from_str(&json)?;
            if e.correction.is_none() && !e.steps.is_empty() && !only_pleasantries(&e.steps)
                && (e.result.is_some() || !e.gaps.is_empty()) {
                return Ok(Some(e));
            }
        }
        Ok(None)
    }

    pub(super) fn reject_episode(&mut self, previous: &Episode, text: &str) -> spoon_store::Result<()> {
        crate::correct::apply(&self.store, previous, Utc::now())?;
        // The feedback is evidence even when no realization or assertion exists.
        self.store.mark_episode_corrected(previous.id, text)?;
        self.phrasing = PhrasingIndex::from_store(&self.store)?;
        Ok(())
    }

    pub(super) fn remember_pair(&mut self, text: &str, steps: &[Concept], source: PairSource) -> spoon_store::Result<i64> {
        for step in steps {
            self.remember_names(step);
        }
        let pair = self.store.put_pair(text, steps, source)?;
        // Keep source, identity and evidence exactly aligned with restart behavior.
        self.phrasing = PhrasingIndex::from_store(&self.store)?;
        Ok(pair)
    }
}
