//! The shared `ExtractionPayload` v2 contract (`docs/contracts.md` §A2 in
//! the `betomcat` repository), its id/slug helpers, and validation against
//! Global Constraints 10 and 11 (`docs/plan-phase1.md`) as extended by the
//! v2 contract: `content_hash` integrity, claim `support` range, and the
//! content-addressed `src:<provider>:<16 hex>` source id shape.
//!
//! Also carries [`HistoryItem`], the personal/bot forecast history record
//! (`docs/contracts.md` §C4/§E), which is validated the same way but is not
//! part of an [`ExtractionPayload`].

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::embed::fnv1a64;

const ENTITY_KINDS: &[&str] = &["person", "organization", "country", "technology", "policy"];
/// Source providers accepted in schema v2: the two AskNews endpoints plus
/// the v1 no-key default pair (`docs/contracts.md` §B1).
const SOURCE_PROVIDERS: &[&str] = &["asknews_news", "asknews_wiki", "wikipedia", "arxiv"];
const CLAIM_KINDS: &[&str] = &["hypothesis", "fact", "assumption"];
const EVIDENCE_STANCES: &[&str] = &["supports", "contradicts"];
const CONFIDENCE_LEVELS: &[&str] = &["low", "medium", "high"];
const TEMPORAL_RELATION_KINDS: &[&str] = &["before", "during", "after"];
/// `claims[].support_method`: `docs/contracts.md` §A2.
const SUPPORT_METHODS: &[&str] = &["jev", "none"];
/// `history_item.question_type`: `docs/contracts.md` §C4 `HistoryItem`.
const QUESTION_TYPES: &[&str] = &["binary", "multiple_choice", "numeric", "discrete", "date"];
/// `history_item.kind`: `docs/contracts.md` §C4 `HistoryItem`.
const HISTORY_KINDS: &[&str] = &["personal", "bot"];

/// A research extraction produced by the Python worker and ingested into
/// the mnestic graph store as one dossier. Schema v2
/// (`docs/contracts.md` §A2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractionPayload {
    /// Contract version; only `2` is currently supported.
    pub schema_version: u32,
    /// The research question this extraction answers.
    pub question: String,
    /// Named entities referenced by events, claims, and causal links.
    pub entities: Vec<Entity>,
    /// Dated occurrences.
    pub events: Vec<Event>,
    /// Citable source documents, each carrying its full fetched content.
    pub sources: Vec<Source>,
    /// Propositions about one or more subjects.
    pub claims: Vec<Claim>,
    /// Source excerpts that support or contradict a claim.
    pub evidence: Vec<Evidence>,
    /// Causal relationships between a cause and an effect.
    pub causal_links: Vec<CausalLink>,
    /// Temporal orderings between events.
    pub temporal_relations: Vec<TemporalRelation>,
    /// One entry per gate invocation (family routing, relevance/injection
    /// filter, claim support scoring) that ran while producing this
    /// payload. Informational; not referentially validated.
    #[serde(default)]
    pub gate_log: Vec<GateLogEntry>,
    /// Passages fetched but excluded from extraction by the relevance or
    /// injection gate. Informational; not referentially validated.
    #[serde(default)]
    pub dropped_sources: Vec<DroppedSource>,
}

/// A named entity, identified by an `ent:` id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    /// Global Constraint 10 id, prefixed `ent:`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One of `person | organization | country | technology | policy`.
    pub kind: String,
    /// Free-text description.
    pub description: String,
}

/// A dated occurrence, identified by an `evt:` id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Global Constraint 10 id, prefixed `evt:`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `YYYY-MM-DD`, or `""` if unknown.
    pub occurred_at: String,
    /// Free-text description.
    pub description: String,
    /// Entity ids that participated in this event.
    pub actor_ids: Vec<String>,
}

/// A citable source document, identified by a `src:<provider>:<16 hex>` id
/// (`docs/contracts.md` §A2), content-addressed via `content_hash`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// `src:<provider>:<first 16 hex of sha256(url)>`.
    pub id: String,
    /// Document title.
    pub title: String,
    /// Document URL.
    pub url: String,
    /// One of `asknews_news | asknews_wiki | wikipedia | arxiv`.
    pub provider: String,
    /// `YYYY-MM-DD`, or `""` if unknown.
    pub published: String,
    /// RFC 3339 UTC timestamp of when the worker fetched this source.
    pub retrieved_at: String,
    /// The full fetched text, after the worker's normalisation, exactly as
    /// hashed into `content_hash`.
    pub content: String,
    /// Lowercase hex sha256 of `content`.
    pub content_hash: String,
}

/// A proposition about one or more subjects, identified by a `clm:` id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Global Constraint 10 id, prefixed `clm:`.
    pub id: String,
    /// The claim text.
    pub text: String,
    /// One of `hypothesis | fact | assumption`.
    pub kind: String,
    /// Entity or event ids this claim is about.
    pub subject_ids: Vec<String>,
    /// Claim support score in `0.0..=1.0`, or `None` when the support gate
    /// did not run or failed.
    pub support: Option<f64>,
    /// One of `jev | none`.
    pub support_method: String,
}

