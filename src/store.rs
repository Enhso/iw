//! The mnestic-backed graph store: schema initialization, atomic dossier
//! ingestion, and typed query methods for every named query in
//! [`crate::schema`].

use std::collections::BTreeMap;
use std::path::Path;

use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability};

use crate::embed::embed;
use crate::model::{dossier_id_for, ExtractionPayload};
use crate::schema;

/// A handle to an open mnestic database, with schema initialization,
/// atomic ingestion, and typed query methods.
pub struct GraphStore {
    db: DbInstance,
}

impl std::fmt::Debug for GraphStore {
    /// Prints `GraphStore { .. }` without dumping database contents.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphStore").finish_non_exhaustive()
    }
}

/// An error returned by a [`GraphStore`] operation.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The underlying mnestic script failed. The message is the formatted
    /// `miette::Report` from mnestic.
    #[error("database error: {0}")]
    Db(String),
    /// A query returned a row that did not match the shape its typed
    /// accessor expected.
    #[error("unexpected row shape in {query}: {detail}")]
    RowShape { query: &'static str, detail: String },
}

/// A named entity row returned by [`GraphStore::entities`].
#[derive(Debug, Clone, PartialEq)]
pub struct EntityRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub description: String,
}

/// A dated event row returned by [`GraphStore::events`].
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub id: String,
    pub name: String,
    pub occurred_at: String,
    pub description: String,
}

/// A citable source row returned by [`GraphStore::sources`].
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRow {
    pub id: String,
    pub title: String,
    pub url: String,
    pub provider: String,
    pub published: String,
    pub retrieved_at: String,
}

/// An evidence row returned by [`GraphStore::evidence`].
#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceRow {
    pub id: String,
    pub claim_id: String,
    pub source_id: String,
    pub stance: String,
    pub excerpt: String,
    pub quality: f64,
}

/// A claim row with zero-filled support/contradict evidence counts,
/// returned by [`GraphStore::claims_with_stance`].
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimStanceRow {
    pub id: String,
    pub text: String,
    pub kind: String,
    pub supports: i64,
    pub contradicts: i64,
}

/// A causal link row returned by [`GraphStore::causal_links`].
#[derive(Debug, Clone, PartialEq)]
pub struct CausalLinkRow {
    pub cause_id: String,
    pub effect_id: String,
    pub mechanism: String,
    pub confidence: String,
}

/// A causal chain row returned by [`GraphStore::causal_chains`].
#[derive(Debug, Clone, PartialEq)]
pub struct CausalChainRow {
    pub start: String,
    pub end: String,
    pub path: Vec<String>,
}

/// A crux claim row returned by [`GraphStore::cruxes`].
#[derive(Debug, Clone, PartialEq)]
pub struct CruxRow {
    pub id: String,
    pub text: String,
    pub supports: i64,
    pub contradicts: i64,
    pub downstream: i64,
    pub score: i64,
}

/// A consensus claim row returned by [`GraphStore::consensus`].
#[derive(Debug, Clone, PartialEq)]
pub struct ConsensusRow {
    pub id: String,
    pub text: String,
    pub sources: i64,
}

/// A temporal relation row returned by [`GraphStore::temporal_relations`].
#[derive(Debug, Clone, PartialEq)]
pub struct TemporalRow {
    pub before_id: String,
    pub after_id: String,
    pub relation: String,
}

/// A vector-similarity evidence row returned by
/// [`GraphStore::similar_evidence`].
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarEvidenceRow {
    pub id: String,
    pub claim_id: String,
    pub source_id: String,
    pub stance: String,
    pub excerpt: String,
    pub distance: f64,
}

/// A vector-similarity claim row returned by
/// [`GraphStore::similar_claims`].
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarClaimRow {
    pub id: String,
    pub text: String,
    pub distance: f64,
}

/// A summary of what [`GraphStore::ingest`] wrote, one count per payload
/// list plus the derived dossier id.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IngestReport {
    pub dossier_id: String,
    pub entities: usize,
    pub events: usize,
    pub sources: usize,
    pub claims: usize,
    pub evidence: usize,
    pub causal_links: usize,
    pub temporal_relations: usize,
}

