use ekg_core::{
    Concept, ConceptId, EkgError, Episode, EpisodeId, MutabilityClass, Procedure, ProcedureId,
    Relationship, RelationshipId,
};
use ekg_episode::{EpisodeFeedback, EpisodeQuery, EpisodeStore};
use ekg_graph::{GraphError, KnowledgeStore};

/// Read-only graph projection exposed by [`crate::Engine`]. Persistence crates
/// remain independently usable, but embedding through Engine cannot reach raw
/// mutation methods without explicit Engine admin authority.
#[derive(Clone, Copy)]
pub struct GraphView<'a> {
    pub(crate) store: &'a KnowledgeStore,
}

impl GraphView<'_> {
    pub fn get_concept(&self, id: ConceptId) -> Result<Option<Concept>, GraphError> {
        self.store.get_concept(id)
    }

    pub fn get_concept_by_name(&self, name: &str) -> Result<Option<Concept>, GraphError> {
        self.store.get_concept_by_name(name)
    }

    pub fn get_concept_version(
        &self,
        id: ConceptId,
        version: u32,
    ) -> Result<Option<Concept>, GraphError> {
        self.store.get_concept_version(id, version)
    }

    pub fn list_concept_versions(&self, id: ConceptId) -> Result<Vec<Concept>, GraphError> {
        self.store.list_concept_versions(id)
    }

    pub fn current_concept_version(&self, id: ConceptId) -> Result<u32, GraphError> {
        self.store.current_concept_version(id)
    }

    pub fn list_concepts(&self) -> Result<Vec<Concept>, GraphError> {
        self.store.list_concepts()
    }

    pub fn get_concepts_by_mutability(
        &self,
        mutability: MutabilityClass,
    ) -> Result<Vec<Concept>, GraphError> {
        self.store.get_concepts_by_mutability(mutability)
    }

    pub fn get_relationship(&self, id: RelationshipId) -> Result<Option<Relationship>, GraphError> {
        self.store.get_relationship(id)
    }

    pub fn get_relationship_version(
        &self,
        id: RelationshipId,
        version: u32,
    ) -> Result<Option<Relationship>, GraphError> {
        self.store.get_relationship_version(id, version)
    }

    pub fn list_relationship_versions(
        &self,
        id: RelationshipId,
    ) -> Result<Vec<Relationship>, GraphError> {
        self.store.list_relationship_versions(id)
    }

    pub fn get_procedure(&self, id: ProcedureId) -> Result<Option<Procedure>, GraphError> {
        self.store.get_procedure(id)
    }

    pub fn get_procedure_by_name(&self, name: &str) -> Result<Option<Procedure>, GraphError> {
        self.store.get_procedure_by_name(name)
    }

    pub fn get_procedure_version(
        &self,
        id: ProcedureId,
        version: u32,
    ) -> Result<Option<Procedure>, GraphError> {
        self.store.get_procedure_version(id, version)
    }

    pub fn list_procedure_versions(&self, id: ProcedureId) -> Result<Vec<Procedure>, GraphError> {
        self.store.list_procedure_versions(id)
    }

    pub fn list_procedures(&self) -> Result<Vec<Procedure>, GraphError> {
        self.store.list_procedures()
    }

    pub fn traverse(
        &self,
        start: ConceptId,
        kind: &str,
        max_hops: u32,
    ) -> Result<Vec<(ConceptId, u32)>, GraphError> {
        self.store.traverse(start, kind, max_hops)
    }
}

/// Read-only episode projection exposed by [`crate::Engine`]. Later feedback
/// and verified observations must enter through Engine operations so caller
/// supplied trust labels cannot become authorization.
#[derive(Clone, Copy)]
pub struct EpisodeView<'a> {
    pub(crate) store: &'a EpisodeStore,
}

impl EpisodeView<'_> {
    pub fn get(&self, id: EpisodeId) -> Result<Episode, EkgError> {
        self.store.get(id)
    }

    pub fn count(&self) -> Result<u64, EkgError> {
        self.store.count()
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<Episode>, EkgError> {
        self.store.list_recent(limit)
    }

    pub fn list_failures(&self, limit: u32) -> Result<Vec<Episode>, EkgError> {
        self.store.list_failures(limit)
    }

    pub fn find_by_concept(&self, concept_id: ConceptId) -> Result<Vec<Episode>, EkgError> {
        self.store.find_by_concept(concept_id)
    }

    pub fn find_by_observed_predicate(&self, predicate: &str) -> Result<Vec<Episode>, EkgError> {
        self.store.find_by_observed_predicate(predicate)
    }

    pub fn list_feedback(&self, episode_id: EpisodeId) -> Result<Vec<EpisodeFeedback>, EkgError> {
        self.store.list_feedback(episode_id)
    }

    pub fn query(&self, query: &EpisodeQuery) -> Result<Vec<Episode>, EkgError> {
        self.store.query(query)
    }
}
