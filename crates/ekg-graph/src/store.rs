use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

use ekg_core::{
    Concept, ConceptId, Confidence, Contract, Lifecycle, MutabilityClass, Param, Procedure,
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
        let rows = stmt.query_map(params![concept_id.0.to_string()], Self::relationship_from_row)?;
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
        let rows = stmt.query_map(params![concept_id.0.to_string()], Self::relationship_from_row)?;
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
        self.conn.execute(
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
        let changed = self.conn.execute(
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
        Ok(())
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
        let rows = stmt.query_map(
            params![start.0.to_string(), kind, max_hops as i64],
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
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT source FROM relationships WHERE target = ?1")?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekg_core::{Expr, Value};

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
}
