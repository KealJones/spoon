use rusqlite::{Connection, OptionalExtension, Row, params};
use uuid::Uuid;

use ekg_core::{
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
    conn: Connection,
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

/// The current entities that would be affected by changing `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyReport {
    pub target: DependencyTarget,
    pub dependents: Vec<Dependent>,
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

    // ---------------------------------------------------------------
    // Concepts
    // ---------------------------------------------------------------

    pub fn insert_concept(&self, concept: &Concept) -> Result<()> {
        self.conn.execute(
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
        let changed = self.conn.execute(
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
        self.conn.execute(
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
        let changed = self.conn.execute(
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
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE procedures SET \
                name = ?2, params_json = ?3, body_json = ?4, contract_json = ?5, \
                test_cases_json = ?6, concept_id = ?7, version = ?8, lifecycle = ?9, \
                updated_at = ?10 \
             WHERE id = ?1",
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
            ],
        )?;
        if changed == 0 {
            return Err(GraphError::NotFound(format!("procedure {}", proc.id)));
        }
        Self::insert_procedure_snapshot(&tx, proc)?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes only the current procedure. Historical snapshots remain
    /// available for replay, auditing, and provenance.
    pub fn delete_procedure(&self, id: ProcedureId) -> Result<()> {
        let changed = self.conn.execute(
            "DELETE FROM procedures WHERE id = ?1",
            params![id.0.to_string()],
        )?;
        if changed == 0 {
            return Err(GraphError::NotFound(format!("procedure {id}")));
        }
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
        let mut stmt = self.conn.prepare(
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

        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE walk(id, hops) AS ( \
                SELECT target, 1 FROM relationships WHERE source = ?1 AND kind = ?2 \
                UNION ALL \
                SELECT r.target, w.hops + 1 \
                FROM relationships r JOIN walk w ON r.source = w.id \
                WHERE r.kind = ?2 AND w.hops < ?3 \
             ) \
             SELECT id, MIN(hops) AS hops FROM walk GROUP BY id ORDER BY hops",
        )?;
        let rows = stmt.query_map(params![start.0.to_string(), kind, max_hops as i64], |row| {
            let id: String = row.get(0)?;
            let hops: i64 = row.get(1)?;
            Ok((id, hops))
        })?;

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
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT source FROM relationships WHERE target = ?1 ORDER BY source",
        )?;
        let rows = stmt.query_map(params![concept_id.0.to_string()], |row| {
            let id: String = row.get(0)?;
            Ok(id)
        })?;

        let mut out = Vec::new();
        for row in rows {
            let id = row?;
            out.push(ConceptId(Uuid::parse_str(&id)?));
        }
        Ok(out)
    }

    /// Reports current graph entities that depend on a concept or procedure.
    ///
    /// A concept is depended on by relationship sources and by procedures
    /// attached to it. A procedure is depended on by procedures whose bodies
    /// call it, including calls nested inside other expressions.
    pub fn get_dependency_report(&self, target: DependencyTarget) -> Result<DependencyReport> {
        let dependents = match target {
            DependencyTarget::Concept(concept_id) => {
                let mut dependents = self
                    .get_dependents(concept_id)?
                    .into_iter()
                    .map(Dependent::Concept)
                    .collect::<Vec<_>>();
                let mut stmt = self
                    .conn
                    .prepare("SELECT id FROM procedures WHERE concept_id = ?1 ORDER BY name, id")?;
                let rows = stmt.query_map(params![concept_id.0.to_string()], |row| {
                    row.get::<_, String>(0)
                })?;
                for row in rows {
                    dependents.push(Dependent::Procedure {
                        id: ProcedureId(Uuid::parse_str(&row?)?),
                        kind: ProcedureDependencyKind::AttachedToConcept,
                    });
                }
                dependents
            }
            DependencyTarget::Procedure(procedure_id) => self
                .list_procedures()?
                .into_iter()
                .filter(|procedure| Self::expression_calls(&procedure.body, procedure_id))
                .map(|procedure| Dependent::Procedure {
                    id: procedure.id,
                    kind: ProcedureDependencyKind::CallsProcedure,
                })
                .collect(),
        };

        Ok(DependencyReport { target, dependents })
    }

    fn expression_calls(expression: &Expr, target: ProcedureId) -> bool {
        match expression {
            Expr::Literal(_) | Expr::Var(_) => false,
            Expr::BinOp { left, right, .. } => {
                Self::expression_calls(left, target) || Self::expression_calls(right, target)
            }
            Expr::UnOp { operand, .. } => Self::expression_calls(operand, target),
            Expr::Call { procedure, args } => {
                *procedure == target
                    || args
                        .iter()
                        .any(|argument| Self::expression_calls(argument, target))
            }
            Expr::If { cond, then, else_ } => {
                Self::expression_calls(cond, target)
                    || Self::expression_calls(then, target)
                    || Self::expression_calls(else_, target)
            }
            Expr::Let { value, body, .. } => {
                Self::expression_calls(value, target) || Self::expression_calls(body, target)
            }
            Expr::Block(expressions) | Expr::ListExpr(expressions) => expressions
                .iter()
                .any(|expression| Self::expression_calls(expression, target)),
            Expr::Index { collection, index } => {
                Self::expression_calls(collection, target) || Self::expression_calls(index, target)
            }
            Expr::FieldAccess { object, .. } => Self::expression_calls(object, target),
            Expr::Map {
                collection, body, ..
            } => Self::expression_calls(collection, target) || Self::expression_calls(body, target),
            Expr::Filter {
                collection,
                predicate,
                ..
            } => {
                Self::expression_calls(collection, target)
                    || Self::expression_calls(predicate, target)
            }
            Expr::Reduce {
                collection,
                init,
                body,
                ..
            } => {
                Self::expression_calls(collection, target)
                    || Self::expression_calls(init, target)
                    || Self::expression_calls(body, target)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekg_core::{Condition, Expr, Value};

    fn concept(name: &str) -> Concept {
        Concept::new(name, MutabilityClass::DefeasibleGeneral)
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
    fn list_concepts_returns_all() {
        let store = KnowledgeStore::in_memory().unwrap();
        store.insert_concept(&concept("a")).unwrap();
        store.insert_concept(&concept("b")).unwrap();

        let all = store.list_concepts().unwrap();
        assert_eq!(all.len(), 2);
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
    }

    #[test]
    fn delete_concept_removes_an_unreferenced_concept() {
        let store = KnowledgeStore::in_memory().unwrap();
        let c = concept("temporary");
        store.insert_concept(&c).unwrap();

        store.delete_concept(c.id).unwrap();

        assert!(store.get_concept(c.id).unwrap().is_none());
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

        store
            .insert_relationship(&Relationship::new(dependent.id, base.id, "depends-on"))
            .unwrap();

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
        store
            .insert_relationship(&Relationship::new(dependent.id, base.id, "depends-on"))
            .unwrap();
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

        assert!(store.get_procedure(proc.id).unwrap().is_none());
        assert_eq!(store.list_procedure_versions(proc.id).unwrap().len(), 2);
        assert!(store.get_procedure_version(proc.id, 1).unwrap().is_some());
        assert!(store.get_contract_version(proc.id, 2).unwrap().is_some());
        assert!(matches!(
            store.delete_procedure(proc.id),
            Err(GraphError::NotFound(_))
        ));
    }
}