/// A source excerpt that supports or contradicts a claim, identified by an
/// `evd:` id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// Global Constraint 10 id, prefixed `evd:`.
    pub id: String,
    /// The claim this evidence bears on.
    pub claim_id: String,
    /// The source this excerpt was taken from.
    pub source_id: String,
    /// One of `supports | contradicts`.
    pub stance: String,
    /// The quoted excerpt.
    pub excerpt: String,
    /// Evidence quality, in `0.0..=1.0`.
    pub quality: f64,
}

/// A causal relationship between a cause and an effect (each a claim or
/// event id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CausalLink {
    /// The causing claim or event id.
    pub cause_id: String,
    /// The affected claim or event id.
    pub effect_id: String,
    /// Free-text description of the causal mechanism.
    pub mechanism: String,
    /// One of `low | medium | high`.
    pub confidence: String,
}

/// A temporal ordering between two events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalRelation {
    /// The earlier event id.
    pub before_id: String,
    /// The later event id.
    pub after_id: String,
    /// One of `before | during | after`.
    pub relation: String,
}

/// One gate invocation record (`docs/contracts.md` §A2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateLogEntry {
    /// The gate's name, e.g. `"family"`, `"relevance"`, `"support"`.
    pub gate: String,
    /// One of `ok | failed | skipped`.
    pub status: String,
    /// Free-text detail.
    pub detail: String,
}

/// A passage fetched but excluded from extraction by the relevance or
/// injection gate (`docs/contracts.md` §A2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DroppedSource {
    /// The passage's source URL.
    pub url: String,
    /// One of `irrelevant | injection`.
    pub reason: String,
    /// The gate score that caused the drop.
    pub score: f64,
}

/// A personal or bot forecast history record (`docs/contracts.md` §C4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryItem {
    /// An external question id, e.g. `"metaculus:41234"`. Not a Global
    /// Constraint 10 id: history items reference questions minted by
    /// another system.
    pub id: String,
    /// One of `personal | bot`.
    pub kind: String,
    /// Question title.
    pub title: String,
    /// Question URL.
    pub url: String,
    /// One of `binary | multiple_choice | numeric | discrete | date`.
    pub question_type: String,
    /// The forecast itself: a float for `binary`, `{option: p}` for
    /// `multiple_choice`, or a percentiles/median object for `numeric` and
    /// `discrete`/`date`. Opaque to Rust; stored as-is.
    pub forecast: serde_json::Value,
    /// The resolved outcome, or `None` if unresolved.
    pub resolution: Option<String>,
    /// RFC 3339 UTC resolution time, or `None` if unresolved.
    pub resolved_at: Option<String>,
    /// The family this question is tagged to, or `None`.
    pub family_id: Option<String>,
    /// Free-text note.
    pub note: String,
}

