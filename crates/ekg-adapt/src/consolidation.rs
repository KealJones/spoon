use ekg_core::{Episode, EpisodeId, VerifiabilityTier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A conservative skill candidate extracted from immutable episode evidence.
/// Candidates are reports only; promotion remains the separate replay gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCandidate {
    pub name: String,
    pub source_episode_ids: Vec<EpisodeId>,
    pub support_count: u32,
    pub rationale: String,
    pub failure_critic: bool,
}

pub fn discover_skills(episodes: &[Episode]) -> Vec<SkillCandidate> {
    let mut groups: BTreeMap<String, Vec<&Episode>> = BTreeMap::new();
    for episode in episodes {
        let Some(action) = episode.action.as_deref() else {
            continue;
        };
        if episode.succeeded()
            && episode.evaluation.as_ref().is_some_and(|evaluation| {
                matches!(
                    evaluation.tier,
                    VerifiabilityTier::Hard | VerifiabilityTier::Consensus
                )
            })
        {
            groups.entry(action.to_owned()).or_default().push(episode);
        }
    }
    groups
        .into_iter()
        .filter(|(_, episodes)| episodes.len() >= 2)
        .map(|(action, episodes)| SkillCandidate {
            name: format!("repeated:{}", action),
            source_episode_ids: episodes.iter().map(|episode| episode.id).collect(),
            support_count: episodes.len() as u32,
            rationale: "same version-pinned procedure succeeded on multiple verified episodes"
                .into(),
            failure_critic: false,
        })
        .collect()
}

pub fn discover_single_success(episode: &Episode) -> Option<SkillCandidate> {
    if !episode.succeeded()
        || !episode.evaluation.as_ref().is_some_and(|evaluation| {
            matches!(
                evaluation.tier,
                VerifiabilityTier::Hard | VerifiabilityTier::Consensus
            )
        })
    {
        return None;
    }
    Some(SkillCandidate {
        name: format!("explanation:{}", episode.id),
        source_episode_ids: vec![episode.id],
        support_count: 1,
        rationale: "single verified success retained for explanation-guided generalization".into(),
        failure_critic: false,
    })
}

pub fn discover_failure_critic(episode: &Episode) -> Option<SkillCandidate> {
    if !episode.failed() {
        return None;
    }
    Some(SkillCandidate {
        name: format!("critic:{}", episode.id),
        source_episode_ids: vec![episode.id],
        support_count: 1,
        rationale: episode
            .evaluation
            .as_ref()
            .map(|evaluation| evaluation.details.clone())
            .unwrap_or_else(|| "failed episode lacks an evaluation detail".into()),
        failure_critic: true,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeCompressionPlan {
    pub retain_full: Vec<EpisodeId>,
    pub summarize: Vec<EpisodeId>,
    pub forgotten_as_known_gap: Vec<EpisodeId>,
}

/// Plans bounded compression while preserving every failure and the boundary
/// examples of a repeated family. It never deletes or mutates episode rows.
pub fn plan_episode_compression(episodes: &[Episode]) -> EpisodeCompressionPlan {
    let mut groups: BTreeMap<String, Vec<&Episode>> = BTreeMap::new();
    for episode in episodes {
        groups
            .entry(
                episode
                    .action
                    .clone()
                    .unwrap_or_else(|| format!("situation:{}", episode.situation)),
            )
            .or_default()
            .push(episode);
    }
    let mut retain_full = Vec::new();
    let mut summarize = Vec::new();
    for group in groups.values() {
        for (index, episode) in group.iter().enumerate() {
            let keep =
                group.len() <= 2 || index == 0 || index + 1 == group.len() || episode.failed();
            if keep {
                retain_full.push(episode.id);
            } else {
                summarize.push(episode.id);
            }
        }
    }
    retain_full.sort_unstable_by_key(|id| id.to_string());
    summarize.sort_unstable_by_key(|id| id.to_string());
    EpisodeCompressionPlan {
        retain_full,
        summarize,
        forgotten_as_known_gap: Vec::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetirementRecord {
    pub retired_skill: String,
    pub successor_skill: String,
    pub reason: String,
    pub reconstructible: bool,
}

pub fn retire_skill(
    retired_skill: impl Into<String>,
    successor_skill: impl Into<String>,
    reason: impl Into<String>,
) -> RetirementRecord {
    RetirementRecord {
        retired_skill: retired_skill.into(),
        successor_skill: successor_skill.into(),
        reason: reason.into(),
        reconstructible: true,
    }
}

#[cfg(test)]
mod tests {
    use ekg_core::{Episode, EscalationRung, Evaluation, VerifiabilityTier};

    use super::{discover_failure_critic, discover_skills, plan_episode_compression, retire_skill};

    fn episode(action: &str, success: bool, created_at: i64) -> Episode {
        let mut value = Episode::new("task");
        value.action = Some(action.into());
        value.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success,
            details: "checked".into(),
            surprise: None,
        });
        value.cost.rung_reached = EscalationRung::Run;
        value.created_at = created_at;
        value
    }

    #[test]
    fn repeated_successes_discover_and_compression_preserves_boundaries_and_failures() {
        let episodes = vec![
            episode("procedure:p@1", true, 1),
            episode("procedure:p@1", true, 2),
            episode("procedure:p@1", false, 3),
            episode("procedure:p@1", true, 4),
        ];
        assert_eq!(discover_skills(&episodes)[0].support_count, 3);
        let plan = plan_episode_compression(&episodes);
        assert_eq!(plan.summarize.len(), 1);
        assert!(plan.retain_full.contains(&episodes[2].id));
        assert!(
            discover_failure_critic(&episodes[2])
                .unwrap()
                .failure_critic
        );
        assert!(retire_skill("old", "new", "replaced").reconstructible);
    }
}
