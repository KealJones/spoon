use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use spoon_core::{Concept, Episode, Procedure, Relationship};

use crate::EngineError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DurableLessonStage {
    pub stage_id: String,
    pub bundle_key: String,
    pub request_binding_digest: String,
    pub concepts: Vec<Concept>,
    pub relationships: Vec<Relationship>,
    pub procedures: Vec<Procedure>,
    pub episode: Episode,
}

pub(crate) struct LessonStageStore {
    conn: Connection,
}

impl LessonStageStore {
    pub fn open(path: &str) -> Result<Self, EngineError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, EngineError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS teacher_lesson_stages (
                stage_id TEXT PRIMARY KEY,
                request_binding_digest TEXT NOT NULL,
                stage_json TEXT NOT NULL,
                completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1))
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn stage(&self, stage: &DurableLessonStage) -> Result<(), EngineError> {
        validate_stage_id(&stage.stage_id)?;
        let stage_json = serde_json::to_string(stage)?;
        let existing = self
            .conn
            .query_row(
                "SELECT request_binding_digest, stage_json
                 FROM teacher_lesson_stages WHERE stage_id = ?1",
                params![stage.stage_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((binding, stored)) = existing {
            if binding == stage.request_binding_digest && stored == stage_json {
                return Ok(());
            }
            return Err(EngineError::InvalidInput(format!(
                "teacher lesson stage {} was reused with a different payload",
                stage.stage_id
            )));
        }
        self.conn.execute(
            "INSERT INTO teacher_lesson_stages
                (stage_id, request_binding_digest, stage_json, completed)
             VALUES (?1, ?2, ?3, 0)",
            params![stage.stage_id, stage.request_binding_digest, stage_json],
        )?;
        Ok(())
    }

    pub fn pending(&self) -> Result<Vec<DurableLessonStage>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT stage_json FROM teacher_lesson_stages
             WHERE completed = 0 ORDER BY stage_id ASC",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut stages = Vec::new();
        for row in rows {
            stages.push(serde_json::from_str(&row?)?);
        }
        Ok(stages)
    }

    pub fn complete(&self, stage: &DurableLessonStage) -> Result<(), EngineError> {
        let stage_json = serde_json::to_string(stage)?;
        let changed = self.conn.execute(
            "UPDATE teacher_lesson_stages SET completed = 1
             WHERE stage_id = ?1 AND request_binding_digest = ?2 AND stage_json = ?3",
            params![stage.stage_id, stage.request_binding_digest, stage_json],
        )?;
        if changed != 1 {
            return Err(EngineError::InvalidInput(format!(
                "teacher lesson stage {} no longer matches its exact payload",
                stage.stage_id
            )));
        }
        Ok(())
    }

    pub fn discard(&self, stage: &DurableLessonStage) -> Result<(), EngineError> {
        let stage_json = serde_json::to_string(stage)?;
        self.conn.execute(
            "DELETE FROM teacher_lesson_stages
             WHERE stage_id = ?1 AND request_binding_digest = ?2 AND stage_json = ?3 AND completed = 0",
            params![
                stage.stage_id,
                stage.request_binding_digest,
                stage_json
            ],
        )?;
        Ok(())
    }
}

fn validate_stage_id(stage_id: &str) -> Result<(), EngineError> {
    if stage_id.trim().is_empty() || stage_id.len() > 256 {
        return Err(EngineError::InvalidInput(
            "teacher lesson stage id must contain 1 to 256 bytes".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use spoon_core::{Concept, Expr, Lifecycle, MutabilityClass, Param, Procedure, Value};
    use uuid::Uuid;

    use super::*;
    use crate::Engine;

    fn recovery_stage() -> DurableLessonStage {
        let mut concept = Concept::new("DOUBLE", MutabilityClass::Procedural);
        concept.lifecycle = Lifecycle::Provisional;
        let mut procedure = Procedure::new(
            "DOUBLE",
            vec![Param::named("x")],
            Expr::BinOp {
                op: spoon_core::BinOp::Mul,
                left: Box::new(Expr::Var("x".into())),
                right: Box::new(Expr::Literal(Value::Int(2))),
            },
        )
        .with_concept(concept.id);
        procedure.lifecycle = Lifecycle::Provisional;
        let mut episode = Episode::new("what is double 7?");
        episode.observed_result = Some(Value::Int(14));
        DurableLessonStage {
            stage_id: "lesson-stage:recovery-test".into(),
            bundle_key: "teacher-lesson:recovery-test".into(),
            request_binding_digest: "sha256:bound-request-and-proposal".into(),
            concepts: vec![concept],
            relationships: Vec::new(),
            procedures: vec![procedure],
            episode,
        }
    }

    #[test]
    fn reopen_recovers_graph_commit_with_missing_terminal_episode_exactly_once() {
        let path = std::env::temp_dir().join(format!(
            "spoon-teacher-lesson-stage-{}.sqlite",
            Uuid::new_v4()
        ));
        let path_text = path.to_string_lossy().into_owned();
        let stage = recovery_stage();
        {
            let engine = Engine::open(&path_text).unwrap();
            engine.lesson_stages.stage(&stage).unwrap();
            engine
                .graph
                .insert_knowledge_bundle(
                    &stage.bundle_key,
                    &stage.concepts,
                    &stage.relationships,
                    &stage.procedures,
                )
                .unwrap();
            assert_eq!(engine.episodes.count().unwrap(), 0);
        }
        {
            let engine = Engine::open(&path_text).unwrap();
            assert_eq!(engine.graph.list_concepts().unwrap().len(), 1);
            assert_eq!(engine.graph.list_procedures().unwrap().len(), 1);
            assert_eq!(engine.episodes.count().unwrap(), 1);
            assert!(engine.lesson_stages.pending().unwrap().is_empty());
        }
        {
            let engine = Engine::open(&path_text).unwrap();
            assert_eq!(engine.episodes.count().unwrap(), 1);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn lesson_stage_retry_is_exact_and_conflicting_request_binding_is_rejected() {
        let store = LessonStageStore::in_memory().unwrap();
        let stage = recovery_stage();
        store.stage(&stage).unwrap();
        store.stage(&stage).unwrap();
        let mut conflict = stage;
        conflict.request_binding_digest = "sha256:different-request".into();
        assert!(matches!(
            store.stage(&conflict),
            Err(EngineError::InvalidInput(message)) if message.contains("different payload")
        ));
        assert_eq!(store.pending().unwrap().len(), 1);
    }
}