/// Validation failure for an [`ExtractionPayload`] or [`HistoryItem`], per
/// Global Constraints 10 and 11 as extended by the v2 contract.
#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    /// A reference field named an id that does not exist in the payload.
    #[error("unknown reference in {field}: {id}")]
    UnknownReference { field: &'static str, id: String },
    /// The same id appears more than once across the payload's id lists.
    #[error("duplicate id: {0}")]
    DuplicateId(String),
    /// An id does not match the Global Constraint 10 format for its list.
    #[error("invalid id: {0}")]
    InvalidId(String),
    /// A source id does not match the v2 `src:<provider>:<16 hex>` shape,
    /// or its provider segment does not match the source's own `provider`
    /// field.
    #[error("invalid source id: {0}")]
    InvalidSourceId(String),
    /// A `String` enumeration field holds a value outside its allowed set.
    #[error("invalid value for {field}: {value}")]
    InvalidEnum { field: &'static str, value: String },
    /// A date field is neither `""` nor `YYYY-MM-DD`.
    #[error("invalid date for {field}: {value}")]
    InvalidDate { field: &'static str, value: String },
    /// An evidence `quality` value falls outside `0.0..=1.0`.
    #[error("quality {quality} out of range for {id}")]
    QualityOutOfRange { id: String, quality: f64 },
    /// A claim `support` value falls outside `0.0..=1.0`.
    #[error("support {support} out of range for {id}")]
    SupportOutOfRange { id: String, support: f64 },
    /// A source's `content_hash` does not equal `sha256(content)`.
    #[error("content_hash mismatch for {id}")]
    ContentHashMismatch { id: String },
    /// `schema_version` is not `2`.
    #[error("unsupported schema version: {0}")]
    UnsupportedSchemaVersion(u32),
    /// `question` is empty (after trimming whitespace).
    #[error("question must not be empty")]
    EmptyQuestion,
}

impl ExtractionPayload {
    /// Validates this payload against Global Constraints 10 and 11 as
    /// extended by the v2 contract (`docs/contracts.md` §A2): schema
    /// version, non-empty question, id format and uniqueness, enumeration
    /// membership, evidence quality and claim support range, source
    /// `content_hash` integrity, referential integrity, and date format.
    ///
    /// # Errors
    /// Returns the first [`PayloadError`] encountered; validation stops at
    /// the first failure rather than collecting every violation.
    pub fn validate(&self) -> Result<(), PayloadError> {
        if self.schema_version != 2 {
            return Err(PayloadError::UnsupportedSchemaVersion(self.schema_version));
        }
        if self.question.trim().is_empty() {
            return Err(PayloadError::EmptyQuestion);
        }

        let mut seen_ids: HashSet<String> = HashSet::new();

        for entity in &self.entities {
            validate_and_record_id(&entity.id, "ent", &mut seen_ids)?;
            validate_enum("entities[].kind", &entity.kind, ENTITY_KINDS)?;
        }
        for event in &self.events {
            validate_and_record_id(&event.id, "evt", &mut seen_ids)?;
            validate_date("events[].occurred_at", &event.occurred_at)?;
        }
        for source in &self.sources {
            validate_and_record_source_id(source, &mut seen_ids)?;
            validate_enum("sources[].provider", &source.provider, SOURCE_PROVIDERS)?;
            validate_date("sources[].published", &source.published)?;
            validate_content_hash(source)?;
        }
        for claim in &self.claims {
            validate_and_record_id(&claim.id, "clm", &mut seen_ids)?;
            validate_enum("claims[].kind", &claim.kind, CLAIM_KINDS)?;
            validate_enum(
                "claims[].support_method",
                &claim.support_method,
                SUPPORT_METHODS,
            )?;
            if let Some(support) = claim.support {
                if !(0.0..=1.0).contains(&support) {
                    return Err(PayloadError::SupportOutOfRange {
                        id: claim.id.clone(),
                        support,
                    });
                }
            }
        }
        for evidence in &self.evidence {
            validate_and_record_id(&evidence.id, "evd", &mut seen_ids)?;
            validate_enum("evidence[].stance", &evidence.stance, EVIDENCE_STANCES)?;
            if !(0.0..=1.0).contains(&evidence.quality) {
                return Err(PayloadError::QualityOutOfRange {
                    id: evidence.id.clone(),
                    quality: evidence.quality,
                });
            }
        }
        for causal_link in &self.causal_links {
            validate_enum(
                "causal_links[].confidence",
                &causal_link.confidence,
                CONFIDENCE_LEVELS,
            )?;
        }
        for temporal_relation in &self.temporal_relations {
            validate_enum(
                "temporal_relations[].relation",
                &temporal_relation.relation,
                TEMPORAL_RELATION_KINDS,
            )?;
        }

        let entity_ids: HashSet<&str> = self
            .entities
            .iter()
            .map(|entity| entity.id.as_str())
            .collect();
        let event_ids: HashSet<&str> = self.events.iter().map(|event| event.id.as_str()).collect();
        let claim_ids: HashSet<&str> = self.claims.iter().map(|claim| claim.id.as_str()).collect();
        let source_ids: HashSet<&str> = self
            .sources
            .iter()
            .map(|source| source.id.as_str())
            .collect();

        for event in &self.events {
            for actor_id in &event.actor_ids {
                if !entity_ids.contains(actor_id.as_str()) {
                    return Err(PayloadError::UnknownReference {
                        field: "actor_ids",
                        id: actor_id.clone(),
                    });
                }
            }
        }
        for claim in &self.claims {
            for subject_id in &claim.subject_ids {
                if !entity_ids.contains(subject_id.as_str())
                    && !event_ids.contains(subject_id.as_str())
                {
                    return Err(PayloadError::UnknownReference {
                        field: "subject_ids",
                        id: subject_id.clone(),
                    });
                }
            }
        }
        for evidence in &self.evidence {
            if !claim_ids.contains(evidence.claim_id.as_str()) {
                return Err(PayloadError::UnknownReference {
                    field: "claim_id",
                    id: evidence.claim_id.clone(),
                });
            }
            if !source_ids.contains(evidence.source_id.as_str()) {
                return Err(PayloadError::UnknownReference {
                    field: "source_id",
                    id: evidence.source_id.clone(),
                });
            }
        }
        for causal_link in &self.causal_links {
            if !claim_ids.contains(causal_link.cause_id.as_str())
                && !event_ids.contains(causal_link.cause_id.as_str())
            {
                return Err(PayloadError::UnknownReference {
                    field: "cause_id",
                    id: causal_link.cause_id.clone(),
                });
            }
            if !claim_ids.contains(causal_link.effect_id.as_str())
                && !event_ids.contains(causal_link.effect_id.as_str())
            {
                return Err(PayloadError::UnknownReference {
                    field: "effect_id",
                    id: causal_link.effect_id.clone(),
                });
            }
        }
        for temporal_relation in &self.temporal_relations {
            if !event_ids.contains(temporal_relation.before_id.as_str()) {
                return Err(PayloadError::UnknownReference {
                    field: "before_id",
                    id: temporal_relation.before_id.clone(),
                });
            }
            if !event_ids.contains(temporal_relation.after_id.as_str()) {
                return Err(PayloadError::UnknownReference {
                    field: "after_id",
                    id: temporal_relation.after_id.clone(),
                });
            }
        }

        Ok(())
    }

    /// Returns the ids of every entity, event, source, claim, and evidence
    /// item in this payload, in that order. Used by ingestion to populate
    /// `dossier_item` rows so graph queries can be scoped to a dossier.
    pub fn all_item_ids(&self) -> Vec<String> {
        self.entities
            .iter()
            .map(|entity| entity.id.clone())
            .chain(self.events.iter().map(|event| event.id.clone()))
            .chain(self.sources.iter().map(|source| source.id.clone()))
            .chain(self.claims.iter().map(|claim| claim.id.clone()))
            .chain(self.evidence.iter().map(|evidence| evidence.id.clone()))
            .collect()
    }
}

impl HistoryItem {
    /// Validates this history item: non-empty `id`, enumerations, and
    /// `resolution`/`resolved_at` consistency is left to the caller (either
    /// may be set independently; the contract does not require them to
    /// agree).
    ///
    /// # Errors
    /// Returns [`PayloadError::EmptyQuestion`] if `id` is empty (reused: an
    /// empty required identifier), or [`PayloadError::InvalidEnum`] if
    /// `kind` or `question_type` is not one of the allowed values.
    pub fn validate(&self) -> Result<(), PayloadError> {
        if self.id.trim().is_empty() {
            return Err(PayloadError::EmptyQuestion);
        }
        validate_enum("kind", &self.kind, HISTORY_KINDS)?;
        validate_enum("question_type", &self.question_type, QUESTION_TYPES)?;
        Ok(())
    }
}

/// Computes the lowercase hex sha256 digest of `content`.
pub fn sha256_hex(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Converts arbitrary text into a lowercase, hyphen-separated slug per
/// Global Constraint 10: runs of characters that are not ASCII alphanumeric
/// collapse to a single `-`, and any leading or trailing `-` is dropped.
///
/// # Examples
/// ```
/// use iw_server::model::slugify;
/// assert_eq!(slugify("  TSMC & ASML: EUV!! "), "tsmc-asml-euv");
/// ```
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

/// Builds a dossier id from a research question: `"dos:"` followed by the
/// first 60 characters of `slugify(question.trim())` (any `-` left
/// dangling by the truncation removed, or the literal `"q"` if that slug
/// is empty), a `-`, and the low 32 bits of the 64-bit FNV-1a hash of the
/// trimmed question as 8 lowercase hex digits.
///
/// The hash disambiguates questions that would otherwise collide: two
/// questions differing only after the 60-character truncation point, or
/// two questions with no ASCII alphanumeric characters at all (which both
/// slugify to `""`, falling back to `"q"`), no longer share a dossier. At
/// most 73 characters (`4 + 60 + 1 + 8`).
pub fn dossier_id_for(question: &str) -> String {
    let trimmed_question = question.trim();
    let slug = slugify(trimmed_question);
    let truncated: String = slug.chars().take(60).collect();
    let truncated = truncated.trim_end_matches('-');
    let slug = if truncated.is_empty() { "q" } else { truncated };
    let hash = fnv1a64(trimmed_question) as u32;
    format!("dos:{slug}-{hash:08x}")
}

/// Checks that `id` matches the Global Constraint 10 format for a list
/// whose ids carry `expected_prefix` (e.g. `"ent"` for entities): the
/// literal `"{expected_prefix}:"` followed by lowercase alphanumeric
/// segments separated by single hyphens, no leading, trailing, or doubled
/// hyphen, and at most 80 characters in the id as a whole.
fn is_valid_id(id: &str, expected_prefix: &str) -> bool {
    if id.len() > 80 {
        return false;
    }
    let Some(rest) = id
        .strip_prefix(expected_prefix)
        .and_then(|remainder| remainder.strip_prefix(':'))
    else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    let bytes = rest.as_bytes();
    if bytes[0] == b'-' || bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    let mut prev_was_dash = false;
    for &byte in bytes {
        if byte == b'-' {
            if prev_was_dash {
                return false;
            }
            prev_was_dash = true;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            prev_was_dash = false;
        } else {
            return false;
        }
    }
    true
}

/// Checks that `id` matches the v2 source id shape
/// (`docs/contracts.md` §A2): `src:<provider>:<16 lowercase hex digits>`,
/// where `<provider>` is exactly `provider` (the source's own `provider`
/// field, tying the id to the record it names).
fn is_valid_source_id(id: &str, provider: &str) -> bool {
    let Some(rest) = id.strip_prefix("src:") else {
        return false;
    };
    let Some(hex) = rest
        .strip_prefix(provider)
        .and_then(|remainder| remainder.strip_prefix(':'))
    else {
        return false;
    };
    hex.len() == 16
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Validates `id` against [`is_valid_id`] for `expected_prefix` and, if
/// valid, records it in `seen`.
///
/// # Errors
/// Returns [`PayloadError::InvalidId`] if the format check fails, or
/// [`PayloadError::DuplicateId`] if `id` was already present in `seen`.
fn validate_and_record_id(
    id: &str,
    expected_prefix: &str,
    seen: &mut HashSet<String>,
) -> Result<(), PayloadError> {
    if !is_valid_id(id, expected_prefix) {
        return Err(PayloadError::InvalidId(id.to_string()));
    }
    if !seen.insert(id.to_string()) {
        return Err(PayloadError::DuplicateId(id.to_string()));
    }
    Ok(())
}

/// Validates `source.id` against [`is_valid_source_id`] and, if valid,
/// records it in `seen`.
///
/// # Errors
/// Returns [`PayloadError::InvalidSourceId`] if the format check fails, or
/// [`PayloadError::DuplicateId`] if the id was already present in `seen`.
fn validate_and_record_source_id(
    source: &Source,
    seen: &mut HashSet<String>,
) -> Result<(), PayloadError> {
    if !is_valid_source_id(&source.id, &source.provider) {
        return Err(PayloadError::InvalidSourceId(source.id.clone()));
    }
    if !seen.insert(source.id.clone()) {
        return Err(PayloadError::DuplicateId(source.id.clone()));
    }
    Ok(())
}

/// Validates that `source.content_hash` equals `sha256_hex(source.content)`
/// (`docs/contracts.md` §A2), which also rejects a malformed hash (wrong
/// length or casing), since a correctly computed digest can never match
/// one.
///
/// # Errors
/// Returns [`PayloadError::ContentHashMismatch`] on any mismatch.
fn validate_content_hash(source: &Source) -> Result<(), PayloadError> {
    if source.content_hash != sha256_hex(&source.content) {
        return Err(PayloadError::ContentHashMismatch {
            id: source.id.clone(),
        });
    }
    Ok(())
}

/// Validates that `value` is one of `allowed`.
///
/// # Errors
/// Returns [`PayloadError::InvalidEnum`] naming `field` and `value` if it
/// is not.
fn validate_enum(field: &'static str, value: &str, allowed: &[&str]) -> Result<(), PayloadError> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(PayloadError::InvalidEnum {
            field,
            value: value.to_string(),
        })
    }
}

/// Validates that `value` is `""` or a `YYYY-MM-DD` date: exactly 10 bytes,
/// ASCII digits at every position except a literal `-` at positions 4 and
/// 7.
///
/// # Errors
/// Returns [`PayloadError::InvalidDate`] naming `field` and `value` if it
/// is neither.
fn validate_date(field: &'static str, value: &str) -> Result<(), PayloadError> {
    if value.is_empty() {
        return Ok(());
    }
    let bytes = value.as_bytes();
    let is_valid = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(index, &byte)| {
            if index == 4 || index == 7 {
                true
            } else {
                byte.is_ascii_digit()
            }
        });
    if is_valid {
        Ok(())
    } else {
        Err(PayloadError::InvalidDate {
            field,
            value: value.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `{valid: [...], invalid: [...]}` vector list from
    /// `fixtures/contract/vectors.json`, shared with the Python worker's
    /// `python/tests/test_contract.py`.
    #[derive(Debug, serde::Deserialize)]
    struct ContractVectorSet<T> {
        valid: Vec<T>,
        invalid: Vec<T>,
    }

    /// The full contents of `fixtures/contract/vectors.json`.
    #[derive(Debug, serde::Deserialize)]
    struct ContractVectors {
        ids: ContractVectorSet<String>,
        dates: ContractVectorSet<String>,
        quality: ContractVectorSet<f64>,
    }

    const CONTRACT_VECTORS_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/contract/vectors.json"
    ));

    /// The four non-source list-prefix tags Global Constraint 10 still
    /// governs in v2 (source ids moved to the content-addressed shape).
    const KNOWN_ID_PREFIXES: [&str; 4] = ["ent", "evt", "clm", "evd"];

    /// Checks `id` against [`is_valid_id`]'s shape rule for whichever of
    /// [`KNOWN_ID_PREFIXES`] it actually carries, without regard to which
    /// specific list it is meant for; the separate
    /// `wrong_prefix_for_list_is_rejected` test covers the list-to-prefix
    /// binding.
    fn is_valid_id_for_any_known_prefix(id: &str) -> bool {
        KNOWN_ID_PREFIXES
            .iter()
            .any(|prefix| is_valid_id(id, prefix))
    }

    #[test]
    fn contract_vectors_classify_ids_dates_and_quality_like_rust_does() {
        let vectors: ContractVectors =
            serde_json::from_str(CONTRACT_VECTORS_JSON).expect("contract vectors parse");

        for id in &vectors.ids.valid {
            // v1's vector file may still contain `src:` slug-shaped ids;
            // v2 moved those to the content-addressed shape validated by
            // `is_valid_source_id`, exercised separately below. Skip any
            // `src:` entries here so this test stays about the four
            // unchanged prefixes.
            if id.starts_with("src:") {
                continue;
            }
            assert!(
                is_valid_id_for_any_known_prefix(id),
                "expected valid id: {id}"
            );
        }
        for id in &vectors.ids.invalid {
            if id.starts_with("src:") {
                continue;
            }
            assert!(
                !is_valid_id_for_any_known_prefix(id),
                "expected invalid id: {id}"
            );
        }

        for date in &vectors.dates.valid {
            assert!(
                validate_date("field", date).is_ok(),
                "expected valid date: {date:?}"
            );
        }
        for date in &vectors.dates.invalid {
            assert!(
                validate_date("field", date).is_err(),
                "expected invalid date: {date:?}"
            );
        }

        for quality in &vectors.quality.valid {
            assert!(
                (0.0..=1.0).contains(quality),
                "expected valid quality: {quality}"
            );
        }
        for quality in &vectors.quality.invalid {
            assert!(
                !(0.0..=1.0).contains(quality),
                "expected invalid quality: {quality}"
            );
        }
    }

    #[test]
    fn source_id_format_ties_hash_and_provider() {
        let url = "https://en.wikipedia.org/wiki/Semiconductor_industry_in_China";
        let hash = &sha256_hex(url)[..16];
        let good = format!("src:wikipedia:{hash}");
        assert!(is_valid_source_id(&good, "wikipedia"));
        // wrong provider segment
        assert!(!is_valid_source_id(&good, "arxiv"));
        // too short
        assert!(!is_valid_source_id(
            &format!("src:wikipedia:{}", &hash[..8]),
            "wikipedia"
        ));
        // uppercase hex
        assert!(!is_valid_source_id(
            &format!("src:wikipedia:{}", hash.to_uppercase()),
            "wikipedia"
        ));
        // missing colon segment
        assert!(!is_valid_source_id(&format!("src:{hash}"), "wikipedia"));
    }

    fn sample_source() -> Source {
        let url = "https://en.wikipedia.org/wiki/Semiconductor_industry_in_China".to_string();
        let content = "Semiconductor industry in China.\n\nSMIC produced a 7nm-class chip despite \
             controls."
            .to_string();
        let hash = &sha256_hex(&url)[..16];
        Source {
            id: format!("src:wikipedia:{hash}"),
            title: "Semiconductor industry in China".to_string(),
            url,
            provider: "wikipedia".to_string(),
            published: String::new(),
            retrieved_at: "2026-09-14T00:00:00Z".to_string(),
            content_hash: sha256_hex(&content),
            content,
        }
    }

    fn sample_payload() -> ExtractionPayload {
        let source = sample_source();
        ExtractionPayload {
            schema_version: 2,
            question: "Can export controls durably slow China's access to advanced \
                        semiconductor manufacturing capability?"
                .to_string(),
            entities: vec![
                Entity {
                    id: "ent:tsmc".to_string(),
                    name: "TSMC".to_string(),
                    kind: "organization".to_string(),
                    description: "Taiwanese semiconductor foundry.".to_string(),
                },
                Entity {
                    id: "ent:china".to_string(),
                    name: "China".to_string(),
                    kind: "country".to_string(),
                    description: "People's Republic of China.".to_string(),
                },
                Entity {
                    id: "ent:united-states".to_string(),
                    name: "United States".to_string(),
                    kind: "country".to_string(),
                    description: "United States of America.".to_string(),
                },
            ],
            events: vec![
                Event {
                    id: "evt:bis-export-controls-2022".to_string(),
                    name: "BIS export controls".to_string(),
                    occurred_at: "2022-10-07".to_string(),
                    description: "US Bureau of Industry and Security export rule.".to_string(),
                    actor_ids: vec!["ent:united-states".to_string()],
                },
                Event {
                    id: "evt:smic-7nm-chip-2023".to_string(),
                    name: "SMIC ships 7nm chip".to_string(),
                    occurred_at: "2023-08-01".to_string(),
                    description: "SMIC fabricates a 7nm-class chip.".to_string(),
                    actor_ids: vec![],
                },
            ],
            sources: vec![source.clone()],
            claims: vec![Claim {
                id: "clm:controls-durably-slow-china".to_string(),
                text: "Export controls durably slow China's semiconductor progress.".to_string(),
                kind: "hypothesis".to_string(),
                subject_ids: vec!["ent:china".to_string()],
                support: Some(0.5),
                support_method: "jev".to_string(),
            }],
            evidence: vec![Evidence {
                id: "evd:smic-7nm-mate-60".to_string(),
                claim_id: "clm:controls-durably-slow-china".to_string(),
                source_id: source.id.clone(),
                stance: "contradicts".to_string(),
                excerpt: "SMIC produced a 7nm-class chip despite controls.".to_string(),
                quality: 0.6,
            }],
            causal_links: vec![CausalLink {
                cause_id: "evt:bis-export-controls-2022".to_string(),
                effect_id: "clm:controls-durably-slow-china".to_string(),
                mechanism: "Restricts access to EUV lithography tools.".to_string(),
                confidence: "medium".to_string(),
            }],
            temporal_relations: vec![TemporalRelation {
                before_id: "evt:bis-export-controls-2022".to_string(),
                after_id: "evt:smic-7nm-chip-2023".to_string(),
                relation: "before".to_string(),
            }],
            gate_log: vec![GateLogEntry {
                gate: "support".to_string(),
                status: "ok".to_string(),
                detail: "1 claim scored".to_string(),
            }],
            dropped_sources: vec![],
        }
    }

    #[test]
    fn valid_payload_passes() {
        assert!(sample_payload().validate().is_ok());
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let mut payload = sample_payload();
        payload.schema_version = 1;
        assert!(matches!(
            payload.validate(),
            Err(PayloadError::UnsupportedSchemaVersion(1))
        ));
    }

    #[test]
    fn empty_question_is_rejected() {
        let mut payload = sample_payload();
        payload.question = "   ".to_string();
        assert!(matches!(
            payload.validate(),
            Err(PayloadError::EmptyQuestion)
        ));
    }

    #[test]
    fn wrong_prefix_for_list_is_rejected() {
        let mut payload = sample_payload();
        payload.entities[0].id = "evt:tsmc".to_string();
        let result = payload.validate();
        assert!(matches!(result, Err(PayloadError::InvalidId(ref id)) if id == "evt:tsmc"));
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let mut payload = sample_payload();
        let duplicate = payload.entities[0].clone();
        payload.entities.push(duplicate);
        let result = payload.validate();
        assert!(matches!(result, Err(PayloadError::DuplicateId(ref id)) if id == "ent:tsmc"));
    }

    #[test]
    fn bad_enum_is_rejected() {
        let mut payload = sample_payload();
        payload.entities[0].kind = "widget".to_string();
        match payload.validate() {
            Err(PayloadError::InvalidEnum { field, value }) => {
                assert_eq!(field, "entities[].kind");
                assert_eq!(value, "widget");
            }
            other => panic!("expected InvalidEnum, got {other:?}"),
        }
    }

    #[test]
    fn quality_out_of_range_is_rejected() {
        let mut payload = sample_payload();
        payload.evidence[0].quality = 1.5;
        match payload.validate() {
            Err(PayloadError::QualityOutOfRange { id, quality }) => {
                assert_eq!(id, "evd:smic-7nm-mate-60");
                assert_eq!(quality, 1.5);
            }
            other => panic!("expected QualityOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn support_out_of_range_is_rejected() {
        let mut payload = sample_payload();
        payload.claims[0].support = Some(1.5);
        match payload.validate() {
            Err(PayloadError::SupportOutOfRange { id, support }) => {
                assert_eq!(id, "clm:controls-durably-slow-china");
                assert_eq!(support, 1.5);
            }
            other => panic!("expected SupportOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn null_support_is_accepted() {
        let mut payload = sample_payload();
        payload.claims[0].support = None;
        payload.claims[0].support_method = "none".to_string();
        assert!(payload.validate().is_ok());
    }

    #[test]
    fn content_hash_mismatch_is_rejected() {
        let mut payload = sample_payload();
        payload.sources[0].content_hash = "0".repeat(64);
        match payload.validate() {
            Err(PayloadError::ContentHashMismatch { id }) => {
                assert_eq!(id, payload.sources[0].id);
            }
            other => panic!("expected ContentHashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn source_id_not_matching_content_addressed_shape_is_rejected() {
        let mut payload = sample_payload();
        payload.sources[0].id = "src:wikipedia-semiconductor-industry-in-china".to_string();
        // referential integrity would also fail once the id changes and
        // evidence still points at the old id; use a payload with no
        // evidence pointing at it to isolate the id-shape check.
        payload.evidence.clear();
        match payload.validate() {
            Err(PayloadError::InvalidSourceId(id)) => {
                assert_eq!(id, "src:wikipedia-semiconductor-industry-in-china");
            }
            other => panic!("expected InvalidSourceId, got {other:?}"),
        }
    }

    #[test]
    fn bad_date_is_rejected() {
        let mut payload = sample_payload();
        payload.events[0].occurred_at = "2022/10/07".to_string();
        match payload.validate() {
            Err(PayloadError::InvalidDate { field, value }) => {
                assert_eq!(field, "events[].occurred_at");
                assert_eq!(value, "2022/10/07");
            }
            other => panic!("expected InvalidDate, got {other:?}"),
        }
    }

    #[test]
    fn dangling_claim_id_is_rejected() {
        let mut payload = sample_payload();
        payload.evidence[0].claim_id = "clm:does-not-exist".to_string();
        match payload.validate() {
            Err(PayloadError::UnknownReference { field, id }) => {
                assert_eq!(field, "claim_id");
                assert_eq!(id, "clm:does-not-exist");
            }
            other => panic!("expected UnknownReference, got {other:?}"),
        }
    }

    #[test]
    fn slugify_collapses_non_alphanumeric_runs_and_trims() {
        assert_eq!(slugify("  TSMC & ASML: EUV!! "), "tsmc-asml-euv");
    }

    /// Returns whether every character in `s` is an ASCII digit or a
    /// lowercase hex letter (`a`-`f`).
    fn is_lowercase_hex(s: &str) -> bool {
        s.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f'))
    }

    #[test]
    fn dossier_id_for_is_stable_and_ignores_surrounding_whitespace() {
        let question = "Can export controls durably slow China's access to advanced \
                         semiconductor manufacturing capability?";
        let id = dossier_id_for(question);
        assert_eq!(id, dossier_id_for(question));
        assert_eq!(id, dossier_id_for(&format!("  {question}  \n")));
    }

    #[test]
    fn dossier_id_for_disambiguates_similar_questions() {
        let semiconductors = dossier_id_for(
            "Can export controls durably slow China's access to advanced semiconductor \
             manufacturing capability?",
        );
        let accelerators = dossier_id_for(
            "Can export controls durably slow China's access to advanced AI accelerators?",
        );
        assert_ne!(semiconductors, accelerators);
    }

    #[test]
    fn dossier_id_for_long_question_stays_within_73_chars_with_hex_hash_suffix() {
        let question = "a".repeat(200);
        let id = dossier_id_for(&question);
        assert!(id.len() <= 73, "id too long: {id} ({} chars)", id.len());
        let (_, hash) = id.rsplit_once('-').expect("id has a `-<hash>` suffix");
        assert_eq!(hash.len(), 8);
        assert!(is_lowercase_hex(hash), "hash not lowercase hex: {hash}");
    }

    #[test]
    fn dossier_id_for_non_latin_question_falls_back_to_q_slug() {
        let question = "هل ستبطئ ضوابط التصدير وصول الصين إلى الرقائق المتقدمة؟";
        let id = dossier_id_for(question);
        let hash = id
            .strip_prefix("dos:q-")
            .unwrap_or_else(|| panic!("id should start with dos:q-, got: {id}"));
        assert_eq!(hash.len(), 8);
        assert!(is_lowercase_hex(hash), "hash not lowercase hex: {hash}");
    }

    #[test]
    fn json_round_trip_preserves_field_names() {
        let payload = sample_payload();
        let value = serde_json::to_value(&payload).expect("serialize payload");
        assert!(value.get("schema_version").is_some());
        assert!(value.get("causal_links").is_some());
        assert!(value.get("temporal_relations").is_some());
        assert!(value.get("gate_log").is_some());
        assert!(value.get("dropped_sources").is_some());
        assert!(value["events"][0].get("occurred_at").is_some());
        assert!(value["events"][0].get("actor_ids").is_some());
        assert!(value["sources"][0].get("content_hash").is_some());
        assert!(value["claims"][0].get("support").is_some());

        let round_tripped: ExtractionPayload =
            serde_json::from_value(value).expect("deserialize payload");
        assert_eq!(payload, round_tripped);
    }

    #[test]
    fn gate_log_and_dropped_sources_default_to_empty_when_absent() {
        // Older/minimal producers may omit these fields; both are `#[serde(default)]`.
        let mut value = serde_json::to_value(sample_payload()).expect("serialize");
        value.as_object_mut().unwrap().remove("gate_log");
        value.as_object_mut().unwrap().remove("dropped_sources");
        let payload: ExtractionPayload = serde_json::from_value(value).expect("deserialize");
        assert!(payload.gate_log.is_empty());
        assert!(payload.dropped_sources.is_empty());
    }

    #[test]
    fn all_item_ids_lists_every_item_in_order() {
        let payload = sample_payload();
        let expected_source_id = payload.sources[0].id.clone();
        assert_eq!(
            payload.all_item_ids(),
            vec![
                "ent:tsmc".to_string(),
                "ent:china".to_string(),
                "ent:united-states".to_string(),
                "evt:bis-export-controls-2022".to_string(),
                "evt:smic-7nm-chip-2023".to_string(),
                expected_source_id,
                "clm:controls-durably-slow-china".to_string(),
                "evd:smic-7nm-mate-60".to_string(),
            ]
        );
    }

    #[test]
    fn history_item_validates_kind_and_question_type() {
        let item = HistoryItem {
            id: "metaculus:41234".to_string(),
            kind: "personal".to_string(),
            title: "Will the ECB cut rates?".to_string(),
            url: "https://www.metaculus.com/questions/41234".to_string(),
            question_type: "binary".to_string(),
            forecast: serde_json::json!(0.42),
            resolution: None,
            resolved_at: None,
            family_id: Some("fam:ecb-rate-decisions".to_string()),
            note: String::new(),
        };
        assert!(item.validate().is_ok());

        let mut bad_kind = item.clone();
        bad_kind.kind = "shadow".to_string();
        assert!(matches!(
            bad_kind.validate(),
            Err(PayloadError::InvalidEnum { field: "kind", .. })
        ));

        let mut bad_type = item.clone();
        bad_type.question_type = "vibes".to_string();
        assert!(matches!(
            bad_type.validate(),
            Err(PayloadError::InvalidEnum {
                field: "question_type",
                ..
            })
        ));

        let mut empty_id = item;
        empty_id.id = String::new();
        assert!(matches!(
            empty_id.validate(),
            Err(PayloadError::EmptyQuestion)
        ));
    }
}
