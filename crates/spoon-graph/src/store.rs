use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use spoon_core::{
    Concept, ConceptId, Confidence, Contract, Expr, Lifecycle, MutabilityClass, Param, Procedure,
    ProcedureId, Relationship, RelationshipId, ScopeCondition, TestCase,
};

use crate::error::{GraphError, Result};
use crate::schema;

/// A SQLite-backed store for concepts, relationships, and procedures.
///
/// Simple scalar fields (names, timestamps, lifecycle tags) live in native
/// columns so they can be indexed and filtered directly. Nested structured
/// data (confidence, scope, contracts, procedure bodies) is stored as JSON
/// text, since it's read/written wholesale and never queried by sub-field.
pub struct KnowledgeStore {
    pub(crate) conn: Connection,
}

const MAX_KNOWLEDGE_BUNDLE_CONCEPTS: usize = 8;
const MAX_KNOWLEDGE_BUNDLE_RELATIONSHIPS: usize = 16;
const MAX_KNOWLEDGE_BUNDLE_PROCEDURES: usize = 1;

/// Maximum number of relationships returned by the bounded read collection.
///
/// The collection is intended for inspection and API consumers, so callers
/// cannot accidentally turn it into an unbounded database read.
pub const MAX_RELATIONSHIP_LIST_LIMIT: u32 = 1_024;

fn bundle_reference_lifecycle(lifecycle: Lifecycle) -> bool {
    matches!(
        lifecycle,
        Lifecycle::Active | Lifecycle::Validated | Lifecycle::Provisional
    )
}

/// An entity whose downstream dependencies should be inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependencyTarget {
    Concept(ConceptId),
    Procedure(ProcedureId),
}

/// Why a procedure depends on the report's target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcedureDependencyKind {
    AttachedToConcept,
    CallsProcedure,
}

/// A current graph entity that depends on another concept or procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dependent {
    Concept(ConceptId),
    Procedure {
        id: ProcedureId,
        kind: ProcedureDependencyKind,
    },
}

/// A live relationship claim whose validity depends on the report target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RelationshipDependency {
    pub relationship_id: RelationshipId,
    pub version: u32,
    pub source: ConceptId,
    pub target: ConceptId,
    pub direction: RelationshipDependencyDirection,
}

/// Which endpoint's current validity depends on the other endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelationshipDependencyDirection {
    SourceDependsOnTarget,
    TargetDependsOnSource,
    Bidirectional,
    Unknown,
}