impl GraphStore {
    /// Opens an in-memory mnestic database. Used by tests and by any run
    /// that does not need to persist across process restarts.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if mnestic fails to open the database.
    pub fn open_memory() -> Result<Self, StoreError> {
        let db = DbInstance::new("mem", "", "").map_err(db_error)?;
        Ok(Self { db })
    }

    /// Opens (or creates) a mnestic database persisted at `path` using the
    /// `sqlite` storage engine.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if mnestic fails to open the database.
    pub fn open_sqlite(path: &Path) -> Result<Self, StoreError> {
        let db = DbInstance::new("sqlite", path, "").map_err(db_error)?;
        Ok(Self { db })
    }

    /// Initializes the schema if it is not already present.
    ///
    /// Runs `::relations` and checks whether any relation is named
    /// `claim`; if so, the schema is assumed already initialized and this
    /// is a no-op. Otherwise runs [`schema::SCHEMA_DDL`]. This guard makes
    /// the method safe to call on every startup, since re-running
    /// `::hnsw create` on an existing index would otherwise fail.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if either script fails.
    pub fn init_schema(&self) -> Result<(), StoreError> {
        let existing = self.run("::relations", BTreeMap::new())?;
        let already_initialized = existing
            .rows
            .iter()
            .any(|row| row.first().and_then(DataValue::get_str) == Some("claim"));
        if already_initialized {
            return Ok(());
        }
        self.db
            .run_script(
                schema::SCHEMA_DDL,
                BTreeMap::new(),
                ScriptMutability::Mutable,
            )
            .map_err(db_error)?;
        Ok(())
    }

    /// Ingests `payload` as one dossier, atomically, via
    /// [`schema::INGEST_SCRIPT`].
    ///
    /// # Preconditions
    /// The caller must have already called `payload.validate()`
    /// successfully; this method does not re-validate.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the ingestion script fails; because
    /// the script is one chained set of blocks, mnestic rolls back every
    /// block and no partial write is left behind.
    pub fn ingest(
        &self,
        payload: &ExtractionPayload,
        created_at: &str,
    ) -> Result<IngestReport, StoreError> {
        let dossier_id = dossier_id_for(&payload.question);
        let params = ingest_params(payload, &dossier_id, created_at);

        self.db
            .run_script(schema::INGEST_SCRIPT, params, ScriptMutability::Mutable)
            .map_err(db_error)?;

        Ok(IngestReport {
            dossier_id,
            entities: payload.entities.len(),
            events: payload.events.len(),
            sources: payload.sources.len(),
            claims: payload.claims.len(),
            evidence: payload.evidence.len(),
            causal_links: payload.causal_links.len(),
            temporal_relations: payload.temporal_relations.len(),
        })
    }

