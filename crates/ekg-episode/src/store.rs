use rusqlite::{Connection, params};
use ekg_core::{
    Episode, EpisodeId, EkgError,
    concept::ConceptId,
};

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
                "CREATE TABLE IF NOT EXISTS episodes (
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
        let rung = serde_json::to_value(&episode.cost.rung_reached)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        self.conn
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

        for interp in &episode.interpretations {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO episode_concepts (episode_id, concept_id, role)
                     VALUES (?1, ?2, 'interpretation')",
                    params![episode.id.to_string(), interp.meaning.to_string()],
                )
                .map_err(|e| EkgError::Storage(e.to_string()))?;
        }

        for entity in &episode.context.entities {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO episode_concepts (episode_id, concept_id, role)
                     VALUES (?1, ?2, 'context')",
                    params![episode.id.to_string(), entity.to_string()],
                )
                .map_err(|e| EkgError::Storage(e.to_string()))?;
        }

        for candidate in &episode.knowledge_considered {
            self.conn
                .execute(
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
                rusqlite::Error::QueryReturnedNoRows => {
                    EkgError::NotFound(format!("episode {id}"))
                }
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
            .filter_map(|r| r.ok())
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect();

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
            .filter_map(|r| r.ok())
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect();

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
            .filter_map(|r| r.ok())
            .filter_map(|json| serde_json::from_str(&json).ok())
            .collect();

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
            .filter_map(|r| r.ok())
            .collect();

        Ok(dist)
    }

    pub fn count(&self) -> Result<u64, EkgError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM episodes", [], |row| row.get(0))
            .map_err(|e| EkgError::Storage(e.to_string()))
    }

    /// Update an episode (e.g., adding evaluation after the fact).
    pub fn update(&self, episode: &Episode) -> Result<(), EkgError> {
        let data_json =
            serde_json::to_string(episode).map_err(|e| EkgError::Serialization(e.to_string()))?;

        let success = episode.evaluation.as_ref().map(|e| e.success as i32);
        let rung = serde_json::to_value(&episode.cost.rung_reached)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        let rows = self
            .conn
            .execute(
                "UPDATE episodes SET data_json = ?1, success = ?2, rung_reached = ?3
                 WHERE id = ?4",
                params![data_json, success, rung, episode.id.to_string()],
            )
            .map_err(|e| EkgError::Storage(e.to_string()))?;

        if rows == 0 {
            return Err(EkgError::NotFound(format!("episode {}", episode.id)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekg_core::episode::*;
    use ekg_core::evidence::VerifiabilityTier;
    use ekg_core::Value;

    fn make_episode(situation: &str) -> Episode {
        Episode::new(situation)
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
}
