use ekg_core::{EkgError, Episode, EpisodeId, EscalationRung, concept::ConceptId};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeQuery {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub outcome: Option<bool>,
    pub rung: Option<EscalationRung>,
    pub concept: Option<ConceptId>,
    pub limit: u32,
}

impl Default for EpisodeQuery {
    fn default() -> Self {
        Self {
            since: None,
            until: None,
            outcome: None,
            rung: None,
            concept: None,
            limit: 100,
        }
    }
}

pub struct EpisodeStore {
    conn: Connection,
}

impl EpisodeStore {
    pub fn new(path: &str) -> Result<Self, EkgError> {
        let conn = Connection::open(path).map_err(|e| EkgError::Storage(e.to_string()))?;
        let store = Self { conn };
        store.create_schema()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, EkgError> {
        let conn = Connection::open_in_memory().map_err(|e| EkgError::Storage(e.to_string()))?;
        let store = Self { conn };
        store.create_schema()?;
        Ok(store)
    }

    fn create_schema(&self) -> Result<(), EkgError> {
        self.conn
            .execute_batch(
                "PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS episodes (
                    id TEXT PRIMARY KEY,
                    situation TEXT NOT NULL,
                    data_json TEXT NOT NULL,
                    success INTEGER,
                    rung_reached TEXT,
                    created_at INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_episodes_created
                    ON episodes(created_at);

                CREATE INDEX IF NOT EXISTS idx_episodes_success
                    ON episodes(success);

                CREATE INDEX IF NOT EXISTS idx_episodes_rung
                    ON episodes(rung_reached);

                CREATE TABLE IF NOT EXISTS episode_concepts (
                    episode_id TEXT NOT NULL,
                    concept_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    PRIMARY KEY (episode_id, concept_id, role),
                    FOREIGN KEY (episode_id) REFERENCES episodes(id)
                );

                CREATE INDEX IF NOT EXISTS idx_episode_concepts_concept
                    ON episode_concepts(concept_id);",
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;
        Ok(())
    }

    pub fn insert(&self, episode: &Episode) -> Result<(), EkgError> {
        let data_json =
            serde_json::to_string(episode).map_err(|e| EkgError::Serialization(e.to_string()))?;

        let success = episode.evaluation.as_ref().map(|e| e.success as i32);
        let rung = serde_json::to_value(episode.cost.rung_reached)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        let transaction = self
            .conn
            .unchecked_transaction()
            .map_err(|e| EkgError::Storage(e.to_string()))?;
        transaction
            .execute(
                "INSERT INTO episodes (id, situation, data_json, success, rung_reached, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    episode.id.to_string(),
                    episode.situation,
                    data_json,
                    success,
                    rung,
                    episode.created_at,
                ],
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;

        Self::insert_concept_index(&transaction, episode)?;
        transaction
            .commit()
            .map_err(|e| EkgError::Storage(e.to_string()))
    }

    fn insert_concept_index(conn: &Connection, episode: &Episode) -> Result<(), EkgError> {
        for interp in &episode.interpretations {
            conn.execute(
                "INSERT OR IGNORE INTO episode_concepts (episode_id, concept_id, role)
                     VALUES (?1, ?2, 'interpretation')",
                params![episode.id.to_string(), interp.meaning.to_string()],
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;
        }

        for entity in &episode.context.entities {
            conn.execute(
                "INSERT OR IGNORE INTO episode_concepts (episode_id, concept_id, role)
                     VALUES (?1, ?2, 'context')",
                params![episode.id.to_string(), entity.to_string()],
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;
        }

        for candidate in &episode.knowledge_considered {
            conn.execute(
                "INSERT OR IGNORE INTO episode_concepts (episode_id, concept_id, role)
                     VALUES (?1, ?2, 'considered')",
                params![episode.id.to_string(), candidate.concept.to_string()],
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;
        }

        Ok(())
    }

    pub fn get(&self, id: EpisodeId) -> Result<Episode, EkgError> {
        let json: String = self
            .conn
            .query_row(
                "SELECT data_json FROM episodes WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => EkgError::NotFound(format!("episode {id}")),
                _ => EkgError::Storage(e.to_string()),
            })?;

        serde_json::from_str(&json).map_err(|e| EkgError::Serialization(e.to_string()))
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<Episode>, EkgError> {
        let mut stmt = self
            .conn
            .prepare("SELECT data_json FROM episodes ORDER BY created_at DESC LIMIT ?1")
            .map_err(|e| EkgError::Storage(e.to_string()))?;

        let episodes = stmt
            .query_map(params![limit], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| EkgError::Storage(e.to_string()))?
            .map(|row| {
                let json = row.map_err(|e| EkgError::Storage(e.to_string()))?;
                serde_json::from_str(&json).map_err(|e| EkgError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(episodes)
    }

    pub fn list_failures(&self, limit: u32) -> Result<Vec<Episode>, EkgError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT data_json FROM episodes WHERE success = 0
                 ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;

        let episodes = stmt
            .query_map(params![limit], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| EkgError::Storage(e.to_string()))?
            .map(|row| {
                let json = row.map_err(|e| EkgError::Storage(e.to_string()))?;
                serde_json::from_str(&json).map_err(|e| EkgError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(episodes)
    }

    /// Find episodes involving a specific concept (in any role).
    pub fn find_by_concept(&self, concept_id: ConceptId) -> Result<Vec<Episode>, EkgError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT e.data_json FROM episodes e
                 INNER JOIN episode_concepts ec ON e.id = ec.episode_id
                 WHERE ec.concept_id = ?1
                 ORDER BY e.created_at DESC",
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;

        let episodes = stmt
            .query_map(params![concept_id.to_string()], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| EkgError::Storage(e.to_string()))?
            .map(|row| {
                let json = row.map_err(|e| EkgError::Storage(e.to_string()))?;
                serde_json::from_str(&json).map_err(|e| EkgError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(episodes)
    }

    /// Count episodes by escalation rung. Used for section 38 metric 5
    /// (rung distribution drift).
    pub fn rung_distribution(&self) -> Result<Vec<(String, u32)>, EkgError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT rung_reached, COUNT(*) FROM episodes
                 WHERE rung_reached IS NOT NULL
                 GROUP BY rung_reached
                 ORDER BY COUNT(*) DESC",
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;

        let dist = stmt
            .query_map([], |row| {
                let rung: String = row.get(0)?;
                let count: u32 = row.get(1)?;
                Ok((rung, count))
            })
            .map_err(|e| EkgError::Storage(e.to_string()))?
            .map(|row| row.map_err(|e| EkgError::Storage(e.to_string())))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(dist)
    }

    pub fn count(&self) -> Result<u64, EkgError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM episodes", [], |row| row.get(0))
            .map_err(|e| EkgError::Storage(e.to_string()))
    }

    /// Query episodes using composable indexed filters.
    pub fn query(&self, query: &EpisodeQuery) -> Result<Vec<Episode>, EkgError> {
        let rung = query.rung.and_then(|value| {
            serde_json::to_value(value)
                .ok()
                .and_then(|json| json.as_str().map(str::to_owned))
        });
        let concept = query.concept.map(|value| value.to_string());
        let outcome = query.outcome.map(i32::from);

        let mut stmt = self
            .conn
            .prepare(
                "SELECT e.data_json FROM episodes e
                 WHERE (?1 IS NULL OR e.created_at >= ?1)
                   AND (?2 IS NULL OR e.created_at <= ?2)
                   AND (?3 IS NULL OR e.success = ?3)
                   AND (?4 IS NULL OR e.rung_reached = ?4)
                   AND (?5 IS NULL OR EXISTS (
                       SELECT 1 FROM episode_concepts ec
                       WHERE ec.episode_id = e.id AND ec.concept_id = ?5
                   ))
                 ORDER BY e.created_at DESC
                 LIMIT ?6",
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;

        let episodes = stmt
            .query_map(
                params![
                    query.since,
                    query.until,
                    outcome,
                    rung,
                    concept,
                    query.limit,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?
            .map(|row| {
                let json = row.map_err(|e| EkgError::Storage(e.to_string()))?;
                serde_json::from_str(&json).map_err(|e| EkgError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(episodes)
    }

    /// Update an episode (e.g., adding evaluation after the fact).
    pub fn update(&self, episode: &Episode) -> Result<(), EkgError> {
        let data_json =
            serde_json::to_string(episode).map_err(|e| EkgError::Serialization(e.to_string()))?;

        let success = episode.evaluation.as_ref().map(|e| e.success as i32);
        let rung = serde_json::to_value(episode.cost.rung_reached)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        let transaction = self
            .conn
            .unchecked_transaction()
            .map_err(|e| EkgError::Storage(e.to_string()))?;
        let rows = transaction
            .execute(
                "UPDATE episodes SET data_json = ?1, success = ?2, rung_reached = ?3
                 WHERE id = ?4",
                params![data_json, success, rung, episode.id.to_string()],
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;

        if rows == 0 {
            return Err(EkgError::NotFound(format!("episode {}", episode.id)));
        }

        transaction
            .execute(
                "DELETE FROM episode_concepts WHERE episode_id = ?1",
                params![episode.id.to_string()],
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;
        Self::insert_concept_index(&transaction, episode)?;

        transaction
            .commit()
            .map_err(|e| EkgError::Storage(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekg_core::Value;
    use ekg_core::episode::*;
    use ekg_core::evidence::VerifiabilityTier;

    fn make_episode(situation: impl Into<String>) -> Episode {
        Episode::new(situation)
    }

    fn corrupt_episode_json(store: &EpisodeStore, id: EpisodeId, sql_value: &str) {
        store
            .conn
            .execute(
                &format!("UPDATE episodes SET data_json = {sql_value} WHERE id = ?1"),
                params![id.to_string()],
            )
            .unwrap();
    }

    #[test]
    fn insert_and_retrieve() {
        let store = EpisodeStore::in_memory().unwrap();
        let ep = make_episode("what is 2 + 2?");

        store.insert(&ep).unwrap();
        let retrieved = store.get(ep.id).unwrap();

        assert_eq!(retrieved.id, ep.id);
        assert_eq!(retrieved.situation, "what is 2 + 2?");
    }

    #[test]
    fn insert_rolls_back_episode_when_concept_indexing_fails() {
        let store = EpisodeStore::in_memory().unwrap();
        let mut episode = make_episode("must be atomic");
        episode.context.entities.push(ConceptId::new());
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_concept_insert
                 BEFORE INSERT ON episode_concepts
                 BEGIN
                     SELECT RAISE(ABORT, 'rejected concept insert');
                 END;",
            )
            .unwrap();

        assert!(matches!(store.insert(&episode), Err(EkgError::Storage(_))));
        assert_eq!(store.count().unwrap(), 0);
        assert!(matches!(store.get(episode.id), Err(EkgError::NotFound(_))));
    }

    #[test]
    fn list_recent() {
        let store = EpisodeStore::in_memory().unwrap();

        store.insert(&make_episode("first")).unwrap();
        store.insert(&make_episode("second")).unwrap();
        store.insert(&make_episode("third")).unwrap();

        let recent = store.list_recent(2).unwrap();
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn list_failures() {
        let store = EpisodeStore::in_memory().unwrap();

        let mut success = make_episode("success");
        success.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "correct".into(),
            surprise: None,
        });

        let mut failure = make_episode("failure");
        failure.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "wrong".into(),
            surprise: Some(1.0),
        });

        store.insert(&success).unwrap();
        store.insert(&failure).unwrap();

        let failures = store.list_failures(10).unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].situation, "failure");
    }

    #[test]
    fn find_by_concept() {
        let store = EpisodeStore::in_memory().unwrap();
        let concept = ConceptId::new();

        let mut ep = make_episode("involves a concept");
        ep.context.entities.push(concept);

        store.insert(&ep).unwrap();

        let found = store.find_by_concept(concept).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, ep.id);
    }

    #[test]
    fn update_episode() {
        let store = EpisodeStore::in_memory().unwrap();
        let mut ep = make_episode("pending evaluation");

        store.insert(&ep).unwrap();

        ep.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "verified".into(),
            surprise: None,
        });
        ep.observed_result = Some(Value::Int(42));

        store.update(&ep).unwrap();

        let retrieved = store.get(ep.id).unwrap();
        assert!(retrieved.succeeded());
        assert_eq!(retrieved.observed_result, Some(Value::Int(42)));
    }

    #[test]
    fn update_rebuilds_concept_index() {
        let store = EpisodeStore::in_memory().unwrap();
        let removed = ConceptId::new();
        let added = ConceptId::new();
        let mut episode = make_episode("changing concepts");
        episode.context.entities.push(removed);
        store.insert(&episode).unwrap();

        episode.context.entities.clear();
        episode.context.entities.push(added);
        store.update(&episode).unwrap();

        assert!(store.find_by_concept(removed).unwrap().is_empty());
        assert_eq!(store.find_by_concept(added).unwrap()[0].id, episode.id);
    }

    #[test]
    fn update_rolls_back_episode_and_concept_index_when_rebuild_fails() {
        let store = EpisodeStore::in_memory().unwrap();
        let original = ConceptId::new();
        let replacement = ConceptId::new();
        let mut episode = make_episode("original situation");
        episode.context.entities.push(original);
        store.insert(&episode).unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_concept_insert
                 BEFORE INSERT ON episode_concepts
                 BEGIN
                     SELECT RAISE(ABORT, 'rejected concept insert');
                 END;",
            )
            .unwrap();

        episode.situation = "updated situation".into();
        episode.context.entities.clear();
        episode.context.entities.push(replacement);

        assert!(matches!(store.update(&episode), Err(EkgError::Storage(_))));
        assert_eq!(
            store.get(episode.id).unwrap().situation,
            "original situation"
        );
        assert_eq!(store.find_by_concept(original).unwrap()[0].id, episode.id);
        assert!(store.find_by_concept(replacement).unwrap().is_empty());
    }

    #[test]
    fn count() {
        let store = EpisodeStore::in_memory().unwrap();
        assert_eq!(store.count().unwrap(), 0);

        store.insert(&make_episode("one")).unwrap();
        store.insert(&make_episode("two")).unwrap();
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn not_found() {
        let store = EpisodeStore::in_memory().unwrap();
        let result = store.get(EpisodeId::new());
        assert!(result.is_err());
    }

    #[test]
    fn query_filters_by_time_outcome_and_rung() {
        let store = EpisodeStore::in_memory().unwrap();

        let mut old_success = make_episode("old success");
        old_success.created_at = 100;
        old_success.cost.rung_reached = EscalationRung::Recall;
        old_success.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: true,
            details: "verified".into(),
            surprise: None,
        });

        let mut recent_failure = make_episode("recent failure");
        recent_failure.created_at = 200;
        recent_failure.cost.rung_reached = EscalationRung::Run;
        recent_failure.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "wrong".into(),
            surprise: Some(1.0),
        });

        let mut recent_success = make_episode("recent success");
        recent_success.created_at = 300;
        recent_success.cost.rung_reached = EscalationRung::Run;
        recent_success.evaluation = old_success.evaluation.clone();

        store.insert(&old_success).unwrap();
        store.insert(&recent_failure).unwrap();
        store.insert(&recent_success).unwrap();

        let found = store
            .query(&EpisodeQuery {
                since: Some(150),
                until: Some(350),
                outcome: Some(true),
                rung: Some(EscalationRung::Run),
                ..EpisodeQuery::default()
            })
            .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, recent_success.id);
    }

    #[test]
    fn query_filters_by_concept_and_limit() {
        let store = EpisodeStore::in_memory().unwrap();
        let concept = ConceptId::new();

        for created_at in [100, 200, 300] {
            let mut episode = make_episode(format!("episode {created_at}"));
            episode.created_at = created_at;
            episode.context.entities.push(concept);
            store.insert(&episode).unwrap();
        }
        store.insert(&make_episode("unrelated")).unwrap();

        let found = store
            .query(&EpisodeQuery {
                concept: Some(concept),
                limit: 2,
                ..EpisodeQuery::default()
            })
            .unwrap();

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].created_at, 300);
        assert_eq!(found[1].created_at, 200);
    }

    #[test]
    fn episode_collection_reads_propagate_malformed_json() {
        let store = EpisodeStore::in_memory().unwrap();
        let concept = ConceptId::new();
        let mut episode = make_episode("corrupt JSON");
        episode.context.entities.push(concept);
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "failed".into(),
            surprise: None,
        });
        store.insert(&episode).unwrap();
        corrupt_episode_json(&store, episode.id, "'{'");

        assert!(matches!(
            store.list_recent(10),
            Err(EkgError::Serialization(_))
        ));
        assert!(matches!(
            store.list_failures(10),
            Err(EkgError::Serialization(_))
        ));
        assert!(matches!(
            store.find_by_concept(concept),
            Err(EkgError::Serialization(_))
        ));
        assert!(matches!(
            store.query(&EpisodeQuery {
                concept: Some(concept),
                ..EpisodeQuery::default()
            }),
            Err(EkgError::Serialization(_))
        ));
    }

    #[test]
    fn episode_collection_reads_propagate_sqlite_row_errors() {
        let store = EpisodeStore::in_memory().unwrap();
        let concept = ConceptId::new();
        let mut episode = make_episode("invalid SQLite type");
        episode.context.entities.push(concept);
        episode.evaluation = Some(Evaluation {
            tier: VerifiabilityTier::Hard,
            success: false,
            details: "failed".into(),
            surprise: None,
        });
        store.insert(&episode).unwrap();
        corrupt_episode_json(&store, episode.id, "x'FF'");

        assert!(matches!(store.list_recent(10), Err(EkgError::Storage(_))));
        assert!(matches!(store.list_failures(10), Err(EkgError::Storage(_))));
        assert!(matches!(
            store.find_by_concept(concept),
            Err(EkgError::Storage(_))
        ));
        assert!(matches!(
            store.query(&EpisodeQuery {
                concept: Some(concept),
                ..EpisodeQuery::default()
            }),
            Err(EkgError::Storage(_))
        ));
    }

    #[test]
    fn rung_distribution_propagates_sqlite_row_errors() {
        let store = EpisodeStore::in_memory().unwrap();
        let episode = make_episode("invalid rung type");
        store.insert(&episode).unwrap();
        store
            .conn
            .execute(
                "UPDATE episodes SET rung_reached = x'FF' WHERE id = ?1",
                params![episode.id.to_string()],
            )
            .unwrap();

        assert!(matches!(
            store.rung_distribution(),
            Err(EkgError::Storage(_))
        ));
    }
}