    /// Returns every entity in `dossier_id`, ordered by kind then name.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn entities(&self, dossier_id: &str) -> Result<Vec<EntityRow>, StoreError> {
        let rows = self.run(schema::ENTITIES, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(EntityRow {
                    id: str_at(row, 0, "entities")?,
                    name: str_at(row, 1, "entities")?,
                    kind: str_at(row, 2, "entities")?,
                    description: str_at(row, 3, "entities")?,
                })
            })
            .collect()
    }

    /// Returns every event in `dossier_id`, ordered by occurrence date then
    /// name.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn events(&self, dossier_id: &str) -> Result<Vec<EventRow>, StoreError> {
        let rows = self.run(schema::EVENTS, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(EventRow {
                    id: str_at(row, 0, "events")?,
                    name: str_at(row, 1, "events")?,
                    occurred_at: str_at(row, 2, "events")?,
                    description: str_at(row, 3, "events")?,
                })
            })
            .collect()
    }

    /// Returns every `(event_id, entity_id)` participation pair for events
    /// in `dossier_id`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn event_actors(&self, dossier_id: &str) -> Result<Vec<(String, String)>, StoreError> {
        let rows = self.run(schema::EVENT_ACTORS, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok((
                    str_at(row, 0, "event_actors")?,
                    str_at(row, 1, "event_actors")?,
                ))
            })
            .collect()
    }

    /// Returns every source in `dossier_id`, ordered by provider then
    /// title.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn sources(&self, dossier_id: &str) -> Result<Vec<SourceRow>, StoreError> {
        let rows = self.run(schema::SOURCES, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(SourceRow {
                    id: str_at(row, 0, "sources")?,
                    title: str_at(row, 1, "sources")?,
                    url: str_at(row, 2, "sources")?,
                    provider: str_at(row, 3, "sources")?,
                    published: str_at(row, 4, "sources")?,
                    retrieved_at: str_at(row, 5, "sources")?,
                })
            })
            .collect()
    }

    /// Returns every `(claim_id, subject_id)` pair for claims in
    /// `dossier_id`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn claim_subjects(&self, dossier_id: &str) -> Result<Vec<(String, String)>, StoreError> {
        let rows = self.run(schema::CLAIM_SUBJECTS, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok((
                    str_at(row, 0, "claim_subjects")?,
                    str_at(row, 1, "claim_subjects")?,
                ))
            })
            .collect()
    }

    /// Returns every evidence row in `dossier_id`, ordered by claim then
    /// id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn evidence(&self, dossier_id: &str) -> Result<Vec<EvidenceRow>, StoreError> {
        let rows = self.run(schema::EVIDENCE, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(EvidenceRow {
                    id: str_at(row, 0, "evidence")?,
                    claim_id: str_at(row, 1, "evidence")?,
                    source_id: str_at(row, 2, "evidence")?,
                    stance: str_at(row, 3, "evidence")?,
                    excerpt: str_at(row, 4, "evidence")?,
                    quality: float_at(row, 5, "evidence")?,
                })
            })
            .collect()
    }

    /// Returns every claim in `dossier_id` with zero-filled
    /// support/contradict evidence counts, ordered by id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn claims_with_stance(&self, dossier_id: &str) -> Result<Vec<ClaimStanceRow>, StoreError> {
        let rows = self.run(schema::CLAIMS_WITH_STANCE, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(ClaimStanceRow {
                    id: str_at(row, 0, "claims_with_stance")?,
                    text: str_at(row, 1, "claims_with_stance")?,
                    kind: str_at(row, 2, "claims_with_stance")?,
                    supports: int_at(row, 3, "claims_with_stance")?,
                    contradicts: int_at(row, 4, "claims_with_stance")?,
                })
            })
            .collect()
    }

    /// Returns every causal link in `dossier_id`, ordered by cause then
    /// effect.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn causal_links(&self, dossier_id: &str) -> Result<Vec<CausalLinkRow>, StoreError> {
        let rows = self.run(schema::CAUSAL_LINKS, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(CausalLinkRow {
                    cause_id: str_at(row, 0, "causal_links")?,
                    effect_id: str_at(row, 1, "causal_links")?,
                    mechanism: str_at(row, 2, "causal_links")?,
                    confidence: str_at(row, 3, "causal_links")?,
                })
            })
            .collect()
    }

    /// Returns every causal chain in `dossier_id`, longest first, then
    /// ordered by start then end.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn causal_chains(&self, dossier_id: &str) -> Result<Vec<CausalChainRow>, StoreError> {
        let rows = self.run(schema::CAUSAL_CHAINS, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(CausalChainRow {
                    start: str_at(row, 0, "causal_chains")?,
                    end: str_at(row, 1, "causal_chains")?,
                    path: str_list_at(row, 2, "causal_chains")?,
                })
            })
            .collect()
    }

    /// Returns up to 3 crux claims in `dossier_id`, highest score first,
    /// then ordered by id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn cruxes(&self, dossier_id: &str) -> Result<Vec<CruxRow>, StoreError> {
        let rows = self.run(schema::CRUXES, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(CruxRow {
                    id: str_at(row, 0, "cruxes")?,
                    text: str_at(row, 1, "cruxes")?,
                    supports: int_at(row, 2, "cruxes")?,
                    contradicts: int_at(row, 3, "cruxes")?,
                    downstream: int_at(row, 4, "cruxes")?,
                    score: int_at(row, 5, "cruxes")?,
                })
            })
            .collect()
    }

    /// Returns claims in `dossier_id` with at least 2 distinct supporting
    /// sources and zero contradictions, most-sourced first, then ordered
    /// by id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn consensus(&self, dossier_id: &str) -> Result<Vec<ConsensusRow>, StoreError> {
        let rows = self.run(schema::CONSENSUS, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(ConsensusRow {
                    id: str_at(row, 0, "consensus")?,
                    text: str_at(row, 1, "consensus")?,
                    sources: int_at(row, 2, "consensus")?,
                })
            })
            .collect()
    }

    /// Returns every temporal relation in `dossier_id`, ordered by before
    /// then after.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn temporal_relations(&self, dossier_id: &str) -> Result<Vec<TemporalRow>, StoreError> {
        let rows = self.run(schema::TEMPORAL_RELATIONS, dossier_param(dossier_id))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(TemporalRow {
                    before_id: str_at(row, 0, "temporal_relations")?,
                    after_id: str_at(row, 1, "temporal_relations")?,
                    relation: str_at(row, 2, "temporal_relations")?,
                })
            })
            .collect()
    }

    /// Returns the 8 evidence rows globally nearest to `query_text`'s
    /// embedding, nearest first, then ordered by id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn similar_evidence(
        &self,
        query_text: &str,
    ) -> Result<Vec<SimilarEvidenceRow>, StoreError> {
        let rows = self.run(schema::SIMILAR_EVIDENCE, query_param(query_text))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(SimilarEvidenceRow {
                    id: str_at(row, 0, "similar_evidence")?,
                    claim_id: str_at(row, 1, "similar_evidence")?,
                    source_id: str_at(row, 2, "similar_evidence")?,
                    stance: str_at(row, 3, "similar_evidence")?,
                    excerpt: str_at(row, 4, "similar_evidence")?,
                    distance: float_at(row, 5, "similar_evidence")?,
                })
            })
            .collect()
    }

    /// Returns the 5 claims globally nearest to `query_text`'s embedding,
    /// nearest first, then ordered by id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn similar_claims(&self, query_text: &str) -> Result<Vec<SimilarClaimRow>, StoreError> {
        let rows = self.run(schema::SIMILAR_CLAIMS, query_param(query_text))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(SimilarClaimRow {
                    id: str_at(row, 0, "similar_claims")?,
                    text: str_at(row, 1, "similar_claims")?,
                    distance: float_at(row, 2, "similar_claims")?,
                })
            })
            .collect()
    }

    /// Returns the research question for `dossier_id`, or `None` if no
    /// dossier with that id has been ingested.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if the row does not match the expected
    /// shape.
    pub fn dossier_question(&self, dossier_id: &str) -> Result<Option<String>, StoreError> {
        const QUERY: &str = "?[question] := *dossier{id: $dossier_id, question}";
        let rows = self.run(QUERY, dossier_param(dossier_id))?;
        match rows.rows.first() {
            Some(row) => Ok(Some(str_at(row, 0, "dossier_question")?)),
            None => Ok(None),
        }
    }

    /// Runs `query` as an immutable (read-only) script against this
    /// database.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if mnestic fails to run the script.
    fn run(
        &self,
        query: &'static str,
        params: BTreeMap<String, DataValue>,
    ) -> Result<NamedRows, StoreError> {
        self.db
            .run_script(query, params, ScriptMutability::Immutable)
            .map_err(db_error)
    }
}