/// The current entities that would be affected by changing `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyReport {
    pub target: DependencyTarget,
    pub dependents: Vec<Dependent>,
    pub relationships: Vec<RelationshipDependency>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LifecycleTarget {
    Concept { id: ConceptId },
    Procedure { id: ProcedureId },
    Relationship { id: RelationshipId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LifecycleChange {
    Concept {
        id: ConceptId,
        expected_version: u32,
        lifecycle: Lifecycle,
    },
    Procedure {
        id: ProcedureId,
        expected_version: u32,
        lifecycle: Lifecycle,
    },
    Relationship {
        id: RelationshipId,
        expected_version: u32,
        lifecycle: Lifecycle,
    },
}

impl LifecycleChange {
    pub fn target(&self) -> LifecycleTarget {
        match self {
            Self::Concept { id, .. } => LifecycleTarget::Concept { id: *id },
            Self::Procedure { id, .. } => LifecycleTarget::Procedure { id: *id },
            Self::Relationship { id, .. } => LifecycleTarget::Relationship { id: *id },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleChangeSet {
    pub idempotency_key: String,
    pub updated_at: i64,
    pub changes: Vec<LifecycleChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedLifecycleChange {
    pub target: LifecycleTarget,
    pub previous_version: u32,
    pub current_version: u32,
    pub previous_lifecycle: Lifecycle,
    pub current_lifecycle: Lifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleChangeReceipt {
    pub idempotency_key: String,
    pub updated_at: i64,
    pub changes: Vec<AppliedLifecycleChange>,
}

enum PreparedLifecycleChange {
    Concept {
        id: ConceptId,
        previous_version: u32,
        current_version: u32,
        previous_lifecycle: Lifecycle,
        current_lifecycle: Lifecycle,
    },
    Procedure {
        id: ProcedureId,
        previous_version: u32,
        current_version: u32,
        previous_lifecycle: Lifecycle,
        current_lifecycle: Lifecycle,
    },
    Relationship {
        id: RelationshipId,
        previous_version: u32,
        current_version: u32,
        previous_lifecycle: Lifecycle,
        current_lifecycle: Lifecycle,
    },
}

impl KnowledgeStore {
    /// Opens (or creates) a SQLite database at `path` and ensures the
    /// schema exists.
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        schema::init(&conn)?;
        Ok(Self { conn })
    }

    /// Opens an in-memory database. Useful for tests and ephemeral use.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::init(&conn)?;
        Ok(Self { conn })
    }

    /// Atomically inserts one bounded, engine-sanitized provisional lesson.
    /// The teacher never controls lifecycle escalation or historical fields;
    /// exact idempotent retries are accepted and key/payload reuse conflicts.
    pub fn insert_knowledge_bundle(
        &self,
        idempotency_key: &str,
        concepts: &[Concept],
        relationships: &[Relationship],
        procedures: &[Procedure],
    ) -> Result<()> {
        self.insert_knowledge_bundle_in(idempotency_key, concepts, relationships, procedures, None)
    }

    fn insert_knowledge_bundle_in(
        &self,
        idempotency_key: &str,
        concepts: &[Concept],
        relationships: &[Relationship],
        procedures: &[Procedure],
        fail_after_inserts: Option<usize>,
    ) -> Result<()> {
        if idempotency_key.trim().is_empty() || idempotency_key.len() > 256 {
            return Err(GraphError::InvalidKnowledgeBundle(
                "idempotency key must contain 1 to 256 bytes".into(),
            ));
        }
        if concepts.is_empty()
            || concepts.len() > MAX_KNOWLEDGE_BUNDLE_CONCEPTS
            || relationships.len() > MAX_KNOWLEDGE_BUNDLE_RELATIONSHIPS
            || procedures.len() != MAX_KNOWLEDGE_BUNDLE_PROCEDURES
        {
            return Err(GraphError::InvalidKnowledgeBundle(
                "bundle must contain 1..=8 concepts, 0..=16 relationships, and exactly one procedure"
                    .into(),
            ));
        }
        // Wall-clock fields are engine-owned metadata, not proposal payload.
        // Excluding them from receipt equality lets a crash recovery rebuild
        // the same deterministic bundle without a false conflict.
        let mut canonical_concepts = concepts.to_vec();
        for concept in &mut canonical_concepts {
            concept.created_at = 0;
            concept.updated_at = 0;
        }
        let mut canonical_relationships = relationships.to_vec();
        for relationship in &mut canonical_relationships {
            relationship.created_at = 0;
        }
        let mut canonical_procedures = procedures.to_vec();
        for procedure in &mut canonical_procedures {
            procedure.created_at = 0;
            procedure.updated_at = 0;
        }
        let request_json = serde_json::to_string(&(
            canonical_concepts,
            canonical_relationships,
            canonical_procedures,
        ))?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if let Some(stored) = tx
            .query_row(
                "SELECT request_json FROM knowledge_bundle_receipts WHERE idempotency_key = ?1",
                params![idempotency_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            if stored == request_json {
                return Ok(());
            }
            return Err(GraphError::IdempotencyConflict {
                key: idempotency_key.into(),
            });
        }

        let mut concept_ids = HashSet::new();
        let mut concept_names = HashSet::new();
        for concept in concepts {
            if concept.lifecycle != Lifecycle::Provisional
                || concept.mutability != MutabilityClass::Procedural
            {
                return Err(GraphError::InvalidKnowledgeBundle(
                    "new concepts must be engine-owned Provisional Procedural knowledge".into(),
                ));
            }
            if concept.name.trim().is_empty()
                || concept.name.chars().count() > 256
                || concept
                    .description
                    .as_ref()
                    .is_some_and(|value| value.chars().count() > 2_048)
                || !concept_ids.insert(concept.id)
                || !concept_names.insert(concept.name.to_lowercase())
            {
                return Err(GraphError::InvalidKnowledgeBundle(
                    "concept names/ids must be nonempty, bounded, and unique".into(),
                ));
            }
            let collision: bool = tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM concepts WHERE id = ?1 OR lower(name) = lower(?2)
                )",
                params![concept.id.0.to_string(), concept.name],
                |row| row.get(0),
            )?;
            if collision {
                return Err(GraphError::InvalidKnowledgeBundle(format!(
                    "concept {} collides with existing knowledge",
                    concept.name
                )));
            }
        }

        let procedure_ids = procedures
            .iter()
            .map(|procedure| procedure.id)
            .collect::<HashSet<_>>();
        for procedure in procedures {
            if procedure.lifecycle != Lifecycle::Provisional
                || procedure.version != 1
                || !procedure.test_cases.is_empty()
                || procedure.name.trim().is_empty()
                || procedure.name.chars().count() > 256
            {
                return Err(GraphError::InvalidKnowledgeBundle(
                    "procedures must be fresh bounded Provisional version-1 drafts without caller test cases"
                        .into(),
                ));
            }
            let id_collision: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM procedures WHERE id = ?1 OR lower(name) = lower(?2))",
                params![procedure.id.0.to_string(), procedure.name],
                |row| row.get(0),
            )?;
            if id_collision {
                return Err(GraphError::InvalidKnowledgeBundle(format!(
                    "procedure {} collides with existing knowledge",
                    procedure.name
                )));
            }
            let concept_id = procedure.concept.ok_or_else(|| {
                GraphError::InvalidKnowledgeBundle(
                    "a learned procedure must attach to a concept".into(),
                )
            })?;
            Self::validate_bundle_concept_reference(&tx, concept_id, &concept_ids)?;

            let mut calls = HashSet::new();
            Self::collect_expression_calls(&procedure.body, &mut calls);
            for condition in procedure
                .contract
                .requires
                .iter()
                .chain(&procedure.contract.promises)
                .chain(&procedure.contract.fails_when)
            {
                if let Some(check) = &condition.check {
                    Self::collect_expression_calls(check, &mut calls);
                }
            }
            for call in calls {
                if procedure_ids.contains(&call) {
                    return Err(GraphError::InvalidKnowledgeBundle(
                        "lesson procedures may call only pre-existing executable procedures".into(),
                    ));
                }
                let lifecycle = tx
                    .query_row(
                        "SELECT lifecycle FROM procedures WHERE id = ?1",
                        params![call.0.to_string()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                    .map(|value| serde_json::from_str::<Lifecycle>(&value))
                    .transpose()?
                    .ok_or_else(|| {
                        GraphError::InvalidKnowledgeBundle(format!(
                            "procedure call target {call} is absent"
                        ))
                    })?;
                if !bundle_reference_lifecycle(lifecycle) {
                    return Err(GraphError::InvalidKnowledgeBundle(format!(
                        "procedure call target {call} is not executable"
                    )));
                }
            }
        }

        let mut relationship_ids = HashSet::new();
        for relationship in relationships {
            if relationship.lifecycle != Lifecycle::Provisional
                || !relationship.strength.is_finite()
                || !(0.0..=1.0).contains(&relationship.strength)
                || relationship.kind.trim().is_empty()
                || relationship.kind.chars().count() > 256
                || !relationship.scope.is_empty()
                || !relationship.evidence.is_empty()
                || !relationship_ids.insert(relationship.id)
            {
                return Err(GraphError::InvalidKnowledgeBundle(
                    "relationships must be bounded fresh Provisional claims without caller evidence or scope"
                        .into(),
                ));
            }
            Self::validate_bundle_concept_reference(&tx, relationship.source, &concept_ids)?;
            Self::validate_bundle_concept_reference(&tx, relationship.target, &concept_ids)?;
            let collision: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM relationships WHERE id = ?1)",
                params![relationship.id.0.to_string()],
                |row| row.get(0),
            )?;
            if collision {
                return Err(GraphError::InvalidKnowledgeBundle(format!(
                    "relationship {} collides with existing knowledge",
                    relationship.id
                )));
            }
        }

        let mut inserted = 0_usize;
        for concept in concepts {
            tx.execute(
                "INSERT INTO concepts
                    (id, name, description, mutability, confidence_json, lifecycle, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    concept.id.0.to_string(),
                    concept.name,
                    concept.description,
                    serde_json::to_string(&concept.mutability)?,
                    serde_json::to_string(&concept.confidence)?,
                    serde_json::to_string(&concept.lifecycle)?,
                    concept.created_at,
                    concept.updated_at,
                ],
            )?;
            Self::insert_concept_snapshot(&tx, concept, 1)?;
            inserted += 1;
            Self::bundle_failpoint(fail_after_inserts, inserted)?;
        }
        for relationship in relationships {
            tx.execute(
                "INSERT INTO relationships
                    (id, source, target, kind, strength, scope_json, evidence_json, lifecycle, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    relationship.id.0.to_string(),
                    relationship.source.0.to_string(),
                    relationship.target.0.to_string(),
                    relationship.kind,
                    relationship.strength,
                    serde_json::to_string(&relationship.scope)?,
                    serde_json::to_string(&relationship.evidence)?,
                    serde_json::to_string(&relationship.lifecycle)?,
                    relationship.created_at,
                ],
            )?;
            Self::insert_relationship_snapshot(&tx, relationship, 1)?;
            inserted += 1;
            Self::bundle_failpoint(fail_after_inserts, inserted)?;
        }
        for procedure in procedures {
            tx.execute(
                "INSERT INTO procedures
                    (id, name, params_json, body_json, contract_json, test_cases_json,
                     concept_id, version, lifecycle, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    procedure.id.0.to_string(),
                    procedure.name,
                    serde_json::to_string(&procedure.params)?,
                    serde_json::to_string(&procedure.body)?,
                    serde_json::to_string(&procedure.contract)?,
                    serde_json::to_string(&procedure.test_cases)?,
                    procedure.concept.map(|concept| concept.0.to_string()),
                    procedure.version,
                    serde_json::to_string(&procedure.lifecycle)?,
                    procedure.created_at,
                    procedure.updated_at,
                ],
            )?;
            Self::insert_procedure_snapshot(&tx, procedure)?;
            inserted += 1;
            Self::bundle_failpoint(fail_after_inserts, inserted)?;
        }
        tx.execute(
            "INSERT INTO knowledge_bundle_receipts (idempotency_key, request_json, created_at)
             VALUES (?1, ?2, CAST(strftime('%s', 'now') AS INTEGER))",
            params![idempotency_key, request_json],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn validate_bundle_concept_reference(
        conn: &Connection,
        id: ConceptId,
        new_concepts: &HashSet<ConceptId>,
    ) -> Result<()> {
        if new_concepts.contains(&id) {
            return Ok(());
        }
        let lifecycle = conn
            .query_row(
                "SELECT lifecycle FROM concepts WHERE id = ?1",
                params![id.0.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| serde_json::from_str::<Lifecycle>(&value))
            .transpose()?
            .ok_or_else(|| {
                GraphError::InvalidKnowledgeBundle(format!("concept reference {id} is absent"))
            })?;
        if !bundle_reference_lifecycle(lifecycle) {
            return Err(GraphError::InvalidKnowledgeBundle(format!(
                "concept reference {id} is inactive"
            )));
        }
        Ok(())
    }

    fn bundle_failpoint(fail_after: Option<usize>, inserted: usize) -> Result<()> {
        if fail_after == Some(inserted) {
            return Err(GraphError::InvalidKnowledgeBundle(
                "injected knowledge-bundle failure".into(),
            ));
        }
        Ok(())
    }

    fn collect_expression_calls(expression: &Expr, calls: &mut HashSet<ProcedureId>) {
        match expression {
            Expr::Literal(_) | Expr::Var(_) => {}
            Expr::BinOp { left, right, .. } => {
                Self::collect_expression_calls(left, calls);
                Self::collect_expression_calls(right, calls);
            }
            Expr::UnOp { operand, .. } => Self::collect_expression_calls(operand, calls),
            Expr::Call { procedure, args } => {
                calls.insert(*procedure);
                for argument in args {
                    Self::collect_expression_calls(argument, calls);
                }
            }
            Expr::If { cond, then, else_ } => {
                Self::collect_expression_calls(cond, calls);
                Self::collect_expression_calls(then, calls);
                Self::collect_expression_calls(else_, calls);
            }
            Expr::Let { value, body, .. } => {
                Self::collect_expression_calls(value, calls);
                Self::collect_expression_calls(body, calls);
            }
            Expr::Block(expressions) | Expr::ListExpr(expressions) => {
                for expression in expressions {
                    Self::collect_expression_calls(expression, calls);
                }
            }
            Expr::Index { collection, index } => {
                Self::collect_expression_calls(collection, calls);
                Self::collect_expression_calls(index, calls);
            }
            Expr::FieldAccess { object, .. } => Self::collect_expression_calls(object, calls),
            Expr::Map {
                collection, body, ..
            } => {
                Self::collect_expression_calls(collection, calls);
                Self::collect_expression_calls(body, calls);
            }
            Expr::Filter {
                collection,
                predicate,
                ..
            } => {
                Self::collect_expression_calls(collection, calls);
                Self::collect_expression_calls(predicate, calls);
            }
            Expr::Reduce {
                collection,
                init,
                body,
                ..
            } => {
                Self::collect_expression_calls(collection, calls);
                Self::collect_expression_calls(init, calls);
                Self::collect_expression_calls(body, calls);
            }
        }
    }

    // ---------------------------------------------------------------
    // Concepts
    // ---------------------------------------------------------------

    pub fn insert_concept(&self, concept: &Concept) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO concepts \
                (id, name, description, mutability, confidence_json, lifecycle, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                concept.id.0.to_string(),
                concept.name,
                concept.description,
                serde_json::to_string(&concept.mutability)?,
                serde_json::to_string(&concept.confidence)?,
                serde_json::to_string(&concept.lifecycle)?,
                concept.created_at,
                concept.updated_at,
            ],
        )?;
        Self::insert_concept_snapshot(&tx, concept, 1)?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_concept(&self, id: ConceptId) -> Result<Option<Concept>> {
        self.conn
            .query_row(
                "SELECT id, name, description, mutability, confidence_json, lifecycle, \
                        created_at, updated_at \
                 FROM concepts WHERE id = ?1",
                params![id.0.to_string()],
                Self::concept_from_row,
            )
            .optional()?
            .transpose()
    }

    pub fn get_concept_by_name(&self, name: &str) -> Result<Option<Concept>> {
        self.conn
            .query_row(
                "SELECT id, name, description, mutability, confidence_json, lifecycle, \
                        created_at, updated_at \
                 FROM concepts WHERE name = ?1",
                params![name],
                Self::concept_from_row,
            )
            .optional()?
            .transpose()
    }

    pub fn update_concept(&self, concept: &Concept) -> Result<()> {
        let current_version = self.current_concept_version(concept.id)?;
        if current_version != 1 {
            return Err(GraphError::ExpectedVersionRequired {
                entity: format!("concept {}", concept.id),
            });
        }
        self.revise_concept(concept, current_version)?;
        Ok(())
    }

    /// Atomically replaces the current concept and records an immutable
    /// snapshot when `expected_version` still matches the store.
    pub fn revise_concept(&self, concept: &Concept, expected_version: u32) -> Result<u32> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let (actual, created_at) = Self::current_concept_state_in(&tx, concept.id)?;
        if actual != expected_version {
            return Err(GraphError::RevisionConflict {
                entity: format!("concept {}", concept.id),
                expected: expected_version,
                actual,
            });
        }
        if concept.created_at != created_at {
            return Err(GraphError::ImmutableFieldChange {
                entity: format!("concept {}", concept.id),
                field: "created_at",
            });
        }
        let next_version =
            actual
                .checked_add(1)
                .ok_or_else(|| GraphError::NonMonotonicRevision {
                    entity: format!("concept {}", concept.id),
                    expected_next: u32::MAX,
                    proposed: actual,
                })?;
        let changed = tx.execute(
            "UPDATE concepts SET \
                name = ?2, description = ?3, mutability = ?4, confidence_json = ?5, \
                lifecycle = ?6, updated_at = ?7 \
             WHERE id = ?1",
            params![
                concept.id.0.to_string(),
                concept.name,
                concept.description,
                serde_json::to_string(&concept.mutability)?,
                serde_json::to_string(&concept.confidence)?,
                serde_json::to_string(&concept.lifecycle)?,
                concept.updated_at,
            ],
        )?;
        if changed == 0 {
            return Err(GraphError::NotFound(format!("concept {}", concept.id)));
        }
        let committed = tx.query_row(
            "SELECT id, name, description, mutability, confidence_json, lifecycle,
                    created_at, updated_at
             FROM concepts WHERE id = ?1",
            params![concept.id.0.to_string()],
            Self::concept_from_row,
        )??;
        Self::insert_concept_snapshot(&tx, &committed, next_version)?;
        tx.commit()?;
        Ok(next_version)
    }

    /// Returns a concept snapshot at an exact revision.
    pub fn get_concept_version(&self, id: ConceptId, version: u32) -> Result<Option<Concept>> {
        self.conn
            .query_row(
                "SELECT id, name, description, mutability, confidence_json, lifecycle,
                        created_at, updated_at
                 FROM concept_versions WHERE id = ?1 AND version = ?2",
                params![id.0.to_string(), version],
                Self::concept_from_row,
            )
            .optional()?
            .transpose()
    }

    /// Lists concept snapshots oldest first.
    pub fn list_concept_versions(&self, id: ConceptId) -> Result<Vec<Concept>> {
        let mut statement = self.conn.prepare(
            "SELECT id, name, description, mutability, confidence_json, lifecycle,
                    created_at, updated_at
             FROM concept_versions WHERE id = ?1 ORDER BY version ASC",
        )?;
        let rows = statement.query_map(params![id.0.to_string()], Self::concept_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    pub fn current_concept_version(&self, id: ConceptId) -> Result<u32> {
        Self::current_concept_version_in(&self.conn, id)
    }

    fn current_concept_version_in(conn: &Connection, id: ConceptId) -> Result<u32> {
        Self::current_concept_state_in(conn, id).map(|(version, _)| version)
    }

    fn current_concept_state_in(conn: &Connection, id: ConceptId) -> Result<(u32, i64)> {
        conn.query_row(
            "SELECT MAX(versions.version), current.created_at
             FROM concepts AS current
             JOIN concept_versions AS versions ON versions.id = current.id
             WHERE current.id = ?1
             GROUP BY current.id, current.created_at",
            params![id.0.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| GraphError::NotFound(format!("concept {id}")))
    }

    fn insert_concept_snapshot(conn: &Connection, concept: &Concept, version: u32) -> Result<()> {
        conn.execute(
            "INSERT INTO concept_versions
                (id, version, name, description, mutability, confidence_json,
                 lifecycle, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                concept.id.0.to_string(),
                version,
                concept.name,
                concept.description,
                serde_json::to_string(&concept.mutability)?,
                serde_json::to_string(&concept.confidence)?,
                serde_json::to_string(&concept.lifecycle)?,
                concept.created_at,
                concept.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Deletes a concept only when no live relationship or procedure
    /// references it.
    pub fn delete_concept(&self, id: ConceptId) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let dependent_count: i64 = tx.query_row(
            "SELECT \
                (SELECT COUNT(*) FROM relationships WHERE source = ?1 OR target = ?1) + \
                (SELECT COUNT(*) FROM procedures WHERE concept_id = ?1)",
            params![id.0.to_string()],
            |row| row.get(0),
        )?;
        if dependent_count > 0 {
            return Err(GraphError::HasDependents(format!(
                "concept {id} has {dependent_count} live reference(s)"
            )));
        }

        let changed = tx.execute(
            "DELETE FROM concepts WHERE id = ?1",
            params![id.0.to_string()],
        )?;
        if changed == 0 {
            return Err(GraphError::NotFound(format!("concept {id}")));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_concepts(&self) -> Result<Vec<Concept>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, mutability, confidence_json, lifecycle, \
                    created_at, updated_at \
             FROM concepts ORDER BY name",
        )?;
        let rows = stmt.query_map([], Self::concept_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    /// Returns concepts with exactly the requested mutability class.
    pub fn get_concepts_by_mutability(&self, mutability: MutabilityClass) -> Result<Vec<Concept>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, mutability, confidence_json, lifecycle, \
                    created_at, updated_at \
             FROM concepts WHERE mutability = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(
            params![serde_json::to_string(&mutability)?],
            Self::concept_from_row,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    fn concept_from_row(row: &Row) -> rusqlite::Result<Result<Concept>> {
        Ok((|| -> Result<Concept> {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let description: Option<String> = row.get(2)?;
            let mutability_json: String = row.get(3)?;
            let confidence_json: String = row.get(4)?;
            let lifecycle_json: String = row.get(5)?;
            let created_at: i64 = row.get(6)?;
            let updated_at: i64 = row.get(7)?;

            Ok(Concept {
                id: ConceptId(Uuid::parse_str(&id)?),
                name,
                description,
                mutability: serde_json::from_str::<MutabilityClass>(&mutability_json)?,
                confidence: serde_json::from_str::<Confidence>(&confidence_json)?,
                lifecycle: serde_json::from_str::<Lifecycle>(&lifecycle_json)?,
                created_at,
                updated_at,
            })
        })())
    }

    // ---------------------------------------------------------------
    // Relationships
    // ---------------------------------------------------------------

    pub fn insert_relationship(&self, rel: &Relationship) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO relationships \
                (id, source, target, kind, strength, scope_json, evidence_json, lifecycle, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                rel.id.0.to_string(),
                rel.source.0.to_string(),
                rel.target.0.to_string(),
                rel.kind,
                rel.strength,
                serde_json::to_string(&rel.scope)?,
                serde_json::to_string(&rel.evidence)?,
                serde_json::to_string(&rel.lifecycle)?,
                rel.created_at,
            ],
        )?;
        Self::insert_relationship_snapshot(&tx, rel, 1)?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_relationship(&self, id: RelationshipId) -> Result<Option<Relationship>> {
        self.conn
            .query_row(
                "SELECT id, source, target, kind, strength, scope_json, evidence_json, \
                        lifecycle, created_at \
                 FROM relationships WHERE id = ?1",
                params![id.0.to_string()],
                Self::relationship_from_row,
            )
            .optional()?
            .transpose()
    }

    /// Returns a deterministic, bounded snapshot of current relationships.
    ///
    /// This is deliberately read-only. Requests above the hard limit are
    /// capped rather than causing an unexpectedly large query.
    pub fn list_relationships(&self, limit: u32) -> Result<Vec<Relationship>> {
        let limit = limit.min(MAX_RELATIONSHIP_LIST_LIMIT);
        let mut stmt = self.conn.prepare(
            "SELECT id, source, target, kind, strength, scope_json, evidence_json, \
                    lifecycle, created_at \
             FROM relationships ORDER BY id LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![i64::from(limit)], Self::relationship_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    pub fn get_relationships_from(&self, concept_id: ConceptId) -> Result<Vec<Relationship>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, target, kind, strength, scope_json, evidence_json, \
                    lifecycle, created_at \
             FROM relationships WHERE source = ?1",
        )?;
        let rows = stmt.query_map(
            params![concept_id.0.to_string()],
            Self::relationship_from_row,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    pub fn get_relationships_to(&self, concept_id: ConceptId) -> Result<Vec<Relationship>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, target, kind, strength, scope_json, evidence_json, \
                    lifecycle, created_at \
             FROM relationships WHERE target = ?1",
        )?;
        let rows = stmt.query_map(
            params![concept_id.0.to_string()],
            Self::relationship_from_row,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    pub fn get_relationships_by_kind(&self, kind: &str) -> Result<Vec<Relationship>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, target, kind, strength, scope_json, evidence_json, \
                    lifecycle, created_at \
             FROM relationships WHERE kind = ?1",
        )?;
        let rows = stmt.query_map(params![kind], Self::relationship_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    pub fn update_relationship(&self, rel: &Relationship) -> Result<()> {
        let current_version = self.current_relationship_version(rel.id)?;
        if current_version != 1 {
            return Err(GraphError::ExpectedVersionRequired {
                entity: format!("relationship {}", rel.id),
            });
        }
        self.revise_relationship(rel, current_version)?;
        Ok(())
    }

    /// Atomically replaces a relationship and records an immutable snapshot
    /// when `expected_version` still matches the store.
    pub fn revise_relationship(&self, rel: &Relationship, expected_version: u32) -> Result<u32> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let (actual, created_at) = Self::current_relationship_state_in(&tx, rel.id)?;
        if actual != expected_version {
            return Err(GraphError::RevisionConflict {
                entity: format!("relationship {}", rel.id),
                expected: expected_version,
                actual,
            });
        }
        if rel.created_at != created_at {
            return Err(GraphError::ImmutableFieldChange {
                entity: format!("relationship {}", rel.id),
                field: "created_at",
            });
        }
        let next_version =
            actual
                .checked_add(1)
                .ok_or_else(|| GraphError::NonMonotonicRevision {
                    entity: format!("relationship {}", rel.id),
                    expected_next: u32::MAX,
                    proposed: actual,
                })?;
        let changed = tx.execute(
            "UPDATE relationships SET \
                source = ?2, target = ?3, kind = ?4, strength = ?5, scope_json = ?6, \
                evidence_json = ?7, lifecycle = ?8 \
             WHERE id = ?1",
            params![
                rel.id.0.to_string(),
                rel.source.0.to_string(),
                rel.target.0.to_string(),
                rel.kind,
                rel.strength,
                serde_json::to_string(&rel.scope)?,
                serde_json::to_string(&rel.evidence)?,
                serde_json::to_string(&rel.lifecycle)?,
            ],
        )?;
        if changed == 0 {
            return Err(GraphError::NotFound(format!("relationship {}", rel.id)));
        }
        let committed = tx.query_row(
            "SELECT id, source, target, kind, strength, scope_json, evidence_json,
                    lifecycle, created_at
             FROM relationships WHERE id = ?1",
            params![rel.id.0.to_string()],
            Self::relationship_from_row,
        )??;
        Self::insert_relationship_snapshot(&tx, &committed, next_version)?;
        tx.commit()?;
        Ok(next_version)
    }

    /// Returns a relationship snapshot at an exact revision.
    pub fn get_relationship_version(
        &self,
        id: RelationshipId,
        version: u32,
    ) -> Result<Option<Relationship>> {
        self.conn
            .query_row(
                "SELECT id, source, target, kind, strength, scope_json, evidence_json,
                        lifecycle, created_at
                 FROM relationship_versions WHERE id = ?1 AND version = ?2",
                params![id.0.to_string(), version],
                Self::relationship_from_row,
            )
            .optional()?
            .transpose()
    }

    /// Lists relationship snapshots oldest first.
    pub fn list_relationship_versions(&self, id: RelationshipId) -> Result<Vec<Relationship>> {
        let mut statement = self.conn.prepare(
            "SELECT id, source, target, kind, strength, scope_json, evidence_json,
                    lifecycle, created_at
             FROM relationship_versions WHERE id = ?1 ORDER BY version ASC",
        )?;
        let rows = statement.query_map(params![id.0.to_string()], Self::relationship_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    pub fn current_relationship_version(&self, id: RelationshipId) -> Result<u32> {
        Self::current_relationship_version_in(&self.conn, id)
    }

    fn current_relationship_version_in(conn: &Connection, id: RelationshipId) -> Result<u32> {
        Self::current_relationship_state_in(conn, id).map(|(version, _)| version)
    }

    fn current_relationship_state_in(conn: &Connection, id: RelationshipId) -> Result<(u32, i64)> {
        conn.query_row(
            "SELECT MAX(versions.version), current.created_at
             FROM relationships AS current
             JOIN relationship_versions AS versions ON versions.id = current.id
             WHERE current.id = ?1
             GROUP BY current.id, current.created_at",
            params![id.0.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| GraphError::NotFound(format!("relationship {id}")))
    }

    fn insert_relationship_snapshot(
        conn: &Connection,
        rel: &Relationship,
        version: u32,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO relationship_versions
                (id, version, source, target, kind, strength, scope_json, evidence_json,
                 lifecycle, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                rel.id.0.to_string(),
                version,
                rel.source.0.to_string(),
                rel.target.0.to_string(),
                rel.kind,
                rel.strength,
                serde_json::to_string(&rel.scope)?,
                serde_json::to_string(&rel.evidence)?,
                serde_json::to_string(&rel.lifecycle)?,
                rel.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn delete_relationship(&self, id: RelationshipId) -> Result<()> {
        let changed = self.conn.execute(
            "DELETE FROM relationships WHERE id = ?1",
            params![id.0.to_string()],
        )?;
        if changed == 0 {
            return Err(GraphError::NotFound(format!("relationship {id}")));
        }
        Ok(())
    }

    fn relationship_from_row(row: &Row) -> rusqlite::Result<Result<Relationship>> {
        Ok((|| -> Result<Relationship> {
            let id: String = row.get(0)?;
            let source: String = row.get(1)?;
            let target: String = row.get(2)?;
            let kind: String = row.get(3)?;
            let strength: f64 = row.get(4)?;
            let scope_json: String = row.get(5)?;
            let evidence_json: String = row.get(6)?;
            let lifecycle_json: String = row.get(7)?;
            let created_at: i64 = row.get(8)?;

            Ok(Relationship {
                id: RelationshipId(Uuid::parse_str(&id)?),
                source: ConceptId(Uuid::parse_str(&source)?),
                target: ConceptId(Uuid::parse_str(&target)?),
                kind,
                strength,
                scope: serde_json::from_str::<Vec<ScopeCondition>>(&scope_json)?,
                evidence: serde_json::from_str(&evidence_json)?,
                lifecycle: serde_json::from_str::<Lifecycle>(&lifecycle_json)?,
                created_at,
            })
        })())
    }

    // ---------------------------------------------------------------
    // Procedures
    // ---------------------------------------------------------------

    pub fn insert_procedure(&self, proc: &Procedure) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO procedures \
                (id, name, params_json, body_json, contract_json, test_cases_json, \
                 concept_id, version, lifecycle, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                proc.id.0.to_string(),
                proc.name,
                serde_json::to_string(&proc.params)?,
                serde_json::to_string(&proc.body)?,
                serde_json::to_string(&proc.contract)?,
                serde_json::to_string(&proc.test_cases)?,
                proc.concept.map(|c| c.0.to_string()),
                proc.version,
                serde_json::to_string(&proc.lifecycle)?,
                proc.created_at,
                proc.updated_at,
            ],
        )?;
        Self::insert_procedure_snapshot(&tx, proc)?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_procedure(&self, id: ProcedureId) -> Result<Option<Procedure>> {
        self.conn
            .query_row(
                "SELECT id, name, params_json, body_json, contract_json, test_cases_json, \
                        concept_id, version, lifecycle, created_at, updated_at \
                 FROM procedures WHERE id = ?1",
                params![id.0.to_string()],
                Self::procedure_from_row,
            )
            .optional()?
            .transpose()
    }

    pub fn get_procedure_by_name(&self, name: &str) -> Result<Option<Procedure>> {
        self.conn
            .query_row(
                "SELECT id, name, params_json, body_json, contract_json, test_cases_json, \
                        concept_id, version, lifecycle, created_at, updated_at \
                 FROM procedures WHERE name = ?1",
                params![name],
                Self::procedure_from_row,
            )
            .optional()?
            .transpose()
    }

    pub fn update_procedure(&self, proc: &Procedure) -> Result<()> {
        let expected_version =
            proc.version
                .checked_sub(1)
                .ok_or_else(|| GraphError::NonMonotonicRevision {
                    entity: format!("procedure {}", proc.id),
                    expected_next: 1,
                    proposed: proc.version,
                })?;
        self.revise_procedure(proc, expected_version)
    }

    /// Atomically advances a procedure by exactly one version when the caller
    /// still holds the current version.
    pub fn revise_procedure(&self, proc: &Procedure, expected_version: u32) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let (actual, created_at) = tx
            .query_row(
                "SELECT version, created_at FROM procedures WHERE id = ?1",
                params![proc.id.0.to_string()],
                |row| Ok((row.get::<_, u32>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?
            .ok_or_else(|| GraphError::NotFound(format!("procedure {}", proc.id)))?;
        if actual != expected_version {
            return Err(GraphError::RevisionConflict {
                entity: format!("procedure {}", proc.id),
                expected: expected_version,
                actual,
            });
        }
        if proc.created_at != created_at {
            return Err(GraphError::ImmutableFieldChange {
                entity: format!("procedure {}", proc.id),
                field: "created_at",
            });
        }
        let expected_next =
            actual
                .checked_add(1)
                .ok_or_else(|| GraphError::NonMonotonicRevision {
                    entity: format!("procedure {}", proc.id),
                    expected_next: u32::MAX,
                    proposed: proc.version,
                })?;
        if proc.version != expected_next {
            return Err(GraphError::NonMonotonicRevision {
                entity: format!("procedure {}", proc.id),
                expected_next,
                proposed: proc.version,
            });
        }
        let changed = tx.execute(
            "UPDATE procedures SET \
                name = ?2, params_json = ?3, body_json = ?4, contract_json = ?5, \
                test_cases_json = ?6, concept_id = ?7, version = ?8, lifecycle = ?9, \
                updated_at = ?10 \
             WHERE id = ?1 AND version = ?11",
            params![
                proc.id.0.to_string(),
                proc.name,
                serde_json::to_string(&proc.params)?,
                serde_json::to_string(&proc.body)?,
                serde_json::to_string(&proc.contract)?,
                serde_json::to_string(&proc.test_cases)?,
                proc.concept.map(|c| c.0.to_string()),
                proc.version,
                serde_json::to_string(&proc.lifecycle)?,
                proc.updated_at,
                expected_version,
            ],
        )?;
        if changed == 0 {
            return Err(GraphError::RevisionConflict {
                entity: format!("procedure {}", proc.id),
                expected: expected_version,
                actual,
            });
        }
        let committed = tx.query_row(
            "SELECT id, name, params_json, body_json, contract_json, test_cases_json,
                    concept_id, version, lifecycle, created_at, updated_at
             FROM procedures WHERE id = ?1",
            params![proc.id.0.to_string()],
            Self::procedure_from_row,
        )??;
        Self::insert_procedure_snapshot(&tx, &committed)?;
        tx.commit()?;
        Ok(())
    }

    pub fn current_procedure_version(&self, id: ProcedureId) -> Result<u32> {
        self.conn
            .query_row(
                "SELECT version FROM procedures WHERE id = ?1",
                params![id.0.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| GraphError::NotFound(format!("procedure {id}")))
    }

    /// Deletes only the current procedure. Historical snapshots remain
    /// available for replay, auditing, and provenance.
    pub fn delete_procedure(&self, id: ProcedureId) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let current = tx
            .query_row(
                "SELECT id, name, params_json, body_json, contract_json, test_cases_json,
                        concept_id, version, lifecycle, created_at, updated_at
                 FROM procedures WHERE id = ?1",
                params![id.0.to_string()],
                Self::procedure_from_row,
            )
            .optional()?
            .transpose()?
            .ok_or_else(|| GraphError::NotFound(format!("procedure {id}")))?;
        if current.lifecycle == Lifecycle::Retired {
            return Err(GraphError::InvalidChangeSet(format!(
                "procedure {id} is already retired"
            )));
        }
        let callers = Self::list_procedures_in(&tx)?
            .into_iter()
            .filter(|procedure| {
                procedure.id != id
                    && Self::lifecycle_is_usable(procedure.lifecycle)
                    && Self::procedure_calls(procedure, id)
            })
            .map(|procedure| procedure.id)
            .collect::<Vec<_>>();
        if !callers.is_empty() {
            return Err(GraphError::HasDependents(format!(
                "procedure {id} has live caller(s): {}",
                callers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let next_version =
            current
                .version
                .checked_add(1)
                .ok_or_else(|| GraphError::NonMonotonicRevision {
                    entity: format!("procedure {id}"),
                    expected_next: u32::MAX,
                    proposed: current.version,
                })?;
        let updated_at = now_unix().max(current.updated_at.saturating_add(1));
        let changed = tx.execute(
            "UPDATE procedures
             SET version = ?2, lifecycle = ?3, updated_at = ?4
             WHERE id = ?1 AND version = ?5",
            params![
                id.0.to_string(),
                next_version,
                serde_json::to_string(&Lifecycle::Retired)?,
                updated_at,
                current.version,
            ],
        )?;
        if changed != 1 {
            return Err(GraphError::RevisionConflict {
                entity: format!("procedure {id}"),
                expected: current.version,
                actual: Self::current_procedure_version_in(&tx, id)?,
            });
        }
        let retired = tx.query_row(
            "SELECT id, name, params_json, body_json, contract_json, test_cases_json,
                    concept_id, version, lifecycle, created_at, updated_at
             FROM procedures WHERE id = ?1",
            params![id.0.to_string()],
            Self::procedure_from_row,
        )??;
        Self::insert_procedure_snapshot(&tx, &retired)?;
        tx.commit()?;
        Ok(())
    }

    /// Returns a historical procedure snapshot at an exact version.
    pub fn get_procedure_version(
        &self,
        id: ProcedureId,
        version: u32,
    ) -> Result<Option<Procedure>> {
        self.conn
            .query_row(
                "SELECT id, name, params_json, body_json, contract_json, test_cases_json, \
                        concept_id, version, lifecycle, created_at, updated_at \
                 FROM procedure_versions WHERE id = ?1 AND version = ?2",
                params![id.0.to_string(), version],
                Self::procedure_from_row,
            )
            .optional()?
            .transpose()
    }

    /// Lists all historical snapshots for a procedure, oldest version first.
    pub fn list_procedure_versions(&self, id: ProcedureId) -> Result<Vec<Procedure>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, params_json, body_json, contract_json, test_cases_json, \
                    concept_id, version, lifecycle, created_at, updated_at \
             FROM procedure_versions WHERE id = ?1 ORDER BY version ASC",
        )?;
        let rows = stmt.query_map(params![id.0.to_string()], Self::procedure_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    /// Returns the contract that belonged to a procedure at an exact version.
    pub fn get_contract_version(&self, id: ProcedureId, version: u32) -> Result<Option<Contract>> {
        let contract_json = self
            .conn
            .query_row(
                "SELECT contract_json FROM procedure_versions \
                 WHERE id = ?1 AND version = ?2",
                params![id.0.to_string(), version],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        contract_json
            .map(|json| serde_json::from_str(&json).map_err(GraphError::from))
            .transpose()
    }

    /// Lists contract snapshots with their procedure versions, oldest first.
    pub fn list_contract_versions(&self, id: ProcedureId) -> Result<Vec<(u32, Contract)>> {
        let mut stmt = self.conn.prepare(
            "SELECT version, contract_json FROM procedure_versions \
             WHERE id = ?1 ORDER BY version ASC",
        )?;
        let rows = stmt.query_map(params![id.0.to_string()], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|(version, json)| Ok((version, serde_json::from_str(&json)?)))
            .collect()
    }

    pub fn list_procedures(&self) -> Result<Vec<Procedure>> {
        Self::list_procedures_in(&self.conn)
    }

    fn list_procedures_in(conn: &Connection) -> Result<Vec<Procedure>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, params_json, body_json, contract_json, test_cases_json, \
             concept_id, version, lifecycle, created_at, updated_at \
             FROM procedures ORDER BY name",
        )?;
        let rows = stmt.query_map([], Self::procedure_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect()
    }

    fn insert_procedure_snapshot(conn: &Connection, proc: &Procedure) -> Result<()> {
        conn.execute(
            "INSERT INTO procedure_versions \
                (id, name, params_json, body_json, contract_json, test_cases_json, \
                 concept_id, version, lifecycle, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                proc.id.0.to_string(),
                proc.name,
                serde_json::to_string(&proc.params)?,
                serde_json::to_string(&proc.body)?,
                serde_json::to_string(&proc.contract)?,
                serde_json::to_string(&proc.test_cases)?,
                proc.concept.map(|c| c.0.to_string()),
                proc.version,
                serde_json::to_string(&proc.lifecycle)?,
                proc.created_at,
                proc.updated_at,
            ],
        )?;
        Ok(())
    }

    fn procedure_from_row(row: &Row) -> rusqlite::Result<Result<Procedure>> {
        Ok((|| -> Result<Procedure> {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let params_json: String = row.get(2)?;
            let body_json: String = row.get(3)?;
            let contract_json: String = row.get(4)?;
            let test_cases_json: String = row.get(5)?;
            let concept_id: Option<String> = row.get(6)?;
            let version: u32 = row.get(7)?;
            let lifecycle_json: String = row.get(8)?;
            let created_at: i64 = row.get(9)?;
            let updated_at: i64 = row.get(10)?;

            let concept = concept_id
                .map(|s| -> Result<ConceptId> { Ok(ConceptId(Uuid::parse_str(&s)?)) })
                .transpose()?;

            Ok(Procedure {
                id: ProcedureId(Uuid::parse_str(&id)?),
                name,
                params: serde_json::from_str::<Vec<Param>>(&params_json)?,
                body: serde_json::from_str(&body_json)?,
                contract: serde_json::from_str::<Contract>(&contract_json)?,
                test_cases: serde_json::from_str::<Vec<TestCase>>(&test_cases_json)?,
                concept,
                version,
                lifecycle: serde_json::from_str::<Lifecycle>(&lifecycle_json)?,
                created_at,
                updated_at,
            })
        })())
    }

    // ---------------------------------------------------------------
    // Atomic lifecycle change sets
    // ---------------------------------------------------------------

    /// Applies a validated persistence-only lifecycle change set atomically.
    ///
    /// Domain policy remains the caller's responsibility. This boundary
    /// guarantees expected-version checks, immutable snapshots, and an audit
    /// receipt are committed together. Retrying the identical request returns
    /// the original receipt without applying another revision.
    pub fn apply_lifecycle_change_set(
        &self,
        change_set: &LifecycleChangeSet,
    ) -> Result<LifecycleChangeReceipt> {
        if change_set.idempotency_key.trim().is_empty() {
            return Err(GraphError::InvalidChangeSet(
                "idempotency key must be non-empty".into(),
            ));
        }
        let request_json = serde_json::to_string(change_set)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if let Some((stored_request, stored_receipt)) = tx
            .query_row(
                "SELECT request_json, receipt_json FROM graph_change_receipts
                 WHERE idempotency_key = ?1",
                params![change_set.idempotency_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if stored_request != request_json {
                return Err(GraphError::IdempotencyConflict {
                    key: change_set.idempotency_key.clone(),
                });
            }
            return Ok(serde_json::from_str(&stored_receipt)?);
        }
        let mut targets = HashSet::new();
        let mut prepared = Vec::with_capacity(change_set.changes.len());
        for change in &change_set.changes {
            if !targets.insert(change.target()) {
                return Err(GraphError::InvalidChangeSet(format!(
                    "duplicate lifecycle target {:?}",
                    change.target()
                )));
            }
            prepared.push(Self::prepare_lifecycle_change(&tx, change)?);
        }

        let mut applied = Vec::with_capacity(prepared.len());
        for change in prepared {
            applied.push(Self::apply_prepared_lifecycle_change(
                &tx,
                change,
                change_set.updated_at,
            )?);
        }
        let receipt = LifecycleChangeReceipt {
            idempotency_key: change_set.idempotency_key.clone(),
            updated_at: change_set.updated_at,
            changes: applied,
        };
        let receipt_json = serde_json::to_string(&receipt)?;
        tx.execute(
            "INSERT INTO graph_change_receipts
                (idempotency_key, request_json, receipt_json, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                change_set.idempotency_key,
                request_json,
                receipt_json,
                change_set.updated_at,
            ],
        )?;
        tx.commit()?;
        Ok(receipt)
    }

    /// Looks up the immutable audit receipt for an applied change set.
    pub fn get_change_receipt(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<LifecycleChangeReceipt>> {
        self.conn
            .query_row(
                "SELECT receipt_json FROM graph_change_receipts WHERE idempotency_key = ?1",
                params![idempotency_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|json| serde_json::from_str(&json).map_err(GraphError::from))
            .transpose()
    }

    /// Looks up a receipt only when it belongs to this exact change set.
    ///
    /// Recovery code should prefer this over key-only lookup so a reused key
    /// can never make a different staged plan appear complete.
    pub fn get_change_set_receipt(
        &self,
        change_set: &LifecycleChangeSet,
    ) -> Result<Option<LifecycleChangeReceipt>> {
        let request_json = serde_json::to_string(change_set)?;
        let stored = self
            .conn
            .query_row(
                "SELECT request_json, receipt_json FROM graph_change_receipts
                 WHERE idempotency_key = ?1",
                params![change_set.idempotency_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((stored_request, stored_receipt)) = stored else {
            return Ok(None);
        };
        if stored_request != request_json {
            return Err(GraphError::IdempotencyConflict {
                key: change_set.idempotency_key.clone(),
            });
        }
        Ok(Some(serde_json::from_str(&stored_receipt)?))
    }

    fn prepare_lifecycle_change(
        conn: &Connection,
        change: &LifecycleChange,
    ) -> Result<PreparedLifecycleChange> {
        match change {
            LifecycleChange::Concept {
                id,
                expected_version,
                lifecycle,
            } => {
                let value = conn
                    .query_row(
                        "SELECT id, name, description, mutability, confidence_json, lifecycle,
                                created_at, updated_at
                         FROM concepts WHERE id = ?1",
                        params![id.0.to_string()],
                        Self::concept_from_row,
                    )
                    .optional()?
                    .transpose()?
                    .ok_or_else(|| GraphError::NotFound(format!("concept {id}")))?;
                let actual = Self::current_concept_version_in(conn, *id)?;
                Self::ensure_batch_version(format!("concept {id}"), *expected_version, actual)?;
                if value.lifecycle == *lifecycle {
                    return Err(GraphError::InvalidChangeSet(format!(
                        "concept {id} already has lifecycle {lifecycle:?}"
                    )));
                }
                Ok(PreparedLifecycleChange::Concept {
                    id: *id,
                    previous_version: actual,
                    current_version: Self::next_batch_version(format!("concept {id}"), actual)?,
                    previous_lifecycle: value.lifecycle,
                    current_lifecycle: *lifecycle,
                })
            }
            LifecycleChange::Procedure {
                id,
                expected_version,
                lifecycle,
            } => {
                let value = conn
                    .query_row(
                        "SELECT id, name, params_json, body_json, contract_json, test_cases_json,
                                concept_id, version, lifecycle, created_at, updated_at
                         FROM procedures WHERE id = ?1",
                        params![id.0.to_string()],
                        Self::procedure_from_row,
                    )
                    .optional()?
                    .transpose()?
                    .ok_or_else(|| GraphError::NotFound(format!("procedure {id}")))?;
                Self::ensure_batch_version(
                    format!("procedure {id}"),
                    *expected_version,
                    value.version,
                )?;
                if value.lifecycle == *lifecycle {
                    return Err(GraphError::InvalidChangeSet(format!(
                        "procedure {id} already has lifecycle {lifecycle:?}"
                    )));
                }
                Ok(PreparedLifecycleChange::Procedure {
                    id: *id,
                    previous_version: value.version,
                    current_version: Self::next_batch_version(
                        format!("procedure {id}"),
                        value.version,
                    )?,
                    previous_lifecycle: value.lifecycle,
                    current_lifecycle: *lifecycle,
                })
            }
            LifecycleChange::Relationship {
                id,
                expected_version,
                lifecycle,
            } => {
                let value = conn
                    .query_row(
                        "SELECT id, source, target, kind, strength, scope_json, evidence_json,
                                lifecycle, created_at
                         FROM relationships WHERE id = ?1",
                        params![id.0.to_string()],
                        Self::relationship_from_row,
                    )
                    .optional()?
                    .transpose()?
                    .ok_or_else(|| GraphError::NotFound(format!("relationship {id}")))?;
                let actual = Self::current_relationship_version_in(conn, *id)?;
                Self::ensure_batch_version(
                    format!("relationship {id}"),
                    *expected_version,
                    actual,
                )?;
                if value.lifecycle == *lifecycle {
                    return Err(GraphError::InvalidChangeSet(format!(
                        "relationship {id} already has lifecycle {lifecycle:?}"
                    )));
                }
                Ok(PreparedLifecycleChange::Relationship {
                    id: *id,
                    previous_version: actual,
                    current_version: Self::next_batch_version(
                        format!("relationship {id}"),
                        actual,
                    )?,
                    previous_lifecycle: value.lifecycle,
                    current_lifecycle: *lifecycle,
                })
            }
        }
    }

    fn apply_prepared_lifecycle_change(
        conn: &Connection,
        change: PreparedLifecycleChange,
        updated_at: i64,
    ) -> Result<AppliedLifecycleChange> {
        match change {
            PreparedLifecycleChange::Concept {
                id,
                previous_version,
                current_version,
                previous_lifecycle,
                current_lifecycle,
            } => {
                let changed = conn.execute(
                    "UPDATE concepts SET lifecycle = ?2, updated_at = ?3 WHERE id = ?1",
                    params![
                        id.0.to_string(),
                        serde_json::to_string(&current_lifecycle)?,
                        updated_at,
                    ],
                )?;
                if changed != 1 {
                    return Err(GraphError::NotFound(format!("concept {id}")));
                }
                let committed = conn.query_row(
                    "SELECT id, name, description, mutability, confidence_json, lifecycle,
                            created_at, updated_at
                     FROM concepts WHERE id = ?1",
                    params![id.0.to_string()],
                    Self::concept_from_row,
                )??;
                Self::ensure_committed_lifecycle(
                    format!("concept {id}"),
                    current_lifecycle,
                    committed.lifecycle,
                )?;
                Self::insert_concept_snapshot(conn, &committed, current_version)?;
                Ok(AppliedLifecycleChange {
                    target: LifecycleTarget::Concept { id },
                    previous_version,
                    current_version,
                    previous_lifecycle,
                    current_lifecycle,
                })
            }
            PreparedLifecycleChange::Procedure {
                id,
                previous_version,
                current_version,
                previous_lifecycle,
                current_lifecycle,
            } => {
                let changed = conn.execute(
                    "UPDATE procedures
                     SET version = ?2, lifecycle = ?3, updated_at = ?4
                     WHERE id = ?1 AND version = ?5",
                    params![
                        id.0.to_string(),
                        current_version,
                        serde_json::to_string(&current_lifecycle)?,
                        updated_at,
                        previous_version,
                    ],
                )?;
                if changed != 1 {
                    return Err(GraphError::RevisionConflict {
                        entity: format!("procedure {id}"),
                        expected: previous_version,
                        actual: Self::current_procedure_version_in(conn, id)?,
                    });
                }
                let committed = conn.query_row(
                    "SELECT id, name, params_json, body_json, contract_json, test_cases_json,
                            concept_id, version, lifecycle, created_at, updated_at
                     FROM procedures WHERE id = ?1",
                    params![id.0.to_string()],
                    Self::procedure_from_row,
                )??;
                Self::ensure_committed_lifecycle(
                    format!("procedure {id}"),
                    current_lifecycle,
                    committed.lifecycle,
                )?;
                Self::insert_procedure_snapshot(conn, &committed)?;
                Ok(AppliedLifecycleChange {
                    target: LifecycleTarget::Procedure { id },
                    previous_version,
                    current_version,
                    previous_lifecycle,
                    current_lifecycle,
                })
            }
            PreparedLifecycleChange::Relationship {
                id,
                previous_version,
                current_version,
                previous_lifecycle,
                current_lifecycle,
            } => {
                let changed = conn.execute(
                    "UPDATE relationships SET lifecycle = ?2 WHERE id = ?1",
                    params![id.0.to_string(), serde_json::to_string(&current_lifecycle)?,],
                )?;
                if changed != 1 {
                    return Err(GraphError::NotFound(format!("relationship {id}")));
                }
                let committed = conn.query_row(
                    "SELECT id, source, target, kind, strength, scope_json, evidence_json,
                            lifecycle, created_at
                     FROM relationships WHERE id = ?1",
                    params![id.0.to_string()],
                    Self::relationship_from_row,
                )??;
                Self::ensure_committed_lifecycle(
                    format!("relationship {id}"),
                    current_lifecycle,
                    committed.lifecycle,
                )?;
                Self::insert_relationship_snapshot(conn, &committed, current_version)?;
                Ok(AppliedLifecycleChange {
                    target: LifecycleTarget::Relationship { id },
                    previous_version,
                    current_version,
                    previous_lifecycle,
                    current_lifecycle,
                })
            }
        }
    }

    fn ensure_committed_lifecycle(
        entity: String,
        requested: Lifecycle,
        committed: Lifecycle,
    ) -> Result<()> {
        if requested == committed {
            Ok(())
        } else {
            Err(GraphError::InvalidChangeSet(format!(
                "{entity} lifecycle update was not committed as requested"
            )))
        }
    }

    fn ensure_batch_version(entity: String, expected: u32, actual: u32) -> Result<()> {
        if expected == actual {
            Ok(())
        } else {
            Err(GraphError::RevisionConflict {
                entity,
                expected,
                actual,
            })
        }
    }

    fn next_batch_version(entity: String, actual: u32) -> Result<u32> {
        actual
            .checked_add(1)
            .ok_or(GraphError::NonMonotonicRevision {
                entity,
                expected_next: u32::MAX,
                proposed: actual,
            })
    }

    fn current_procedure_version_in(conn: &Connection, id: ProcedureId) -> Result<u32> {
        conn.query_row(
            "SELECT version FROM procedures WHERE id = ?1",
            params![id.0.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| GraphError::NotFound(format!("procedure {id}")))
    }

    // ---------------------------------------------------------------
    // Graph traversal
    // ---------------------------------------------------------------

    /// Walks relationships of the given `kind` starting from `start`,
    /// up to `max_hops` hops away, using a recursive CTE. Returns each
    /// reached concept paired with the minimum number of hops needed to
    /// reach it.
    pub fn traverse(
        &self,
        start: ConceptId,
        kind: &str,
        max_hops: u32,
    ) -> Result<Vec<(ConceptId, u32)>> {
        if max_hops == 0 {
            return Ok(Vec::new());
        }

        let active = serde_json::to_string(&Lifecycle::Active)?;
        let validated = serde_json::to_string(&Lifecycle::Validated)?;
        let provisional = serde_json::to_string(&Lifecycle::Provisional)?;
        let under_review = serde_json::to_string(&Lifecycle::UnderReview)?;
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE walk(id, hops) AS ( \
                SELECT r.target, 1 FROM relationships r \
                JOIN concepts origin ON origin.id = r.source \
                JOIN concepts reached ON reached.id = r.target \
                WHERE r.source = ?1 AND r.kind = ?2 \
                  AND r.lifecycle IN (?4, ?5, ?6, ?7) \
                  AND origin.lifecycle IN (?4, ?5, ?6, ?7) \
                  AND reached.lifecycle IN (?4, ?5, ?6, ?7) \
                UNION ALL \
                SELECT r.target, w.hops + 1 \
                FROM relationships r JOIN walk w ON r.source = w.id \
                JOIN concepts reached ON reached.id = r.target \
                WHERE r.kind = ?2 AND w.hops < ?3 \
                  AND r.lifecycle IN (?4, ?5, ?6, ?7) \
                  AND reached.lifecycle IN (?4, ?5, ?6, ?7) \
             ) \
             SELECT id, MIN(hops) AS hops FROM walk GROUP BY id ORDER BY hops",
        )?;
        let rows = stmt.query_map(
            params![
                start.0.to_string(),
                kind,
                max_hops as i64,
                active,
                validated,
                provisional,
                under_review,
            ],
            |row| {
                let id: String = row.get(0)?;
                let hops: i64 = row.get(1)?;
                Ok((id, hops))
            },
        )?;

        let mut out = Vec::new();
        for row in rows {
            let (id, hops) = row?;
            out.push((ConceptId(Uuid::parse_str(&id)?), hops as u32));
        }
        Ok(out)
    }

    /// Finds concepts that depend on `concept_id`: the sources of every
    /// relationship that targets it. In a relationship `A --kind--> B`,
    /// `A` depends on `B`, so `get_dependents(B)` returns `A`.
    pub fn get_dependents(&self, concept_id: ConceptId) -> Result<Vec<ConceptId>> {
        let mut dependents = Vec::new();
        for relationship in self.get_relationships_to(concept_id)? {
            if !Self::lifecycle_is_usable(relationship.lifecycle) {
                continue;
            }
            if matches!(
                Self::relationship_dependency_direction(&relationship.kind),
                RelationshipDependencyDirection::SourceDependsOnTarget
                    | RelationshipDependencyDirection::Bidirectional
            ) && self
                .get_concept(relationship.source)?
                .is_some_and(|concept| Self::lifecycle_is_usable(concept.lifecycle))
            {
                dependents.push(relationship.source);
            }
        }
        for relationship in self.get_relationships_from(concept_id)? {
            if !Self::lifecycle_is_usable(relationship.lifecycle) {
                continue;
            }
            if matches!(
                Self::relationship_dependency_direction(&relationship.kind),
                RelationshipDependencyDirection::TargetDependsOnSource
                    | RelationshipDependencyDirection::Bidirectional
            ) && self
                .get_concept(relationship.target)?
                .is_some_and(|concept| Self::lifecycle_is_usable(concept.lifecycle))
            {
                dependents.push(relationship.target);
            }
        }
        dependents.sort_by_key(|id| id.0);
        dependents.dedup();
        Ok(dependents)
    }

    /// Reports current graph entities that depend on a concept or procedure.
    ///
    /// A concept is depended on by relationship sources and by procedures
    /// attached to it. A procedure is depended on by procedures whose bodies
    /// call it, including calls nested inside other expressions.
    pub fn get_dependency_report(&self, target: DependencyTarget) -> Result<DependencyReport> {
        let (dependents, relationships) = match target {
            DependencyTarget::Concept(concept_id) => {
                let mut dependents = self
                    .get_dependents(concept_id)?
                    .into_iter()
                    .map(Dependent::Concept)
                    .collect::<Vec<_>>();
                for procedure in self.list_procedures()?.into_iter().filter(|procedure| {
                    procedure.concept == Some(concept_id)
                        && Self::lifecycle_is_usable(procedure.lifecycle)
                }) {
                    dependents.push(Dependent::Procedure {
                        id: procedure.id,
                        kind: ProcedureDependencyKind::AttachedToConcept,
                    });
                }
                let mut relationships = self
                    .get_relationships_from(concept_id)?
                    .into_iter()
                    .chain(self.get_relationships_to(concept_id)?)
                    .filter(|relationship| Self::lifecycle_is_usable(relationship.lifecycle))
                    .map(|relationship| {
                        Ok(RelationshipDependency {
                            relationship_id: relationship.id,
                            version: self.current_relationship_version(relationship.id)?,
                            source: relationship.source,
                            target: relationship.target,
                            direction: Self::relationship_dependency_direction(&relationship.kind),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                relationships.sort_by_key(|relationship| relationship.relationship_id.0);
                relationships.dedup_by_key(|relationship| relationship.relationship_id);
                (dependents, relationships)
            }
            DependencyTarget::Procedure(procedure_id) => (
                self.list_procedures()?
                    .into_iter()
                    .filter(|procedure| {
                        Self::lifecycle_is_usable(procedure.lifecycle)
                            && Self::procedure_calls(procedure, procedure_id)
                    })
                    .map(|procedure| Dependent::Procedure {
                        id: procedure.id,
                        kind: ProcedureDependencyKind::CallsProcedure,
                    })
                    .collect(),
                Vec::new(),
            ),
        };

        Ok(DependencyReport {
            target,
            dependents,
            relationships,
        })
    }

    fn lifecycle_is_usable(lifecycle: Lifecycle) -> bool {
        !matches!(
            lifecycle,
            Lifecycle::Stale | Lifecycle::Superseded | Lifecycle::Retired | Lifecycle::Invalid
        )
    }

    fn relationship_dependency_direction(kind: &str) -> RelationshipDependencyDirection {
        match kind {
            "inverse-of" => RelationshipDependencyDirection::Bidirectional,
            "implements" | "tests" | "contained-by" => {
                RelationshipDependencyDirection::TargetDependsOnSource
            }
            "depends-on" | "is-a" | "has" | "implemented-by" | "tested-by" | "special-case-of" => {
                RelationshipDependencyDirection::SourceDependsOnTarget
            }
            value if value.starts_with("alternative-support:") => {
                RelationshipDependencyDirection::SourceDependsOnTarget
            }
            _ => RelationshipDependencyDirection::Unknown,
        }
    }

    fn procedure_calls(procedure: &Procedure, target: ProcedureId) -> bool {
        let mut calls = HashSet::new();
        Self::collect_expression_calls(&procedure.body, &mut calls);
        for condition in procedure
            .contract
            .requires
            .iter()
            .chain(&procedure.contract.promises)
            .chain(&procedure.contract.fails_when)
        {
            if let Some(check) = &condition.check {
                Self::collect_expression_calls(check, &mut calls);
            }
        }
        calls.contains(&target)
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use spoon_core::{Condition, Expr, Value};

    fn concept(name: &str) -> Concept {
        Concept::new(name, MutabilityClass::DefeasibleGeneral)
    }

    fn provisional_bundle() -> (Concept, Procedure) {
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
        (concept, procedure)
    }

    #[test]
    fn provisional_knowledge_bundle_is_atomic_idempotent_and_snapshotted() {
        let store = KnowledgeStore::in_memory().unwrap();
        let (concept, procedure) = provisional_bundle();
        store
            .insert_knowledge_bundle(
                "teacher-double",
                std::slice::from_ref(&concept),
                &[],
                std::slice::from_ref(&procedure),
            )
            .unwrap();
        store
            .insert_knowledge_bundle(
                "teacher-double",
                std::slice::from_ref(&concept),
                &[],
                std::slice::from_ref(&procedure),
            )
            .unwrap();
        let mut recovered_concept = concept.clone();
        recovered_concept.created_at += 10;
        recovered_concept.updated_at += 10;
        let mut recovered_procedure = procedure.clone();
        recovered_procedure.created_at += 10;
        recovered_procedure.updated_at += 10;
        store
            .insert_knowledge_bundle(
                "teacher-double",
                std::slice::from_ref(&recovered_concept),
                &[],
                std::slice::from_ref(&recovered_procedure),
            )
            .unwrap();

        assert_eq!(store.list_concepts().unwrap().len(), 1);
        assert_eq!(store.list_procedures().unwrap().len(), 1);
        assert_eq!(store.list_concept_versions(concept.id).unwrap().len(), 1);
        assert_eq!(
            store.list_procedure_versions(procedure.id).unwrap().len(),
            1
        );

        let mut conflicting = procedure.clone();
        conflicting.name = "NOT_DOUBLE".into();
        assert!(matches!(
            store.insert_knowledge_bundle(
                "teacher-double",
                std::slice::from_ref(&concept),
                &[],
                std::slice::from_ref(&conflicting),
            ),
            Err(GraphError::IdempotencyConflict { .. })
        ));
    }

    #[test]
    fn provisional_knowledge_bundle_rolls_back_every_entity_on_failure() {
        let store = KnowledgeStore::in_memory().unwrap();
        let (concept, procedure) = provisional_bundle();
        assert!(
            store
                .insert_knowledge_bundle_in(
                    "teacher-double-failpoint",
                    std::slice::from_ref(&concept),
                    &[],
                    std::slice::from_ref(&procedure),
                    Some(1),
                )
                .is_err()
        );
        assert!(store.list_concepts().unwrap().is_empty());
        assert!(store.list_procedures().unwrap().is_empty());
        assert!(
            store
                .insert_knowledge_bundle(
                    "teacher-double-failpoint",
                    std::slice::from_ref(&concept),
                    &[],
                    std::slice::from_ref(&procedure),
                )
                .is_ok()
        );
    }

    #[test]
    fn provisional_knowledge_bundle_receipt_survives_reopen_and_recovery_metadata() {
        let path = std::env::temp_dir().join(format!(
            "spoon-knowledge-bundle-reopen-{}.sqlite",
            Uuid::new_v4()
        ));
        let path_text = path.to_string_lossy().into_owned();
        let (concept, procedure) = provisional_bundle();
        {
            let store = KnowledgeStore::new(&path_text).unwrap();
            store
                .insert_knowledge_bundle(
                    "teacher-double-reopen",
                    std::slice::from_ref(&concept),
                    &[],
                    std::slice::from_ref(&procedure),
                )
                .unwrap();
        }
        let mut recovered_concept = concept.clone();
        recovered_concept.created_at += 42;
        recovered_concept.updated_at += 42;
        let mut recovered_procedure = procedure.clone();
        recovered_procedure.created_at += 42;
        recovered_procedure.updated_at += 42;
        {
            let store = KnowledgeStore::new(&path_text).unwrap();
            store
                .insert_knowledge_bundle(
                    "teacher-double-reopen",
                    std::slice::from_ref(&recovered_concept),
                    &[],
                    std::slice::from_ref(&recovered_procedure),
                )
                .unwrap();
            assert_eq!(store.list_concepts().unwrap().len(), 1);
            assert_eq!(store.list_procedures().unwrap().len(), 1);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn provisional_knowledge_bundle_rejects_authority_and_call_escape_hatches() {
        let store = KnowledgeStore::in_memory().unwrap();
        let (mut concept, mut procedure) = provisional_bundle();
        concept.lifecycle = Lifecycle::Validated;
        assert!(matches!(
            store.insert_knowledge_bundle(
                "teacher-escalated",
                std::slice::from_ref(&concept),
                &[],
                std::slice::from_ref(&procedure),
            ),
            Err(GraphError::InvalidKnowledgeBundle(_))
        ));

        concept.lifecycle = Lifecycle::Provisional;
        procedure.body = Expr::Call {
            procedure: ProcedureId::new(),
            args: vec![Expr::Var("x".into())],
        };
        assert!(matches!(
            store.insert_knowledge_bundle(
                "teacher-missing-call",
                std::slice::from_ref(&concept),
                &[],
                std::slice::from_ref(&procedure),
            ),
            Err(GraphError::InvalidKnowledgeBundle(_))
        ));
        assert!(store.list_concepts().unwrap().is_empty());
    }

    #[test]
    fn create_and_retrieve_concept() {
        let store = KnowledgeStore::in_memory().unwrap();
        let c = concept("dog").with_description("a canine");
        store.insert_concept(&c).unwrap();

        let fetched = store.get_concept(c.id).unwrap().expect("concept exists");
        assert_eq!(fetched.name, "dog");
        assert_eq!(fetched.description.as_deref(), Some("a canine"));
        assert_eq!(fetched.mutability, MutabilityClass::DefeasibleGeneral);
    }

    #[test]
    fn concept_by_name_lookup() {
        let store = KnowledgeStore::in_memory().unwrap();
        let c = concept("cat");
        store.insert_concept(&c).unwrap();

        let fetched = store
            .get_concept_by_name("cat")
            .unwrap()
            .expect("concept exists");
        assert_eq!(fetched.id, c.id);

        assert!(store.get_concept_by_name("nonexistent").unwrap().is_none());
    }

    #[test]
    fn update_concept_roundtrip() {
        let store = KnowledgeStore::in_memory().unwrap();
        let mut c = concept("bird");
        store.insert_concept(&c).unwrap();

        c.description = Some("flies".to_string());
        c.lifecycle = Lifecycle::Validated;
        store.update_concept(&c).unwrap();

        let fetched = store.get_concept(c.id).unwrap().unwrap();
        assert_eq!(fetched.description.as_deref(), Some("flies"));
        assert_eq!(fetched.lifecycle, Lifecycle::Validated);
    }

    #[test]
    fn concept_revisions_are_immutable_and_reject_stale_writers() {
        let store = KnowledgeStore::in_memory().unwrap();
        let mut concept = concept("bird");
        store.insert_concept(&concept).unwrap();

        concept.description = Some("usually flies".into());
        concept.updated_at += 1;
        assert_eq!(store.revise_concept(&concept, 1).unwrap(), 2);

        let original = store.get_concept_version(concept.id, 1).unwrap().unwrap();
        let revised = store.get_concept_version(concept.id, 2).unwrap().unwrap();
        assert_eq!(original.description, None);
        assert_eq!(revised.description.as_deref(), Some("usually flies"));
        assert!(matches!(
            store.revise_concept(&concept, 1),
            Err(GraphError::RevisionConflict {
                expected: 1,
                actual: 2,
                ..
            })
        ));
        assert_eq!(store.list_concept_versions(concept.id).unwrap().len(), 2);
    }

    #[test]
    fn concept_revision_rejects_created_at_drift_and_legacy_second_writes() {
        let store = KnowledgeStore::in_memory().unwrap();
        let mut concept = concept("bird");
        store.insert_concept(&concept).unwrap();

        let mut invalid = concept.clone();
        invalid.created_at += 1;
        assert!(matches!(
            store.revise_concept(&invalid, 1),
            Err(GraphError::ImmutableFieldChange {
                field: "created_at",
                ..
            })
        ));

        concept.description = Some("first revision".into());
        store.update_concept(&concept).unwrap();
        concept.description = Some("unsafe legacy overwrite".into());
        assert!(matches!(
            store.update_concept(&concept),
            Err(GraphError::ExpectedVersionRequired { .. })
        ));
    }

    #[test]
    fn list_concepts_returns_all() {
        let store = KnowledgeStore::in_memory().unwrap();
        store.insert_concept(&concept("a")).unwrap();
        store.insert_concept(&concept("b")).unwrap();

        let all = store.list_concepts().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn list_relationships_is_bounded_deterministic_and_read_only() {
        let store = KnowledgeStore::in_memory().unwrap();
        let source = concept("source");
        let target = concept("target");
        store.insert_concept(&source).unwrap();
        store.insert_concept(&target).unwrap();

        let mut first = Relationship::new(source.id, target.id, "supports");
        first.id = RelationshipId(Uuid::from_u128(1));
        let mut second = Relationship::new(source.id, target.id, "tests");
        second.id = RelationshipId(Uuid::from_u128(2));
        store.insert_relationship(&second).unwrap();
        store.insert_relationship(&first).unwrap();

        let limited = store.list_relationships(1).unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, first.id);
        assert!(store.list_relationships(0).unwrap().is_empty());
        let all = store
            .list_relationships(MAX_RELATIONSHIP_LIST_LIMIT + 1)
            .unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, first.id);
        assert_eq!(all[1].id, second.id);

        // A read collection must not alter either row or its version history.
        let fetched_first = store.get_relationship(first.id).unwrap().unwrap();
        assert_eq!(fetched_first.id, first.id);
        assert_eq!(fetched_first.kind, first.kind);
        assert_eq!(store.list_relationship_versions(first.id).unwrap().len(), 1);
        assert_eq!(
            store.get_relationship(second.id).unwrap().unwrap().id,
            second.id
        );
    }

    #[test]
    fn concepts_can_be_filtered_by_mutability_class() {
        let store = KnowledgeStore::in_memory().unwrap();
        let definitional = Concept::new("addition", MutabilityClass::Definitional);
        let procedural = Concept::new("bake", MutabilityClass::Procedural);
        let another_definitional = Concept::new("multiplication", MutabilityClass::Definitional);
        for concept in [&definitional, &procedural, &another_definitional] {
            store.insert_concept(concept).unwrap();
        }

        let matches = store
            .get_concepts_by_mutability(MutabilityClass::Definitional)
            .unwrap();

        assert_eq!(
            matches
                .iter()
                .map(|concept| concept.name.as_str())
                .collect::<Vec<_>>(),
            vec!["addition", "multiplication"]
        );
        assert!(
            store
                .get_concepts_by_mutability(MutabilityClass::Normative)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn create_relationship_between_concepts() {
        let store = KnowledgeStore::in_memory().unwrap();
        let a = concept("mammal");
        let b = concept("dog");
        store.insert_concept(&a).unwrap();
        store.insert_concept(&b).unwrap();

        let rel = Relationship::new(b.id, a.id, "is-a");
        store.insert_relationship(&rel).unwrap();

        let fetched = store.get_relationship(rel.id).unwrap().expect("exists");
        assert_eq!(fetched.source, b.id);
        assert_eq!(fetched.target, a.id);
        assert_eq!(fetched.kind, "is-a");

        let from_b = store.get_relationships_from(b.id).unwrap();
        assert_eq!(from_b.len(), 1);

        let to_a = store.get_relationships_to(a.id).unwrap();
        assert_eq!(to_a.len(), 1);

        let by_kind = store.get_relationships_by_kind("is-a").unwrap();
        assert_eq!(by_kind.len(), 1);
    }

    #[test]
    fn update_relationship_roundtrip() {
        let store = KnowledgeStore::in_memory().unwrap();
        let a = concept("a");
        let b = concept("b");
        store.insert_concept(&a).unwrap();
        store.insert_concept(&b).unwrap();

        let mut rel = Relationship::new(a.id, b.id, "possible-link");
        store.insert_relationship(&rel).unwrap();

        rel.kind = "confirmed-link".into();
        rel.strength = 0.95;
        rel.lifecycle = Lifecycle::Validated;
        store.update_relationship(&rel).unwrap();

        let updated = store.get_relationship(rel.id).unwrap().unwrap();
        assert_eq!(updated.kind, "confirmed-link");
        assert_eq!(updated.strength, 0.95);
        assert_eq!(updated.lifecycle, Lifecycle::Validated);
    }

    #[test]
    fn relationship_revisions_are_immutable_and_compare_and_swap() {
        let store = KnowledgeStore::in_memory().unwrap();
        let a = concept("a");
        let b = concept("b");
        store.insert_concept(&a).unwrap();
        store.insert_concept(&b).unwrap();
        let mut relationship = Relationship::new(a.id, b.id, "possible-link");
        store.insert_relationship(&relationship).unwrap();

        relationship.kind = "confirmed-link".into();
        relationship.strength = 0.9;
        assert_eq!(store.revise_relationship(&relationship, 1).unwrap(), 2);

        let original = store
            .get_relationship_version(relationship.id, 1)
            .unwrap()
            .unwrap();
        assert_eq!(original.kind, "possible-link");
        assert_eq!(
            store
                .get_relationship_version(relationship.id, 2)
                .unwrap()
                .unwrap()
                .kind,
            "confirmed-link"
        );
        assert!(matches!(
            store.revise_relationship(&relationship, 1),
            Err(GraphError::RevisionConflict {
                expected: 1,
                actual: 2,
                ..
            })
        ));
    }

    #[test]
    fn relationship_revision_rejects_created_at_drift() {
        let store = KnowledgeStore::in_memory().unwrap();
        let a = concept("a");
        let b = concept("b");
        store.insert_concept(&a).unwrap();
        store.insert_concept(&b).unwrap();
        let relationship = Relationship::new(a.id, b.id, "link");
        store.insert_relationship(&relationship).unwrap();
        let mut invalid = relationship.clone();
        invalid.created_at += 1;

        assert!(matches!(
            store.revise_relationship(&invalid, 1),
            Err(GraphError::ImmutableFieldChange {
                field: "created_at",
                ..
            })
        ));
        assert_eq!(
            store
                .get_relationship_version(relationship.id, 1)
                .unwrap()
                .unwrap()
                .created_at,
            relationship.created_at
        );
    }

    #[test]
    fn delete_relationship_removes_it() {
        let store = KnowledgeStore::in_memory().unwrap();
        let a = concept("a");
        let b = concept("b");
        store.insert_concept(&a).unwrap();
        store.insert_concept(&b).unwrap();
        let rel = Relationship::new(a.id, b.id, "link");
        store.insert_relationship(&rel).unwrap();

        store.delete_relationship(rel.id).unwrap();

        assert!(store.get_relationship(rel.id).unwrap().is_none());
        assert!(matches!(
            store.delete_relationship(rel.id),
            Err(GraphError::NotFound(_))
        ));
        assert!(matches!(
            store.current_relationship_version(rel.id),
            Err(GraphError::NotFound(_))
        ));
        assert!(store.get_relationship_version(rel.id, 1).unwrap().is_some());
    }

    #[test]
    fn delete_concept_removes_an_unreferenced_concept() {
        let store = KnowledgeStore::in_memory().unwrap();
        let c = concept("temporary");
        store.insert_concept(&c).unwrap();

        store.delete_concept(c.id).unwrap();

        assert!(store.get_concept(c.id).unwrap().is_none());
        assert!(matches!(
            store.current_concept_version(c.id),
            Err(GraphError::NotFound(_))
        ));
        assert!(store.get_concept_version(c.id, 1).unwrap().is_some());
    }

    #[test]
    fn delete_concept_rejects_relationship_and_procedure_dependencies() {
        let store = KnowledgeStore::in_memory().unwrap();
        let relationship_dependency = concept("relationship dependency");
        let procedure_dependency = concept("procedure dependency");
        let source = concept("source");
        for c in [&relationship_dependency, &procedure_dependency, &source] {
            store.insert_concept(c).unwrap();
        }
        store
            .insert_relationship(&Relationship::new(
                source.id,
                relationship_dependency.id,
                "depends-on",
            ))
            .unwrap();
        store
            .insert_procedure(
                &Procedure::new("implementation", Vec::new(), Expr::Literal(Value::Null))
                    .with_concept(procedure_dependency.id),
            )
            .unwrap();

        assert!(matches!(
            store.delete_concept(relationship_dependency.id),
            Err(GraphError::HasDependents(_))
        ));
        assert!(matches!(
            store.delete_concept(procedure_dependency.id),
            Err(GraphError::HasDependents(_))
        ));
        assert!(
            store
                .get_concept(relationship_dependency.id)
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .get_concept(procedure_dependency.id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn traverse_multi_hop() {
        let store = KnowledgeStore::in_memory().unwrap();
        let a = concept("a");
        let b = concept("b");
        let c = concept("c");
        for concept in [&a, &b, &c] {
            store.insert_concept(concept).unwrap();
        }

        store
            .insert_relationship(&Relationship::new(a.id, b.id, "leads-to"))
            .unwrap();
        store
            .insert_relationship(&Relationship::new(b.id, c.id, "leads-to"))
            .unwrap();

        let reached = store.traverse(a.id, "leads-to", 2).unwrap();
        assert!(reached.iter().any(|(id, hops)| *id == b.id && *hops == 1));
        assert!(reached.iter().any(|(id, hops)| *id == c.id && *hops == 2));

        // With only 1 hop allowed, c should not be reachable.
        let reached_one_hop = store.traverse(a.id, "leads-to", 1).unwrap();
        assert!(!reached_one_hop.iter().any(|(id, _)| *id == c.id));
    }

    #[test]
    fn traverse_with_zero_hops_returns_nothing() {
        let store = KnowledgeStore::in_memory().unwrap();
        let a = concept("a");
        let b = concept("b");
        for concept in [&a, &b] {
            store.insert_concept(concept).unwrap();
        }
        store
            .insert_relationship(&Relationship::new(a.id, b.id, "leads-to"))
            .unwrap();

        assert!(store.traverse(a.id, "leads-to", 0).unwrap().is_empty());
    }

    #[test]
    fn get_dependents_finds_sources() {
        let store = KnowledgeStore::in_memory().unwrap();
        let base = concept("database");
        let dependent = concept("auth-service");
        store.insert_concept(&base).unwrap();
        store.insert_concept(&dependent).unwrap();

        let base_relationship = Relationship::new(dependent.id, base.id, "depends-on");
        store.insert_relationship(&base_relationship).unwrap();

        let dependents = store.get_dependents(base.id).unwrap();
        assert_eq!(dependents, vec![dependent.id]);
    }

    #[test]
    fn concept_dependency_report_includes_concepts_and_attached_procedures() {
        let store = KnowledgeStore::in_memory().unwrap();
        let base = concept("base");
        let dependent = concept("dependent");
        let unrelated = concept("unrelated");
        for concept in [&base, &dependent, &unrelated] {
            store.insert_concept(concept).unwrap();
        }
        let base_relationship = Relationship::new(dependent.id, base.id, "depends-on");
        store.insert_relationship(&base_relationship).unwrap();
        store
            .insert_relationship(&Relationship::new(unrelated.id, dependent.id, "depends-on"))
            .unwrap();
        let attached = Procedure::new("attached", Vec::new(), Expr::Literal(Value::Null))
            .with_concept(base.id);
        let unrelated_proc = Procedure::new("unrelated", Vec::new(), Expr::Literal(Value::Null))
            .with_concept(unrelated.id);
        store.insert_procedure(&attached).unwrap();
        store.insert_procedure(&unrelated_proc).unwrap();

        let report = store
            .get_dependency_report(DependencyTarget::Concept(base.id))
            .unwrap();

        assert_eq!(report.target, DependencyTarget::Concept(base.id));
        assert_eq!(
            report.dependents,
            vec![
                Dependent::Concept(dependent.id),
                Dependent::Procedure {
                    id: attached.id,
                    kind: ProcedureDependencyKind::AttachedToConcept,
                },
            ]
        );
        assert_eq!(report.relationships.len(), 1);
        assert_eq!(
            report.relationships[0].relationship_id,
            base_relationship.id
        );
        assert_eq!(report.relationships[0].version, 1);
    }

    #[test]
    fn dependency_report_excludes_unusable_edges_entities_and_callers() {
        let store = KnowledgeStore::in_memory().unwrap();
        let base = concept("base");
        let active = concept("active dependent");
        let retired_source = concept("retired dependent");
        for concept in [&base, &active, &retired_source] {
            store.insert_concept(concept).unwrap();
        }
        let active_relationship = Relationship::new(active.id, base.id, "depends-on");
        store.insert_relationship(&active_relationship).unwrap();
        let mut retired_relationship = Relationship::new(retired_source.id, base.id, "depends-on");
        retired_relationship.lifecycle = Lifecycle::Retired;
        store.insert_relationship(&retired_relationship).unwrap();

        let callee = Procedure::new("callee", Vec::new(), Expr::Literal(Value::Null));
        let mut retired_caller = Procedure::new(
            "retired caller",
            Vec::new(),
            Expr::Call {
                procedure: callee.id,
                args: Vec::new(),
            },
        );
        retired_caller.lifecycle = Lifecycle::Retired;
        store.insert_procedure(&callee).unwrap();
        store.insert_procedure(&retired_caller).unwrap();

        let concept_report = store
            .get_dependency_report(DependencyTarget::Concept(base.id))
            .unwrap();
        assert_eq!(
            concept_report.dependents,
            vec![Dependent::Concept(active.id)]
        );
        assert_eq!(concept_report.relationships.len(), 1);
        assert_eq!(
            concept_report.relationships[0].relationship_id,
            active_relationship.id
        );

        let procedure_report = store
            .get_dependency_report(DependencyTarget::Procedure(callee.id))
            .unwrap();
        assert!(procedure_report.dependents.is_empty());
    }

    #[test]
    fn procedure_dependency_report_finds_nested_callers() {
        let store = KnowledgeStore::in_memory().unwrap();
        let callee = Procedure::new("callee", Vec::new(), Expr::Literal(Value::Int(1)));
        store.insert_procedure(&callee).unwrap();
        let direct_caller = Procedure::new(
            "direct caller",
            Vec::new(),
            Expr::Call {
                procedure: callee.id,
                args: Vec::new(),
            },
        );
        let nested_caller = Procedure::new(
            "nested caller",
            Vec::new(),
            Expr::Block(vec![Expr::ListExpr(vec![Expr::Call {
                procedure: callee.id,
                args: Vec::new(),
            }])]),
        );
        let unrelated = Procedure::new("unrelated", Vec::new(), Expr::Literal(Value::Null));
        for procedure in [&direct_caller, &nested_caller, &unrelated] {
            store.insert_procedure(procedure).unwrap();
        }

        let report = store
            .get_dependency_report(DependencyTarget::Procedure(callee.id))
            .unwrap();

        assert_eq!(report.target, DependencyTarget::Procedure(callee.id));
        assert_eq!(
            report.dependents,
            vec![
                Dependent::Procedure {
                    id: direct_caller.id,
                    kind: ProcedureDependencyKind::CallsProcedure,
                },
                Dependent::Procedure {
                    id: nested_caller.id,
                    kind: ProcedureDependencyKind::CallsProcedure,
                },
            ]
        );
    }

    #[test]
    fn procedure_dependency_report_includes_calls_from_executable_contract_checks() {
        let store = KnowledgeStore::in_memory().unwrap();
        let callee = Procedure::new(
            "contract helper",
            Vec::new(),
            Expr::Literal(Value::Bool(true)),
        );
        let mut caller = Procedure::new("contract caller", Vec::new(), Expr::Literal(Value::Null));
        caller
            .contract
            .requires
            .push(
                Condition::described("helper must approve").with_check(Expr::Call {
                    procedure: callee.id,
                    args: Vec::new(),
                }),
            );
        store.insert_procedure(&callee).unwrap();
        store.insert_procedure(&caller).unwrap();

        let report = store
            .get_dependency_report(DependencyTarget::Procedure(callee.id))
            .unwrap();

        assert!(report.dependents.contains(&Dependent::Procedure {
            id: caller.id,
            kind: ProcedureDependencyKind::CallsProcedure,
        }));
    }

    #[test]
    fn retiring_a_procedure_rejects_live_callers_and_records_a_tombstone_revision() {
        let store = KnowledgeStore::in_memory().unwrap();
        let callee = Procedure::new("callee", Vec::new(), Expr::Literal(Value::Null));
        let caller = Procedure::new(
            "caller",
            Vec::new(),
            Expr::Call {
                procedure: callee.id,
                args: Vec::new(),
            },
        );
        store.insert_procedure(&callee).unwrap();
        store.insert_procedure(&caller).unwrap();

        assert!(matches!(
            store.delete_procedure(callee.id),
            Err(GraphError::HasDependents(_))
        ));
        store.delete_procedure(caller.id).unwrap();
        store.delete_procedure(callee.id).unwrap();

        let retired = store.get_procedure(callee.id).unwrap().unwrap();
        assert_eq!(retired.lifecycle, Lifecycle::Retired);
        assert_eq!(retired.version, 2);
        assert_eq!(store.list_procedure_versions(callee.id).unwrap().len(), 2);
        assert_eq!(
            store
                .get_procedure_version(callee.id, 1)
                .unwrap()
                .unwrap()
                .lifecycle,
            Lifecycle::Active
        );
    }

    #[test]
    fn dependency_direction_handles_symmetric_relationships() {
        let store = KnowledgeStore::in_memory().unwrap();
        let left = concept("left");
        let right = concept("right");
        store.insert_concept(&left).unwrap();
        store.insert_concept(&right).unwrap();
        store
            .insert_relationship(&Relationship::new(left.id, right.id, "inverse-of"))
            .unwrap();

        assert_eq!(store.get_dependents(left.id).unwrap(), vec![right.id]);
        assert_eq!(store.get_dependents(right.id).unwrap(), vec![left.id]);
    }

    #[test]
    fn traversal_excludes_retired_relationships_from_current_understanding() {
        let store = KnowledgeStore::in_memory().unwrap();
        let source = concept("source");
        let target = concept("target");
        store.insert_concept(&source).unwrap();
        store.insert_concept(&target).unwrap();
        let mut relationship = Relationship::new(source.id, target.id, "depends-on");
        store.insert_relationship(&relationship).unwrap();
        relationship.lifecycle = Lifecycle::Retired;
        store.revise_relationship(&relationship, 1).unwrap();

        assert!(
            store
                .traverse(source.id, "depends-on", 1)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn procedure_crud_with_contract() {
        let store = KnowledgeStore::in_memory().unwrap();
        let concept = concept("double");
        store.insert_concept(&concept).unwrap();

        let body = Expr::Literal(Value::Int(42));
        let mut proc = Procedure::new("double", vec![Param::named("x")], body)
            .with_contract(Contract::default())
            .with_concept(concept.id);
        store.insert_procedure(&proc).unwrap();

        let fetched = store
            .get_procedure(proc.id)
            .unwrap()
            .expect("procedure exists");
        assert_eq!(fetched.name, "double");
        assert_eq!(fetched.concept, Some(concept.id));

        let by_name = store
            .get_procedure_by_name("double")
            .unwrap()
            .expect("procedure exists");
        assert_eq!(by_name.id, proc.id);

        proc.version = 2;
        proc.lifecycle = Lifecycle::Validated;
        store.update_procedure(&proc).unwrap();

        let updated = store.get_procedure(proc.id).unwrap().unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.lifecycle, Lifecycle::Validated);

        let all = store.list_procedures().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn inserting_a_procedure_records_version_one() {
        let store = KnowledgeStore::in_memory().unwrap();
        let proc = Procedure::new("identity", vec![Param::named("x")], Expr::Var("x".into()));

        store.insert_procedure(&proc).unwrap();

        let stored = store
            .get_procedure_version(proc.id, 1)
            .unwrap()
            .expect("version one exists");
        assert_eq!(stored.id, proc.id);
        assert_eq!(stored.name, "identity");
        assert_eq!(stored.version, 1);

        let contracts = store.list_contract_versions(proc.id).unwrap();
        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts[0].0, 1);
    }

    #[test]
    fn procedure_updates_preserve_earlier_procedure_and_contract_snapshots() {
        let store = KnowledgeStore::in_memory().unwrap();
        let mut proc = Procedure::new("answer", Vec::new(), Expr::Literal(Value::Int(41)))
            .with_contract(Contract {
                promises: vec![Condition::described("returns the old answer")],
                ..Contract::default()
            });
        store.insert_procedure(&proc).unwrap();

        proc.body = Expr::Literal(Value::Int(42));
        proc.contract.promises = vec![Condition::described("returns the new answer")];
        proc.version = 2;
        proc.updated_at += 1;
        store.update_procedure(&proc).unwrap();

        let original = store
            .get_procedure_version(proc.id, 1)
            .unwrap()
            .expect("original version remains queryable");
        let updated = store
            .get_procedure_version(proc.id, 2)
            .unwrap()
            .expect("updated version is recorded");
        assert!(matches!(original.body, Expr::Literal(Value::Int(41))));
        assert!(matches!(updated.body, Expr::Literal(Value::Int(42))));

        let original_contract = store
            .get_contract_version(proc.id, 1)
            .unwrap()
            .expect("original contract remains queryable");
        let updated_contract = store
            .get_contract_version(proc.id, 2)
            .unwrap()
            .expect("updated contract is recorded");
        assert_eq!(
            original_contract.promises[0].description,
            "returns the old answer"
        );
        assert_eq!(
            updated_contract.promises[0].description,
            "returns the new answer"
        );
    }

    #[test]
    fn procedure_revision_requires_expected_current_and_exact_next_version() {
        let store = KnowledgeStore::in_memory().unwrap();
        let mut procedure = Procedure::new("answer", Vec::new(), Expr::Literal(Value::Int(41)));
        store.insert_procedure(&procedure).unwrap();

        procedure.version = 2;
        procedure.body = Expr::Literal(Value::Int(42));
        store.revise_procedure(&procedure, 1).unwrap();

        let mut stale = procedure.clone();
        stale.version = 3;
        stale.body = Expr::Literal(Value::Int(43));
        assert!(matches!(
            store.revise_procedure(&stale, 1),
            Err(GraphError::RevisionConflict {
                expected: 1,
                actual: 2,
                ..
            })
        ));

        let mut skipped = procedure.clone();
        skipped.version = 4;
        assert!(matches!(
            store.revise_procedure(&skipped, 2),
            Err(GraphError::NonMonotonicRevision {
                expected_next: 3,
                proposed: 4,
                ..
            })
        ));
        assert_eq!(
            store.get_procedure(procedure.id).unwrap().unwrap().version,
            2
        );
        assert_eq!(
            store.list_procedure_versions(procedure.id).unwrap().len(),
            2
        );
    }

    #[test]
    fn procedure_revision_rejects_created_at_drift() {
        let store = KnowledgeStore::in_memory().unwrap();
        let procedure = Procedure::new("answer", Vec::new(), Expr::Literal(Value::Int(41)));
        store.insert_procedure(&procedure).unwrap();
        let mut invalid = procedure.clone();
        invalid.version = 2;
        invalid.created_at += 1;

        assert!(matches!(
            store.revise_procedure(&invalid, 1),
            Err(GraphError::ImmutableFieldChange {
                field: "created_at",
                ..
            })
        ));
        assert_eq!(store.current_procedure_version(procedure.id).unwrap(), 1);
        assert_eq!(
            store.list_procedure_versions(procedure.id).unwrap().len(),
            1
        );
    }

    #[test]
    fn procedure_revision_rolls_back_current_when_snapshot_write_fails() {
        let store = KnowledgeStore::in_memory().unwrap();
        let mut procedure = Procedure::new("answer", Vec::new(), Expr::Literal(Value::Int(41)));
        store.insert_procedure(&procedure).unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_second_procedure_snapshot
                 BEFORE INSERT ON procedure_versions WHEN NEW.version = 2
                 BEGIN
                     SELECT RAISE(ABORT, 'snapshot rejected');
                 END;",
            )
            .unwrap();

        procedure.version = 2;
        procedure.body = Expr::Literal(Value::Int(42));
        assert!(matches!(
            store.revise_procedure(&procedure, 1),
            Err(GraphError::Sqlite(_))
        ));

        let current = store.get_procedure(procedure.id).unwrap().unwrap();
        assert_eq!(current.version, 1);
        assert!(matches!(current.body, Expr::Literal(Value::Int(41))));
        assert_eq!(
            store.list_procedure_versions(procedure.id).unwrap().len(),
            1
        );
    }

    #[test]
    fn procedure_and_contract_versions_are_listed_in_version_order() {
        let store = KnowledgeStore::in_memory().unwrap();
        let mut proc = Procedure::new("counter", Vec::new(), Expr::Literal(Value::Int(1)));
        store.insert_procedure(&proc).unwrap();

        for version in [2, 3] {
            proc.version = version;
            proc.body = Expr::Literal(Value::Int(version.into()));
            proc.contract.promises = vec![Condition::described(format!("version {version}"))];
            proc.updated_at += 1;
            store.update_procedure(&proc).unwrap();
        }

        let procedures = store.list_procedure_versions(proc.id).unwrap();
        assert_eq!(
            procedures
                .iter()
                .map(|proc| proc.version)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        let contracts = store.list_contract_versions(proc.id).unwrap();
        assert_eq!(
            contracts
                .iter()
                .map(|(version, _)| *version)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(contracts[0].1.promises.len(), 0);
        assert_eq!(contracts[1].1.promises[0].description, "version 2");
        assert_eq!(contracts[2].1.promises[0].description, "version 3");
    }

    #[test]
    fn delete_procedure_preserves_immutable_history() {
        let store = KnowledgeStore::in_memory().unwrap();
        let mut proc = Procedure::new("historical", Vec::new(), Expr::Literal(Value::Int(1)));
        store.insert_procedure(&proc).unwrap();
        proc.version = 2;
        proc.body = Expr::Literal(Value::Int(2));
        store.update_procedure(&proc).unwrap();

        store.delete_procedure(proc.id).unwrap();

        let retired = store.get_procedure(proc.id).unwrap().unwrap();
        assert_eq!(retired.lifecycle, Lifecycle::Retired);
        assert_eq!(retired.version, 3);
        assert_eq!(store.list_procedure_versions(proc.id).unwrap().len(), 3);
        assert!(store.get_procedure_version(proc.id, 1).unwrap().is_some());
        assert!(store.get_contract_version(proc.id, 2).unwrap().is_some());
        assert!(store.delete_procedure(proc.id).is_err());
    }

    #[test]
    fn mixed_lifecycle_change_set_is_atomic_versioned_and_idempotent() {
        let store = KnowledgeStore::in_memory().unwrap();
        let concept = concept("premise");
        let procedure = Procedure::new("consumer", Vec::new(), Expr::Literal(Value::Null));
        store.insert_concept(&concept).unwrap();
        store.insert_procedure(&procedure).unwrap();
        let relationship = Relationship::new(concept.id, concept.id, "self-check");
        store.insert_relationship(&relationship).unwrap();
        let change_set = LifecycleChangeSet {
            idempotency_key: "reconcile-mixed-1".into(),
            updated_at: 900,
            changes: vec![
                LifecycleChange::Concept {
                    id: concept.id,
                    expected_version: 1,
                    lifecycle: Lifecycle::Stale,
                },
                LifecycleChange::Procedure {
                    id: procedure.id,
                    expected_version: 1,
                    lifecycle: Lifecycle::UnderReview,
                },
                LifecycleChange::Relationship {
                    id: relationship.id,
                    expected_version: 1,
                    lifecycle: Lifecycle::Retired,
                },
            ],
        };

        let first = store.apply_lifecycle_change_set(&change_set).unwrap();
        let retried = store.apply_lifecycle_change_set(&change_set).unwrap();

        assert_eq!(retried, first);
        assert_eq!(
            store
                .get_change_receipt("reconcile-mixed-1")
                .unwrap()
                .as_ref(),
            Some(&first)
        );
        assert_eq!(first.changes.len(), 3);
        assert!(
            first
                .changes
                .iter()
                .all(|change| { change.previous_version == 1 && change.current_version == 2 })
        );
        assert_eq!(
            store.get_concept(concept.id).unwrap().unwrap().lifecycle,
            Lifecycle::Stale
        );
        assert_eq!(
            store
                .get_procedure(procedure.id)
                .unwrap()
                .unwrap()
                .lifecycle,
            Lifecycle::UnderReview
        );
        assert_eq!(
            store
                .get_relationship(relationship.id)
                .unwrap()
                .unwrap()
                .lifecycle,
            Lifecycle::Retired
        );
        assert_eq!(store.list_concept_versions(concept.id).unwrap().len(), 2);
        assert_eq!(
            store.list_procedure_versions(procedure.id).unwrap().len(),
            2
        );
        assert_eq!(
            store
                .list_relationship_versions(relationship.id)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store
                .get_concept_version(concept.id, 1)
                .unwrap()
                .unwrap()
                .lifecycle,
            Lifecycle::Active
        );
    }

    #[test]
    fn lifecycle_change_set_rolls_back_early_updates_when_a_later_snapshot_fails() {
        let store = KnowledgeStore::in_memory().unwrap();
        let concept = concept("early");
        let procedure = Procedure::new("late", Vec::new(), Expr::Literal(Value::Null));
        store.insert_concept(&concept).unwrap();
        store.insert_procedure(&procedure).unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_late_batch_snapshot
                 BEFORE INSERT ON procedure_versions WHEN NEW.version = 2
                 BEGIN
                     SELECT RAISE(ABORT, 'late snapshot rejected');
                 END;",
            )
            .unwrap();
        let change_set = LifecycleChangeSet {
            idempotency_key: "reconcile-rollback-1".into(),
            updated_at: 901,
            changes: vec![
                LifecycleChange::Concept {
                    id: concept.id,
                    expected_version: 1,
                    lifecycle: Lifecycle::Stale,
                },
                LifecycleChange::Procedure {
                    id: procedure.id,
                    expected_version: 1,
                    lifecycle: Lifecycle::Stale,
                },
            ],
        };

        assert!(matches!(
            store.apply_lifecycle_change_set(&change_set),
            Err(GraphError::Sqlite(_))
        ));
        assert_eq!(
            store.get_concept(concept.id).unwrap().unwrap().lifecycle,
            Lifecycle::Active
        );
        assert_eq!(store.current_concept_version(concept.id).unwrap(), 1);
        assert_eq!(store.list_concept_versions(concept.id).unwrap().len(), 1);
        assert_eq!(
            store
                .get_procedure(procedure.id)
                .unwrap()
                .unwrap()
                .lifecycle,
            Lifecycle::Active
        );
        assert!(
            store
                .get_change_receipt("reconcile-rollback-1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn lifecycle_change_set_rejects_reused_keys_and_stale_plans_without_partial_writes() {
        let store = KnowledgeStore::in_memory().unwrap();
        let first = concept("first");
        let second = concept("second");
        store.insert_concept(&first).unwrap();
        store.insert_concept(&second).unwrap();
        let original = LifecycleChangeSet {
            idempotency_key: "reconcile-key-1".into(),
            updated_at: 902,
            changes: vec![LifecycleChange::Concept {
                id: first.id,
                expected_version: 1,
                lifecycle: Lifecycle::Stale,
            }],
        };
        store.apply_lifecycle_change_set(&original).unwrap();
        let conflicting_key = LifecycleChangeSet {
            changes: vec![LifecycleChange::Concept {
                id: second.id,
                expected_version: 1,
                lifecycle: Lifecycle::Stale,
            }],
            ..original.clone()
        };
        assert!(matches!(
            store.apply_lifecycle_change_set(&conflicting_key),
            Err(GraphError::IdempotencyConflict { .. })
        ));
        assert!(matches!(
            store.get_change_set_receipt(&conflicting_key),
            Err(GraphError::IdempotencyConflict { .. })
        ));
        assert_eq!(
            store.get_change_set_receipt(&original).unwrap(),
            store.get_change_receipt("reconcile-key-1").unwrap()
        );

        let stale_plan = LifecycleChangeSet {
            idempotency_key: "reconcile-stale-1".into(),
            updated_at: 903,
            changes: vec![
                LifecycleChange::Concept {
                    id: second.id,
                    expected_version: 1,
                    lifecycle: Lifecycle::UnderReview,
                },
                LifecycleChange::Concept {
                    id: first.id,
                    expected_version: 1,
                    lifecycle: Lifecycle::UnderReview,
                },
            ],
        };
        assert!(matches!(
            store.apply_lifecycle_change_set(&stale_plan),
            Err(GraphError::RevisionConflict { .. })
        ));
        assert_eq!(
            store.get_concept(second.id).unwrap().unwrap().lifecycle,
            Lifecycle::Active
        );
        assert_eq!(store.current_concept_version(second.id).unwrap(), 1);
    }
}