/// Converts a mnestic `miette::Report` into a [`StoreError::Db`].
fn db_error(err: impl std::fmt::Debug) -> StoreError {
    StoreError::Db(format!("{err:?}"))
}

/// Builds the `$dossier_id` parameter map shared by every dossier-scoped
/// query.
fn dossier_param(dossier_id: &str) -> BTreeMap<String, DataValue> {
    let mut params = BTreeMap::new();
    params.insert("dossier_id".to_string(), DataValue::from(dossier_id));
    params
}

/// Builds the `$q` parameter map shared by both vector-similarity queries:
/// the 256-dimensional embedding of `query_text`.
fn query_param(query_text: &str) -> BTreeMap<String, DataValue> {
    let mut params = BTreeMap::new();
    params.insert("q".to_string(), embedding_list(&embed(query_text)));
    params
}

/// Converts an embedding vector into the `DataValue::List` form mnestic
/// expects for a `<F32; 256>` column or a `vec($q)` parameter.
fn embedding_list(embedding: &[f32]) -> DataValue {
    DataValue::List(
        embedding
            .iter()
            .map(|component| DataValue::from(f64::from(*component)))
            .collect(),
    )
}

/// Builds the full `$dossier`, `$dossier_items`, `$entities`, ...
/// parameter map for [`schema::INGEST_SCRIPT`] from `payload`.
fn ingest_params(
    payload: &ExtractionPayload,
    dossier_id: &str,
    created_at: &str,
) -> BTreeMap<String, DataValue> {
    let mut params = BTreeMap::new();

    params.insert(
        "dossier".to_string(),
        DataValue::List(vec![DataValue::List(vec![
            DataValue::from(dossier_id),
            DataValue::from(payload.question.as_str()),
            DataValue::from(created_at),
        ])]),
    );

    params.insert(
        "dossier_items".to_string(),
        DataValue::List(
            payload
                .all_item_ids()
                .iter()
                .map(|item_id| {
                    DataValue::List(vec![
                        DataValue::from(dossier_id),
                        DataValue::from(item_id.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "entities".to_string(),
        DataValue::List(
            payload
                .entities
                .iter()
                .map(|entity| {
                    DataValue::List(vec![
                        DataValue::from(entity.id.as_str()),
                        DataValue::from(entity.name.as_str()),
                        DataValue::from(entity.kind.as_str()),
                        DataValue::from(entity.description.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "events".to_string(),
        DataValue::List(
            payload
                .events
                .iter()
                .map(|event| {
                    DataValue::List(vec![
                        DataValue::from(event.id.as_str()),
                        DataValue::from(event.name.as_str()),
                        DataValue::from(event.occurred_at.as_str()),
                        DataValue::from(event.description.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "event_actors".to_string(),
        DataValue::List(
            payload
                .events
                .iter()
                .flat_map(|event| {
                    event.actor_ids.iter().map(move |actor_id| {
                        DataValue::List(vec![
                            DataValue::from(event.id.as_str()),
                            DataValue::from(actor_id.as_str()),
                        ])
                    })
                })
                .collect(),
        ),
    );

    params.insert(
        "sources".to_string(),
        DataValue::List(
            payload
                .sources
                .iter()
                .map(|source| {
                    DataValue::List(vec![
                        DataValue::from(source.id.as_str()),
                        DataValue::from(source.title.as_str()),
                        DataValue::from(source.url.as_str()),
                        DataValue::from(source.provider.as_str()),
                        DataValue::from(source.published.as_str()),
                        DataValue::from(source.retrieved_at.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "claims".to_string(),
        DataValue::List(
            payload
                .claims
                .iter()
                .map(|claim| {
                    DataValue::List(vec![
                        DataValue::from(claim.id.as_str()),
                        DataValue::from(claim.text.as_str()),
                        DataValue::from(claim.kind.as_str()),
                        embedding_list(&embed(&claim.text)),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "claim_subjects".to_string(),
        DataValue::List(
            payload
                .claims
                .iter()
                .flat_map(|claim| {
                    claim.subject_ids.iter().map(move |subject_id| {
                        DataValue::List(vec![
                            DataValue::from(claim.id.as_str()),
                            DataValue::from(subject_id.as_str()),
                        ])
                    })
                })
                .collect(),
        ),
    );

    params.insert(
        "evidence".to_string(),
        DataValue::List(
            payload
                .evidence
                .iter()
                .map(|evidence| {
                    DataValue::List(vec![
                        DataValue::from(evidence.id.as_str()),
                        DataValue::from(evidence.claim_id.as_str()),
                        DataValue::from(evidence.source_id.as_str()),
                        DataValue::from(evidence.stance.as_str()),
                        DataValue::from(evidence.excerpt.as_str()),
                        DataValue::from(evidence.quality),
                        embedding_list(&embed(&evidence.excerpt)),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "causal_links".to_string(),
        DataValue::List(
            payload
                .causal_links
                .iter()
                .map(|link| {
                    DataValue::List(vec![
                        DataValue::from(link.cause_id.as_str()),
                        DataValue::from(link.effect_id.as_str()),
                        DataValue::from(link.mechanism.as_str()),
                        DataValue::from(link.confidence.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "temporal_relations".to_string(),
        DataValue::List(
            payload
                .temporal_relations
                .iter()
                .map(|relation| {
                    DataValue::List(vec![
                        DataValue::from(relation.before_id.as_str()),
                        DataValue::from(relation.after_id.as_str()),
                        DataValue::from(relation.relation.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params
}

/// Reads a `String` from `row[idx]`.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing or is not a
/// string.
fn str_at(row: &[DataValue], idx: usize, query: &'static str) -> Result<String, StoreError> {
    row.get(idx)
        .and_then(DataValue::get_str)
        .map(str::to_string)
        .ok_or_else(|| StoreError::RowShape {
            query,
            detail: format!("expected string at column {idx}, row: {row:?}"),
        })
}

/// Reads an `i64` from `row[idx]`.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing or is not a
/// number.
fn int_at(row: &[DataValue], idx: usize, query: &'static str) -> Result<i64, StoreError> {
    row.get(idx)
        .and_then(DataValue::get_int)
        .ok_or_else(|| StoreError::RowShape {
            query,
            detail: format!("expected int at column {idx}, row: {row:?}"),
        })
}

/// Reads an `f64` from `row[idx]`.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing or is not a
/// number.
fn float_at(row: &[DataValue], idx: usize, query: &'static str) -> Result<f64, StoreError> {
    row.get(idx)
        .and_then(DataValue::get_float)
        .ok_or_else(|| StoreError::RowShape {
            query,
            detail: format!("expected float at column {idx}, row: {row:?}"),
        })
}

/// Reads a `Vec<String>` from the list-valued `row[idx]`.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing, is not a
/// list, or contains a non-string element.
fn str_list_at(
    row: &[DataValue],
    idx: usize,
    query: &'static str,
) -> Result<Vec<String>, StoreError> {
    let elements =
        row.get(idx)
            .and_then(DataValue::get_slice)
            .ok_or_else(|| StoreError::RowShape {
                query,
                detail: format!("expected list at column {idx}, row: {row:?}"),
            })?;
    elements
        .iter()
        .map(|element| {
            element
                .get_str()
                .map(str::to_string)
                .ok_or_else(|| StoreError::RowShape {
                    query,
                    detail: format!("expected string element in list at column {idx}"),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExtractionPayload;

    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/payload/semiconductor.json"
    ));
    const CREATED_AT: &str = "2026-09-14T00:00:00Z";

    fn fixture_payload() -> ExtractionPayload {
        serde_json::from_str(FIXTURE).expect("fixture payload parses")
    }

    fn small_payload(question: &str, entity_id: &str, entity_name: &str) -> ExtractionPayload {
        ExtractionPayload {
            schema_version: 1,
            question: question.to_string(),
            entities: vec![crate::model::Entity {
                id: entity_id.to_string(),
                name: entity_name.to_string(),
                kind: "organization".to_string(),
                description: "A minimal handcrafted entity.".to_string(),
            }],
            events: vec![],
            sources: vec![],
            claims: vec![],
            evidence: vec![],
            causal_links: vec![],
            temporal_relations: vec![],
        }
    }

    #[test]
    fn init_schema_is_idempotent() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("first init");
        store.init_schema().expect("second init");
    }

    #[test]
    fn ingest_report_counts_match_fixture_lists() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let report = store.ingest(&payload, CREATED_AT).expect("ingest fixture");

        assert_eq!(report.entities, payload.entities.len());
        assert_eq!(report.events, payload.events.len());
        assert_eq!(report.sources, payload.sources.len());
        assert_eq!(report.claims, payload.claims.len());
        assert_eq!(report.evidence, payload.evidence.len());
        assert_eq!(report.causal_links, payload.causal_links.len());
        assert_eq!(report.temporal_relations, payload.temporal_relations.len());
        assert_eq!(report.dossier_id, dossier_id_for(&payload.question));
    }

    #[test]
    fn entities_events_sources_evidence_counts_match_fixture() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store.ingest(&payload, CREATED_AT).expect("ingest fixture");

        assert_eq!(
            store.entities(&report.dossier_id).expect("entities").len(),
            payload.entities.len()
        );
        assert_eq!(
            store.events(&report.dossier_id).expect("events").len(),
            payload.events.len()
        );
        assert_eq!(
            store.sources(&report.dossier_id).expect("sources").len(),
            payload.sources.len()
        );
        assert_eq!(
            store.evidence(&report.dossier_id).expect("evidence").len(),
            payload.evidence.len()
        );
    }

    #[test]
    fn claims_with_stance_has_one_row_per_claim_zero_filled() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store.ingest(&payload, CREATED_AT).expect("ingest fixture");

        let rows = store
            .claims_with_stance(&report.dossier_id)
            .expect("claims_with_stance");
        assert_eq!(rows.len(), payload.claims.len());

        let evidence_less = rows
            .iter()
            .find(|row| row.id == "clm:chinese-fabs-cannot-access-secondhand-euv")
            .expect("evidence-less assumption claim present");
        assert_eq!(evidence_less.supports, 0);
        assert_eq!(evidence_less.contradicts, 0);
    }

    #[test]
    fn causal_chains_contains_a_path_of_at_least_four_nodes() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store.ingest(&payload, CREATED_AT).expect("ingest fixture");

        let chains = store
            .causal_chains(&report.dossier_id)
            .expect("causal_chains");
        assert!(
            chains.iter().any(|chain| chain.path.len() >= 4),
            "no chain of length >= 4 among {chains:?}"
        );
    }

    #[test]
    fn cruxes_returns_contested_claims_sorted_by_score_descending() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store.ingest(&payload, CREATED_AT).expect("ingest fixture");

        let cruxes = store.cruxes(&report.dossier_id).expect("cruxes");
        assert!(!cruxes.is_empty());
        assert!(cruxes.len() <= 3);
        for crux in &cruxes {
            assert!(crux.supports > 0 && crux.contradicts > 0);
        }
        let scores: Vec<i64> = cruxes.iter().map(|crux| crux.score).collect();
        let mut sorted_desc = scores.clone();
        sorted_desc.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(scores, sorted_desc);
    }

    #[test]
    fn consensus_returns_fact_claim_with_at_least_two_sources() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store.ingest(&payload, CREATED_AT).expect("ingest fixture");

        let consensus = store.consensus(&report.dossier_id).expect("consensus");
        assert!(!consensus.is_empty());
        assert!(consensus
            .iter()
            .any(|row| row.id == "clm:smic-achieved-7nm-class-node-2023" && row.sources >= 2));
        for row in &consensus {
            assert!(row.sources >= 2);
        }
    }

    #[test]
    fn temporal_relations_count_matches_fixture() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store.ingest(&payload, CREATED_AT).expect("ingest fixture");

        let rows = store
            .temporal_relations(&report.dossier_id)
            .expect("temporal_relations");
        assert_eq!(rows.len(), payload.temporal_relations.len());
    }

    #[test]
    fn similar_evidence_top_hit_mentions_smic_or_mate_60() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        store.ingest(&payload, CREATED_AT).expect("ingest fixture");

        let rows = store
            .similar_evidence("SMIC 7nm Huawei Mate 60")
            .expect("similar_evidence");
        assert_eq!(rows.len(), 8);
        for pair in rows.windows(2) {
            assert!(pair[0].distance <= pair[1].distance);
        }
        let top = &rows[0];
        assert!(
            top.excerpt.contains("SMIC") || top.excerpt.contains("Mate 60"),
            "top hit excerpt did not mention SMIC or Mate 60: {}",
            top.excerpt
        );
    }

    #[test]
    fn similar_claims_returns_five_rows_sorted_by_distance() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        store.ingest(&payload, CREATED_AT).expect("ingest fixture");

        let rows = store
            .similar_claims("export controls semiconductor manufacturing China")
            .expect("similar_claims");
        assert_eq!(rows.len(), 5);
        for pair in rows.windows(2) {
            assert!(pair[0].distance <= pair[1].distance);
        }
    }

    #[test]
    fn dossier_question_round_trips_and_reingest_is_stable() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store.ingest(&payload, CREATED_AT).expect("first ingest");

        assert_eq!(
            store
                .dossier_question(&report.dossier_id)
                .expect("dossier_question"),
            Some(payload.question.clone())
        );

        let second_report = store.ingest(&payload, CREATED_AT).expect("re-ingest");
        assert_eq!(second_report, report);
        assert_eq!(
            store
                .entities(&report.dossier_id)
                .expect("entities after re-ingest")
                .len(),
            payload.entities.len()
        );
        assert_eq!(
            store
                .evidence(&report.dossier_id)
                .expect("evidence after re-ingest")
                .len(),
            payload.evidence.len()
        );
    }

    #[test]
    fn minimal_payload_with_only_entities_ingests_fine() {
        let payload = small_payload("Does a minimal payload ingest?", "ent:only-one", "Only One");
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let report = store
            .ingest(&payload, CREATED_AT)
            .expect("ingest minimal payload");
        assert_eq!(report.entities, 1);
        assert_eq!(report.evidence, 0);

        let entities = store.entities(&report.dossier_id).expect("entities");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].id, "ent:only-one");
    }

    #[test]
    fn failed_block_in_chained_script_rolls_back_earlier_blocks() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let mut params: BTreeMap<String, DataValue> = BTreeMap::new();
        params.insert(
            "entities".to_string(),
            DataValue::List(vec![DataValue::List(vec![
                DataValue::from("ent:rollback-test"),
                DataValue::from("Rollback Test"),
                DataValue::from("organization"),
                DataValue::from("Should not survive the failed second block."),
            ])]),
        );

        let script = r#"
{ ?[id, name, kind, description] <- $entities
  :put entity {id => name, kind, description} }
{ ?[a, b] <- [[1, 2]]
  :put relation_that_does_not_exist {a, b} }
"#;

        let result = store
            .db
            .run_script(script, params, ScriptMutability::Mutable);
        assert!(result.is_err(), "expected the chained script to fail");

        let dossier_param = {
            let mut params = BTreeMap::new();
            params.insert(
                "dossier_id".to_string(),
                DataValue::from("dos:does-not-matter"),
            );
            params
        };
        let rows = store
            .run(
                "?[id, name, kind, description] := *entity{id, name, kind, description}",
                dossier_param,
            )
            .expect("query entity relation directly");
        assert!(
            rows.rows.is_empty(),
            "entity block should have been rolled back: {:?}",
            rows.rows
        );
    }

    #[test]
    fn open_sqlite_persists_across_reopen() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db_path = dir.path().join("iw.db");

        {
            let store = GraphStore::open_sqlite(&db_path).expect("open sqlite store");
            store.init_schema().expect("init schema");
            let payload = fixture_payload();
            store.ingest(&payload, CREATED_AT).expect("ingest fixture");
        }

        let reopened = GraphStore::open_sqlite(&db_path).expect("reopen sqlite store");
        let dossier_id = dossier_id_for(&fixture_payload().question);
        assert_eq!(
            reopened
                .dossier_question(&dossier_id)
                .expect("dossier_question after reopen"),
            Some(fixture_payload().question)
        );
    }

    #[test]
    fn dossiers_are_scoped_but_vector_search_is_global() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let payload_a = fixture_payload();
        let report_a = store.ingest(&payload_a, CREATED_AT).expect("ingest A");

        let b_claim_text = "Dossier B's own minimal claim for cross-dossier vector testing.";
        let payload_b = ExtractionPayload {
            schema_version: 1,
            question: "Is unrelated dossier B fully isolated from dossier A?".to_string(),
            entities: vec![crate::model::Entity {
                id: "ent:dossier-b-only".to_string(),
                name: "Dossier B Only Entity".to_string(),
                kind: "organization".to_string(),
                description: "A minimal handcrafted entity.".to_string(),
            }],
            events: vec![],
            sources: vec![],
            claims: vec![crate::model::Claim {
                id: "clm:dossier-b-only".to_string(),
                text: b_claim_text.to_string(),
                kind: "assumption".to_string(),
                subject_ids: vec!["ent:dossier-b-only".to_string()],
            }],
            evidence: vec![],
            causal_links: vec![],
            temporal_relations: vec![],
        };
        let report_b = store.ingest(&payload_b, CREATED_AT).expect("ingest B");

        let entities_a = store.entities(&report_a.dossier_id).expect("entities A");
        assert!(!entities_a.iter().any(|row| row.id == "ent:dossier-b-only"));

        let entities_b = store.entities(&report_b.dossier_id).expect("entities B");
        assert!(entities_b.iter().all(|row| row.id == "ent:dossier-b-only"));
        assert!(!entities_b
            .iter()
            .any(|row| payload_a.entities.iter().any(|entity| entity.id == row.id)));

        // Querying with dossier B's own claim text guarantees it is the
        // nearest match (distance 0), so the remaining slots in a 5-row
        // result must come from dossier A: proof that similar_claims does
        // not scope by dossier.
        let similar = store
            .similar_claims(b_claim_text)
            .expect("similar_claims across dossiers");
        assert_eq!(similar.len(), 5);
        assert!(similar.iter().any(|row| row.id == "clm:dossier-b-only"));
        assert!(similar
            .iter()
            .any(|row| payload_a.claims.iter().any(|claim| claim.id == row.id)));
    }
}
