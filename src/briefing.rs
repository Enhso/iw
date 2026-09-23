//! Briefing synthesis: turns a [`GraphStore`] query result set scoped to one
//! dossier into the 11-section analytical briefing mandated by PRD Section
//! 10, with no forecasting language (Global Constraint 1).

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::store::{
    CausalChainRow, CausalLinkRow, ClaimStanceRow, ConsensusRow, CruxRow, EntityRow, EventRow,
    EvidenceRow, GraphStore, SimilarClaimRow, SimilarEvidenceRow, SourceRow, StoreError,
    TemporalRow,
};

/// The 11 briefing section titles, in the fixed PRD Section 10 order.
pub const SECTION_TITLES: [&str; 11] = [
    "Executive Overview",
    "Situation Summary",
    "Historical Context",
    "Causal Model",
    "Competing Hypotheses",
    "Consensus and Dissent",
    "Crux Analysis",
    "Counterfactual Analysis",
    "Evidence Assessment",
    "Open Questions",
    "Source Appendix",
];

/// Entity kinds that group the "Actors" bullet list in the Situation
/// Summary section, in display order. Kinds absent from a dossier are
/// omitted rather than shown empty.
const ACTOR_KIND_ORDER: [&str; 5] = ["country", "organization", "technology", "policy", "person"];

/// One rendered briefing section: a title from [`SECTION_TITLES`] and its
/// markdown body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub body: String,
}

/// A fully rendered analytical briefing for one dossier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Briefing {
    pub dossier_id: String,
    pub question: String,
    pub sections: Vec<Section>,
    pub markdown: String,
}

/// Every graph and vector query result needed to render a [`Briefing`] for
/// one dossier, gathered by [`build_context`]. The graph fields are scoped
/// to the dossier; `similar_evidence` and `similar_claims` are seeded with
/// the dossier's own question text and searched globally per
/// [`GraphStore::similar_evidence`] and [`GraphStore::similar_claims`].
#[derive(Debug, Clone, PartialEq)]
pub struct BriefingContext {
    pub dossier_id: String,
    pub question: String,
    pub entities: Vec<EntityRow>,
    pub events: Vec<EventRow>,
    pub event_actors: Vec<(String, String)>,
    pub sources: Vec<SourceRow>,
    pub claim_subjects: Vec<(String, String)>,
    pub evidence: Vec<EvidenceRow>,
    pub claims: Vec<ClaimStanceRow>,
    pub causal_links: Vec<CausalLinkRow>,
    pub chains: Vec<CausalChainRow>,
    pub cruxes: Vec<CruxRow>,
    pub consensus: Vec<ConsensusRow>,
    pub temporal: Vec<TemporalRow>,
    pub similar_evidence: Vec<SimilarEvidenceRow>,
    pub similar_claims: Vec<SimilarClaimRow>,
}

/// Gathers every graph and vector query result needed to render a
/// [`Briefing`] for `dossier_id` as of `as_of` (an RFC 3339 UTC string, or
/// [`crate::store::AS_OF_NOW`] for current state).
///
/// # Errors
/// Returns [`StoreError`] if any underlying [`GraphStore`] query fails.
///
/// # Returns
/// `Ok(None)` if `dossier_id` did not exist yet as of `as_of` (i.e.
/// [`GraphStore::dossier_meta`] returns `None`) — either it has never been
/// ingested, or `as_of` predates its first ingest.
pub fn build_context(
    store: &GraphStore,
    dossier_id: &str,
    as_of: &str,
) -> Result<Option<BriefingContext>, StoreError> {
    let Some((question, _created_at)) = store.dossier_meta(dossier_id, as_of)? else {
        return Ok(None);
    };

    Ok(Some(BriefingContext {
        dossier_id: dossier_id.to_string(),
        entities: store.entities(dossier_id, as_of)?,
        events: store.events(dossier_id, as_of)?,
        event_actors: store.event_actors(dossier_id, as_of)?,
        sources: store.sources(dossier_id, as_of)?,
        claim_subjects: store.claim_subjects(dossier_id, as_of)?,
        evidence: store.evidence(dossier_id, as_of)?,
        claims: store.claims_with_stance(dossier_id, as_of)?,
        causal_links: store.causal_links(dossier_id, as_of)?,
        chains: store.causal_chains(dossier_id, as_of)?,
        cruxes: store.cruxes(dossier_id, as_of)?,
        consensus: store.consensus(dossier_id, as_of)?,
        temporal: store.temporal_relations(dossier_id, as_of)?,
        similar_evidence: store.similar_evidence(&question, as_of)?,
        similar_claims: store.similar_claims(&question, as_of)?,
        question,
    }))
}

/// Renders `ctx` into the 11-section [`Briefing`] mandated by PRD Section
/// 10, in [`SECTION_TITLES`] order.
pub fn render(ctx: &BriefingContext) -> Briefing {
    let bodies = [
        render_executive_overview(ctx),
        render_situation_summary(ctx),
        render_historical_context(ctx),
        render_causal_model(ctx),
        render_competing_hypotheses(ctx),
        render_consensus_and_dissent(ctx),
        render_crux_analysis(ctx),
        render_counterfactual_analysis(ctx),
        render_evidence_assessment(ctx),
        render_open_questions(ctx),
        render_source_appendix(ctx),
    ];

    let sections: Vec<Section> = SECTION_TITLES
        .iter()
        .zip(bodies)
        .map(|(title, body)| Section {
            title: (*title).to_string(),
            body,
        })
        .collect();

    let mut markdown = format!("# Briefing: {}\n\n", ctx.question);
    for section in &sections {
        markdown.push_str(&format!("## {}\n\n{}\n\n", section.title, section.body));
    }
    let mut markdown = markdown.trim_end().to_string();
    markdown.push('\n');

    Briefing {
        dossier_id: ctx.dossier_id.clone(),
        question: ctx.question.clone(),
        sections,
        markdown,
    }
}

/// Converts an evidence quality score into the word Global Constraint 1
/// requires in place of the raw number: `"high"` at `quality >= 0.7`,
/// `"medium"` at `quality >= 0.4`, `"low"` otherwise.
pub fn quality_word(quality: f64) -> &'static str {
    if quality >= 0.7 {
        "high"
    } else if quality >= 0.4 {
        "medium"
    } else {
        "low"
    }
}

/// Resolves any entity, event, or claim id in `ctx` to its display name
/// (entity name, event name, or claim text). Falls back to the raw id if
/// no entity, event, or claim in `ctx` has that id.
fn name_of(ctx: &BriefingContext, id: &str) -> String {
    if let Some(entity) = ctx.entities.iter().find(|row| row.id == id) {
        return entity.name.clone();
    }
    if let Some(event) = ctx.events.iter().find(|row| row.id == id) {
        return event.name.clone();
    }
    if let Some(claim) = ctx.claims.iter().find(|row| row.id == id) {
        return claim.text.clone();
    }
    id.to_string()
}

/// Returns every claim in `ctx` with both supporting and contradicting
/// evidence, in `ctx.claims` order. This is the full, uncapped contested
/// set; `ctx.cruxes` is this same set ranked by contestation plus
/// downstream causal reach and capped at 3, so it undercounts whenever
/// more than 3 claims are contested.
fn contested_claims(ctx: &BriefingContext) -> Vec<&ClaimStanceRow> {
    ctx.claims
        .iter()
        .filter(|claim| claim.supports > 0 && claim.contradicts > 0)
        .collect()
}

/// Resolves a source id in `ctx` to its display title. Falls back to the
/// raw id if no source in `ctx` has that id.
fn source_title(ctx: &BriefingContext, source_id: &str) -> String {
    ctx.sources
        .iter()
        .find(|row| row.id == source_id)
        .map_or_else(|| source_id.to_string(), |row| row.title.clone())
}

/// Joins `lines` as `"- {line}"` bullets separated by newlines, or returns
/// `placeholder` verbatim if `lines` is empty. Shared by every section that
/// renders a possibly-empty list per the "No `<thing>` recorded." rule.
fn bullets_or(lines: &[String], placeholder: &str) -> String {
    if lines.is_empty() {
        placeholder.to_string()
    } else {
        lines
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Renders the Executive Overview section.
fn render_executive_overview(ctx: &BriefingContext) -> String {
    let crux_lines: Vec<String> = ctx
        .cruxes
        .iter()
        .take(3)
        .map(|crux| crux.text.clone())
        .collect();
    format!(
        "{question}\n\nThis dossier holds {entities} entities, {events} events, {claims} \
claims, {evidence} evidence items from {sources} sources.\n\nThe situation turns on:\n{cruxes}\
\n\n{consensus} claims have consensus support and {contested} are contested.",
        question = ctx.question,
        entities = ctx.entities.len(),
        events = ctx.events.len(),
        claims = ctx.claims.len(),
        evidence = ctx.evidence.len(),
        sources = ctx.sources.len(),
        cruxes = bullets_or(&crux_lines, "No cruxes recorded."),
        consensus = ctx.consensus.len(),
        contested = contested_claims(ctx).len(),
    )
}

/// Renders the Situation Summary section.
fn render_situation_summary(ctx: &BriefingContext) -> String {
    let evidence_lines: Vec<String> = ctx
        .similar_evidence
        .iter()
        .take(8)
        .map(|row| {
            format!(
                "({stance}, {word}) {excerpt} [{source}]",
                stance = row.stance,
                word = quality_word(row.quality),
                excerpt = row.excerpt,
                source = row.source_title,
            )
        })
        .collect();

    let claim_lines: Vec<String> = ctx
        .similar_claims
        .iter()
        .take(5)
        .map(|row| row.text.clone())
        .collect();

    let mut actor_lines: Vec<String> = Vec::new();
    for kind in ACTOR_KIND_ORDER {
        let names: Vec<&str> = ctx
            .entities
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.name.as_str())
            .collect();
        if !names.is_empty() {
            actor_lines.push(format!("{kind}: {}", names.join(", ")));
        }
    }

    format!(
        "Evidence closest to the question:\n{evidence}\n\nClaims closest to the question:\n\
{claims}\n\nActors:\n{actors}",
        evidence = bullets_or(&evidence_lines, "No evidence recorded."),
        claims = bullets_or(&claim_lines, "No claims recorded."),
        actors = bullets_or(&actor_lines, "No actors recorded."),
    )
}

/// Renders the Historical Context section.
fn render_historical_context(ctx: &BriefingContext) -> String {
    // `ctx.events` is already ordered by occurred_at then name (see
    // `schema::EVENTS`), so the timeline bullets come out date-ascending
    // without an extra sort here.
    let timeline_lines: Vec<String> = ctx
        .events
        .iter()
        .map(|event| {
            let when = if event.occurred_at.is_empty() {
                "undated"
            } else {
                &event.occurred_at
            };
            let actors: Vec<String> = ctx
                .event_actors
                .iter()
                .filter(|(event_id, _)| event_id == &event.id)
                .map(|(_, entity_id)| name_of(ctx, entity_id))
                .collect();
            let actors = if actors.is_empty() {
                "none".to_string()
            } else {
                actors.join(", ")
            };
            format!(
                "{when} - {name}: {description} (actors: {actors})",
                name = event.name,
                description = event.description,
            )
        })
        .collect();

    let temporal_lines: Vec<String> = ctx
        .temporal
        .iter()
        .map(|relation| {
            format!(
                "{before} {relation} {after}",
                before = name_of(ctx, &relation.before_id),
                relation = relation.relation,
                after = name_of(ctx, &relation.after_id),
            )
        })
        .collect();

    format!(
        "Timeline:\n{timeline}\n\nTemporal relations:\n{temporal}",
        timeline = bullets_or(&timeline_lines, "No events recorded."),
        temporal = bullets_or(&temporal_lines, "No temporal relations recorded."),
    )
}

/// Renders the Causal Model section.
fn render_causal_model(ctx: &BriefingContext) -> String {
    let mechanism_lines: Vec<String> = ctx
        .causal_links
        .iter()
        .map(|link| {
            format!(
                "{cause} -> {effect}: {mechanism} (confidence: {confidence})",
                cause = name_of(ctx, &link.cause_id),
                effect = name_of(ctx, &link.effect_id),
                mechanism = link.mechanism,
                confidence = link.confidence,
            )
        })
        .collect();

    // `ctx.chains` is already ordered longest-first (see
    // `schema::CAUSAL_CHAINS`); de-duplicate by path before taking the top
    // 5 so a chain is never counted twice under two different labels.
    let mut seen_paths: HashSet<Vec<String>> = HashSet::new();
    let chain_lines: Vec<String> = ctx
        .chains
        .iter()
        .filter(|chain| seen_paths.insert(chain.path.clone()))
        .take(5)
        .map(|chain| {
            chain
                .path
                .iter()
                .map(|id| name_of(ctx, id))
                .collect::<Vec<_>>()
                .join(" -> ")
        })
        .collect();

    format!(
        "Mechanisms:\n{mechanisms}\n\nLongest causal chains:\n{chains}",
        mechanisms = bullets_or(&mechanism_lines, "No causal links recorded."),
        chains = bullets_or(&chain_lines, "No causal chains recorded."),
    )
}

/// Renders the Competing Hypotheses section.
fn render_competing_hypotheses(ctx: &BriefingContext) -> String {
    let hypotheses: Vec<&ClaimStanceRow> = ctx
        .claims
        .iter()
        .filter(|claim| claim.kind == "hypothesis")
        .collect();

    if hypotheses.is_empty() {
        return "No hypotheses recorded.".to_string();
    }

    hypotheses
        .iter()
        .enumerate()
        .map(|(n, claim)| {
            let concerns = claim_subject_names(ctx, &claim.id);
            let concerns = if concerns.is_empty() {
                "none recorded.".to_string()
            } else {
                concerns.join(", ")
            };
            let supporting = evidence_bullets(ctx, &claim.id, "supports");
            let contradicting = evidence_bullets(ctx, &claim.id, "contradicts");
            format!(
                "### H{n}: {text}\n\nConcerns: {concerns}\n\nSupporting evidence:\n{supporting}\
\n\nContradicting evidence:\n{contradicting}",
                n = n + 1,
                text = claim.text,
                supporting = bullets_or(&supporting, "None recorded."),
                contradicting = bullets_or(&contradicting, "None recorded."),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Returns the `name_of` resolutions of `claim_id`'s subject ids from
/// `ctx.claim_subjects`, in `claim_subjects` order.
fn claim_subject_names(ctx: &BriefingContext, claim_id: &str) -> Vec<String> {
    ctx.claim_subjects
        .iter()
        .filter(|(claim, _)| claim == claim_id)
        .map(|(_, subject_id)| name_of(ctx, subject_id))
        .collect()
}

/// Returns `- <excerpt> [<source title>, <quality word>]` lines (without
/// the leading `- `, added later by [`bullets_or`]) for every evidence row
/// on `claim_id` with the given `stance`.
fn evidence_bullets(ctx: &BriefingContext, claim_id: &str, stance: &str) -> Vec<String> {
    ctx.evidence
        .iter()
        .filter(|evidence| evidence.claim_id == claim_id && evidence.stance == stance)
        .map(|evidence| {
            format!(
                "{excerpt} [{source}, {word}]",
                excerpt = evidence.excerpt,
                source = source_title(ctx, &evidence.source_id),
                word = quality_word(evidence.quality),
            )
        })
        .collect()
}

/// Renders the Consensus and Dissent section.
fn render_consensus_and_dissent(ctx: &BriefingContext) -> String {
    let consensus_lines: Vec<String> = ctx
        .consensus
        .iter()
        .map(|row| format!("{} ({} sources)", row.text, row.sources))
        .collect();

    let dissent_lines: Vec<String> = ctx
        .claims
        .iter()
        .filter(|claim| claim.contradicts > 0)
        .map(|claim| {
            let sources: Vec<String> = evidence_sources(ctx, &claim.id, "contradicts");
            format!("{} - contradicted by: {}", claim.text, sources.join(", "))
        })
        .collect();

    let minority_lines: Vec<String> = ctx
        .cruxes
        .first()
        .map(|crux| {
            ctx.evidence
                .iter()
                .filter(|evidence| evidence.claim_id == crux.id && evidence.stance == "contradicts")
                .map(|evidence| evidence.excerpt.clone())
                .collect()
        })
        .unwrap_or_default();

    format!(
        "Consensus (supported by at least two independent sources, uncontradicted):\n{consensus}\
\n\nDissent (claims with contradicting evidence):\n{dissent}\n\nMinority viewpoints worth \
considering:\n{minority}",
        consensus = bullets_or(&consensus_lines, "No consensus claims recorded."),
        dissent = bullets_or(&dissent_lines, "No dissenting claims recorded."),
        minority = bullets_or(&minority_lines, "None recorded."),
    )
}

/// Returns the source titles (in evidence-row order) for every evidence row
/// on `claim_id` with the given `stance`.
fn evidence_sources(ctx: &BriefingContext, claim_id: &str, stance: &str) -> Vec<String> {
    ctx.evidence
        .iter()
        .filter(|evidence| evidence.claim_id == claim_id && evidence.stance == stance)
        .map(|evidence| source_title(ctx, &evidence.source_id))
        .collect()
}

/// Renders the Crux Analysis section.
fn render_crux_analysis(ctx: &BriefingContext) -> String {
    if ctx.cruxes.is_empty() {
        return "No cruxes recorded.".to_string();
    }

    ctx.cruxes
        .iter()
        .enumerate()
        .map(|(n, crux)| {
            let downstream: Vec<String> = ctx
                .causal_links
                .iter()
                .filter(|link| link.cause_id == crux.id)
                .map(|link| name_of(ctx, &link.effect_id))
                .collect();
            format!(
                "### Crux {n}: {text}\n\n- Supporting evidence items: {supports}\n- \
Contradicting evidence items: {contradicts}\n- Downstream effects in the causal graph: \
{downstream_count}\n\nDepends on this crux:\n{deps}",
                n = n + 1,
                text = crux.text,
                supports = crux.supports,
                contradicts = crux.contradicts,
                downstream_count = crux.downstream,
                deps = bullets_or(&downstream, "No modelled downstream effects."),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Renders the Counterfactual Analysis section.
fn render_counterfactual_analysis(ctx: &BriefingContext) -> String {
    let crux_lines: Vec<String> = ctx
        .cruxes
        .iter()
        .map(|crux| {
            let mut seen: HashSet<String> = HashSet::new();
            let mut downstream: Vec<String> = Vec::new();
            for chain in ctx.chains.iter().filter(|chain| chain.start == crux.id) {
                for node in chain.path.iter().skip(1) {
                    if seen.insert(node.clone()) {
                        downstream.push(name_of(ctx, node));
                    }
                }
            }
            let downstream = if downstream.is_empty() {
                "nothing modelled".to_string()
            } else {
                downstream.join(", ")
            };
            format!(
                "If it were false that \"{text}\": the following would not follow: {downstream}",
                text = crux.text,
            )
        })
        .collect();

    let mut turning_points: Vec<String> = Vec::new();
    let mut seen_causes: HashSet<String> = HashSet::new();
    for link in &ctx.causal_links {
        if !ctx.events.iter().any(|event| event.id == link.cause_id) {
            continue;
        }
        if !seen_causes.insert(link.cause_id.clone()) {
            continue;
        }
        let effects: Vec<String> = ctx
            .causal_links
            .iter()
            .filter(|other| other.cause_id == link.cause_id)
            .map(|other| name_of(ctx, &other.effect_id))
            .collect();
        turning_points.push(format!(
            "Had \"{name}\" not occurred: {effects}",
            name = name_of(ctx, &link.cause_id),
            effects = effects.join(", "),
        ));
    }

    format!(
        "{cruxes}\n\nMissed turning points:\n{turning}",
        cruxes = bullets_or(&crux_lines, "No cruxes recorded."),
        turning = bullets_or(&turning_points, "No missed turning points recorded."),
    )
}

/// Renders the Evidence Assessment section.
fn render_evidence_assessment(ctx: &BriefingContext) -> String {
    let mut provider_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for source in &ctx.sources {
        *provider_counts.entry(source.provider.as_str()).or_insert(0) += 1;
    }
    let provider_lines: Vec<String> = provider_counts
        .iter()
        .map(|(provider, count)| format!("{provider}: {count}"))
        .collect();

    let (mut high, mut medium, mut low) = (0usize, 0usize, 0usize);
    let (mut supporting, mut contradicting) = (0usize, 0usize);
    for evidence in &ctx.evidence {
        match quality_word(evidence.quality) {
            "high" => high += 1,
            "medium" => medium += 1,
            _ => low += 1,
        }
        if evidence.stance == "supports" {
            supporting += 1;
        } else if evidence.stance == "contradicts" {
            contradicting += 1;
        }
    }

    let blind_spots: Vec<String> = ctx
        .claims
        .iter()
        .filter(|claim| claim.supports == 0 && claim.contradicts == 0)
        .map(|claim| claim.text.clone())
        .collect();

    let bias_line = if ctx.sources.is_empty() {
        "No sources recorded."
    } else if ctx
        .sources
        .iter()
        .all(|source| source.provider == "wikipedia" || source.provider == "arxiv")
    {
        "All sources are secondary (encyclopedic or preprint); no primary documents, official \
statements, or industry data were consulted."
    } else {
        "No systematic source bias identified."
    };

    format!(
        "Sources by provider:\n{providers}\n\nEvidence quality:\n- high: {high}, medium: \
{medium}, low: {low}\n\nStance balance:\n- supporting: {supporting}, contradicting: \
{contradicting}\n\nBlind spots:\n{blind}\n\nPotential biases:\n{bias_line}",
        providers = bullets_or(&provider_lines, "No sources recorded."),
        blind = bullets_or(&blind_spots, "None recorded."),
    )
}

/// Renders the Open Questions section.
fn render_open_questions(ctx: &BriefingContext) -> String {
    let mut questions: Vec<String> = ctx
        .cruxes
        .iter()
        .map(|crux| {
            format!(
                "Which way does \"{}\" resolve, and what evidence would settle it?",
                crux.text
            )
        })
        .collect();
    questions.extend(
        ctx.claims
            .iter()
            .filter(|claim| claim.supports == 0 && claim.contradicts == 0)
            .map(|claim| format!("\"{}\" is unverified: no evidence recorded.", claim.text)),
    );

    // One bullet per contested claim (uncapped), not per crux: `ctx.cruxes`
    // is the same set ranked and capped at 3, which would undercount this
    // list whenever more than 3 claims are contested.
    let info_lines: Vec<String> = contested_claims(ctx)
        .iter()
        .map(|claim| {
            let mut source_ids: Vec<String> = ctx
                .evidence
                .iter()
                .filter(|evidence| evidence.claim_id == claim.id)
                .map(|evidence| evidence.source_id.clone())
                .collect();
            source_ids.sort_unstable();
            source_ids.dedup();
            let titles: Vec<String> = source_ids
                .iter()
                .map(|source_id| source_title(ctx, source_id))
                .collect();
            format!(
                "New evidence on \"{text}\" from a source other than: {sources}",
                text = claim.text,
                sources = titles.join(", "),
            )
        })
        .collect();

    format!(
        "{questions}\n\nInformation that would matter most:\n{info}",
        questions = bullets_or(&questions, "No open questions recorded."),
        info = bullets_or(&info_lines, "None recorded."),
    )
}

/// Renders the Source Appendix section.
///
/// Relies on `ctx.sources` already being ordered by provider then title
/// (see `schema::SOURCES`), so no re-sort is needed here.
fn render_source_appendix(ctx: &BriefingContext) -> String {
    if ctx.sources.is_empty() {
        return "No sources recorded.".to_string();
    }

    let entries: Vec<String> = ctx
        .sources
        .iter()
        .enumerate()
        .map(|(n, source)| {
            let published = if source.published.is_empty() {
                "n/a"
            } else {
                &source.published
            };
            format!(
                "{n}. {title} ({provider}, published {published}, retrieved {retrieved}) - {url}",
                n = n + 1,
                title = source.title,
                provider = source.provider,
                retrieved = source.retrieved_at,
                url = source.url,
            )
        })
        .collect();

    let ids: Vec<&str> = ctx
        .sources
        .iter()
        .map(|source| source.id.as_str())
        .collect();

    format!("{}\n\nIdentifiers: {}", entries.join("\n"), ids.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        dossier_id_for, sha256_hex, CausalLink, Claim, Entity, Event, Evidence, ExtractionPayload,
        Source, TemporalRelation,
    };
    use crate::store::{GraphStore, AS_OF_NOW};

    /// Builds a v2 [`Source`] for `title`/`url` whose id and `content_hash`
    /// are correctly derived from `content`, so ad hoc test payloads don't
    /// have to reproduce that arithmetic inline.
    fn make_source(provider: &str, title: &str, url: &str, content: &str) -> Source {
        let url_hash = &sha256_hex(url)[..16];
        Source {
            id: format!("src:{provider}:{url_hash}"),
            title: title.to_string(),
            url: url.to_string(),
            provider: provider.to_string(),
            published: String::new(),
            retrieved_at: CREATED_AT.to_string(),
            content_hash: sha256_hex(content),
            content: content.to_string(),
        }
    }

    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/rust/semiconductor_v2.json"
    ));
    const CREATED_AT: &str = "2026-09-14T00:00:00Z";

    fn fixture_payload() -> ExtractionPayload {
        serde_json::from_str(FIXTURE).expect("fixture payload parses")
    }

    /// Ingests the fixture into a fresh in-memory store and renders its
    /// briefing.
    fn fixture_briefing() -> (ExtractionPayload, Briefing) {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");
        let ctx = build_context(&store, &report.dossier_id, AS_OF_NOW)
            .expect("build_context")
            .expect("dossier exists");
        let briefing = render(&ctx);
        (payload, briefing)
    }

    /// Manual scanner for the Global Constraint 1 regex `\d+\s*%`: a run of
    /// ASCII digits followed by optional whitespace then a percent sign.
    /// No regex crate per the task brief.
    fn contains_percent_after_digits(text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        for start in 0..chars.len() {
            if !chars[start].is_ascii_digit() {
                continue;
            }
            let mut idx = start + 1;
            while idx < chars.len() && chars[idx].is_ascii_digit() {
                idx += 1;
            }
            while idx < chars.len() && chars[idx].is_whitespace() {
                idx += 1;
            }
            if idx < chars.len() && chars[idx] == '%' {
                return true;
            }
        }
        false
    }

    #[test]
    fn renders_exactly_eleven_sections_in_prd_order() {
        let (_, briefing) = fixture_briefing();
        assert_eq!(briefing.sections.len(), 11);
        let titles: Vec<&str> = briefing.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, SECTION_TITLES.to_vec());
    }

    #[test]
    fn markdown_contains_every_section_heading_in_ascending_order() {
        let (_, briefing) = fixture_briefing();
        let mut last_offset = 0usize;
        for title in SECTION_TITLES {
            let heading = format!("## {title}");
            let offset = briefing
                .markdown
                .find(&heading)
                .unwrap_or_else(|| panic!("markdown missing heading: {heading}"));
            assert!(offset >= last_offset, "heading {heading} out of order");
            last_offset = offset;
        }
    }

    #[test]
    fn markdown_has_no_forecasting_language() {
        let (_, briefing) = fixture_briefing();
        assert!(
            !contains_percent_after_digits(&briefing.markdown),
            "markdown matched the forbidden \\d+\\s*% pattern"
        );
        let lower = briefing.markdown.to_lowercase();
        assert!(
            !lower.contains("probability"),
            "markdown contains 'probability'"
        );
        assert!(
            !lower.contains("likelihood"),
            "markdown contains 'likelihood'"
        );
    }

    #[test]
    fn crux_analysis_names_every_crux_text() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");
        let cruxes = store.cruxes(&report.dossier_id, AS_OF_NOW).expect("cruxes");
        assert!(!cruxes.is_empty());

        let ctx = build_context(&store, &report.dossier_id, AS_OF_NOW)
            .expect("build_context")
            .expect("dossier exists");
        let briefing = render(&ctx);
        let crux_section = &briefing.sections[6];
        assert_eq!(crux_section.title, "Crux Analysis");
        for crux in &cruxes {
            assert!(
                crux_section.body.contains(&crux.text),
                "Crux Analysis missing crux text: {}",
                crux.text
            );
        }
    }

    #[test]
    fn competing_hypotheses_has_one_heading_per_hypothesis() {
        let (payload, briefing) = fixture_briefing();
        let hypothesis_count = payload
            .claims
            .iter()
            .filter(|claim| claim.kind == "hypothesis")
            .count();
        assert_eq!(hypothesis_count, 3);

        let section = &briefing.sections[4];
        assert_eq!(section.title, "Competing Hypotheses");
        let heading_count = section.body.matches("### H").count();
        assert_eq!(heading_count, hypothesis_count);
    }

    #[test]
    fn situation_summary_lists_claims_closest_to_the_question() {
        let (payload, briefing) = fixture_briefing();
        let section = &briefing.sections[1];
        assert_eq!(section.title, "Situation Summary");
        assert!(section.body.contains("Claims closest to the question:"));

        let hypothesis_texts: Vec<&str> = payload
            .claims
            .iter()
            .filter(|claim| claim.kind == "hypothesis")
            .map(|claim| claim.text.as_str())
            .collect();
        assert!(
            hypothesis_texts
                .iter()
                .any(|text| section.body.contains(text)),
            "Situation Summary should contain at least one hypothesis text"
        );
    }

    #[test]
    fn competing_hypotheses_names_concerns_from_claim_subjects() {
        let (_, briefing) = fixture_briefing();
        let section = &briefing.sections[4];
        assert_eq!(section.title, "Competing Hypotheses");

        let heading = "### H2: Export controls durably slow China's access to advanced \
semiconductor manufacturing capability, rather than merely delaying it.";
        let start = section
            .body
            .find(heading)
            .unwrap_or_else(|| panic!("missing heading for clm:controls-durably-slow-china"));
        let concerns_line = section.body[start..]
            .lines()
            .find(|line| line.starts_with("Concerns:"))
            .expect("Concerns line present under the heading");
        assert!(
            concerns_line.contains("China"),
            "concerns line: {concerns_line}"
        );
        assert!(
            concerns_line.contains("United States"),
            "concerns line: {concerns_line}"
        );
    }

    #[test]
    fn source_appendix_contains_every_source_url() {
        let (payload, briefing) = fixture_briefing();
        let section = &briefing.sections[10];
        assert_eq!(section.title, "Source Appendix");
        for source in &payload.sources {
            assert!(
                section.body.contains(&source.url),
                "Source Appendix missing url: {}",
                source.url
            );
        }
    }

    #[test]
    fn historical_context_lists_events_in_ascending_date_order() {
        let (_, briefing) = fixture_briefing();
        let section = &briefing.sections[2];
        assert_eq!(section.title, "Historical Context");

        let expected_order = [
            "Dutch government withholds ASML EUV export license to China",
            "BIS publishes October 2022 export control rule",
            "Huawei launches Mate 60 Pro with SMIC 7-nanometer-class chip",
            "BIS publishes October 2023 export control rule update",
            "BIS publishes December 2024 export control rule update",
        ];
        let mut last_offset = 0usize;
        for name in expected_order {
            let offset = section
                .body
                .find(name)
                .unwrap_or_else(|| panic!("Historical Context missing event: {name}"));
            assert!(offset >= last_offset, "event {name} out of date order");
            last_offset = offset;
        }
    }

    #[test]
    fn counterfactual_analysis_contains_if_it_were_false_that() {
        let (_, briefing) = fixture_briefing();
        let section = &briefing.sections[7];
        assert_eq!(section.title, "Counterfactual Analysis");
        assert!(section.body.contains("If it were false that"));
    }

    #[test]
    fn quality_word_boundaries() {
        assert_eq!(quality_word(0.7), "high");
        assert_eq!(quality_word(0.69), "medium");
        assert_eq!(quality_word(0.4), "medium");
        assert_eq!(quality_word(0.39), "low");
    }

    #[test]
    fn build_context_on_unknown_dossier_returns_none() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let result = build_context(&store, "dos:does-not-exist", AS_OF_NOW).expect("build_context");
        assert!(result.is_none());
    }

    #[test]
    fn minimal_dossier_still_renders_eleven_non_empty_sections() {
        let payload = ExtractionPayload {
            schema_version: 2,
            question: "Does a minimal dossier still render a full briefing?".to_string(),
            entities: vec![Entity {
                id: "ent:only-one".to_string(),
                name: "Only One".to_string(),
                kind: "organization".to_string(),
                description: "A minimal handcrafted entity.".to_string(),
            }],
            events: vec![],
            sources: vec![],
            claims: vec![],
            evidence: vec![],
            causal_links: vec![],
            temporal_relations: vec![],
            gate_log: vec![],
            dropped_sources: vec![],
        };
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest minimal payload");

        let dossier_id = dossier_id_for(&payload.question);
        let ctx = build_context(&store, &dossier_id, AS_OF_NOW)
            .expect("build_context")
            .expect("dossier exists");
        let briefing = render(&ctx);

        assert_eq!(briefing.sections.len(), 11);
        for section in &briefing.sections {
            assert!(
                !section.body.trim().is_empty(),
                "{} body is empty",
                section.title
            );
        }
        let placeholder_count = briefing
            .sections
            .iter()
            .filter(|section| section.body.contains("No "))
            .count();
        assert!(
            placeholder_count > 0,
            "expected at least one 'No ...' placeholder"
        );

        let evidence_assessment = &briefing.sections[8];
        assert_eq!(evidence_assessment.title, "Evidence Assessment");
        assert!(
            evidence_assessment
                .body
                .contains("Potential biases:\nNo sources recorded."),
            "a zero-source dossier's bias line should read 'No sources recorded.': {}",
            evidence_assessment.body
        );
    }

    /// Contested-claim count must come from the full `supports > 0 &&
    /// contradicts > 0` set on `ctx.claims`, not from `ctx.cruxes`, which is
    /// that same set ranked and capped at 3 by the CRUXES query. Builds a
    /// dossier with 4 contested claims and no causal links, so every crux's
    /// score is `supports + contradicts` with no downstream tie-breaker
    /// needed, and checks the two counts diverge as expected.
    #[test]
    fn contested_claim_count_is_uncapped_unlike_crux_list() {
        let entity = Entity {
            id: "ent:only-one".to_string(),
            name: "Only One".to_string(),
            kind: "organization".to_string(),
            description: "A minimal handcrafted entity.".to_string(),
        };
        let source = make_source(
            "wikipedia",
            "Only Source",
            "https://example.com/only-source",
            "Only source content for the contested-claim-count test.",
        );

        let mut claims = Vec::new();
        let mut evidence = Vec::new();
        for n in 1..=4 {
            let claim_id = format!("clm:contested-{n}");
            claims.push(Claim {
                id: claim_id.clone(),
                text: format!("Contested claim number {n}."),
                kind: "hypothesis".to_string(),
                subject_ids: vec![entity.id.clone()],
                support: Some(0.5),
                support_method: "jev".to_string(),
            });
            evidence.push(Evidence {
                id: format!("evd:supports-{n}"),
                claim_id: claim_id.clone(),
                source_id: source.id.clone(),
                stance: "supports".to_string(),
                excerpt: format!("Supporting excerpt for claim {n}."),
                quality: 0.5,
            });
            evidence.push(Evidence {
                id: format!("evd:contradicts-{n}"),
                claim_id: claim_id.clone(),
                source_id: source.id.clone(),
                stance: "contradicts".to_string(),
                excerpt: format!("Contradicting excerpt for claim {n}."),
                quality: 0.5,
            });
        }

        let payload = ExtractionPayload {
            schema_version: 2,
            question: "Are there more than 3 contested claims in this dossier?".to_string(),
            entities: vec![entity],
            events: vec![],
            sources: vec![source],
            claims,
            evidence,
            causal_links: vec![],
            temporal_relations: vec![],
            gate_log: vec![],
            dropped_sources: vec![],
        };

        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest payload");

        let cruxes = store.cruxes(&report.dossier_id, AS_OF_NOW).expect("cruxes");
        assert_eq!(cruxes.len(), 3, "CRUXES query should cap at 3");

        let ctx = build_context(&store, &report.dossier_id, AS_OF_NOW)
            .expect("build_context")
            .expect("dossier exists");
        let briefing = render(&ctx);

        let overview = &briefing.sections[0];
        assert_eq!(overview.title, "Executive Overview");
        assert!(
            overview.body.contains("4 are contested."),
            "Executive Overview should report the uncapped count of 4, got: {}",
            overview.body
        );

        let open_questions = &briefing.sections[9];
        assert_eq!(open_questions.title, "Open Questions");
        let info_bullet_count = open_questions.body.matches("New evidence on").count();
        assert_eq!(
            info_bullet_count, 4,
            "Open Questions should have one 'New evidence on' bullet per contested claim"
        );

        let crux_analysis = &briefing.sections[6];
        assert_eq!(crux_analysis.title, "Crux Analysis");
        let crux_heading_count = crux_analysis.body.matches("### Crux").count();
        assert!(
            crux_heading_count <= 3,
            "Crux Analysis should stay capped at 3 headings, got {crux_heading_count}"
        );
    }

    /// Ingests a handcrafted dossier B that re-declares one of dossier A's
    /// events and one of A's claims (identical stored fields), and touches
    /// those shared ids with B-only edges, including a quality-0.95
    /// contradicting evidence item whose excerpt is set to A's own question
    /// text verbatim so it is guaranteed to be the globally nearest
    /// evidence row for A's Situation Summary query.
    #[test]
    fn other_dossier_rows_never_leak_into_briefing() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let payload_a = fixture_payload();
        let report_a = store
            .ingest(&payload_a, None, CREATED_AT)
            .expect("ingest A");
        let ctx_before = build_context(&store, &report_a.dossier_id, AS_OF_NOW)
            .expect("build_context before B")
            .expect("dossier A exists");
        let briefing_before = render(&ctx_before);

        let b_only_source = make_source(
            "wikipedia",
            "B Only Source",
            "https://example.com/b-only-source",
            "Dossier B's own source content for the cross-dossier leak test.",
        );
        let payload_b = ExtractionPayload {
            schema_version: 2,
            question: "Did the October 2022 rule directly cause the B-only claim in this \
                        handcrafted dossier?"
                .to_string(),
            entities: vec![Entity {
                id: "ent:b-only-actor".to_string(),
                name: "B Only Actor".to_string(),
                kind: "organization".to_string(),
                description: "A dossier-B-only entity for the cross-dossier leak test.".to_string(),
            }],
            events: vec![
                Event {
                    id: "evt:bis-export-controls-2022".to_string(),
                    name: "BIS publishes October 2022 export control rule".to_string(),
                    occurred_at: "2022-10-07".to_string(),
                    description: "The Bureau of Industry and Security issued a rule imposing \
                                   license requirements on exports to China of advanced logic \
                                   chips, high-bandwidth memory, and the equipment used to \
                                   manufacture them."
                        .to_string(),
                    actor_ids: vec!["ent:b-only-actor".to_string()],
                },
                Event {
                    id: "evt:b-only-event".to_string(),
                    name: "B Only Event".to_string(),
                    occurred_at: "2024-01-01".to_string(),
                    description: "A dossier-B-only event for the cross-dossier leak test."
                        .to_string(),
                    actor_ids: vec![],
                },
            ],
            sources: vec![b_only_source.clone()],
            claims: vec![Claim {
                id: "clm:controls-durably-slow-china".to_string(),
                text: "Export controls durably slow China's access to advanced semiconductor \
                       manufacturing capability, rather than merely delaying it."
                    .to_string(),
                kind: "hypothesis".to_string(),
                subject_ids: vec!["ent:b-only-actor".to_string()],
                support: Some(0.3),
                support_method: "jev".to_string(),
            }],
            evidence: vec![Evidence {
                id: "evd:b-only-evidence".to_string(),
                claim_id: "clm:controls-durably-slow-china".to_string(),
                source_id: b_only_source.id.clone(),
                stance: "contradicts".to_string(),
                excerpt: payload_a.question.clone(),
                quality: 0.95,
            }],
            causal_links: vec![CausalLink {
                cause_id: "evt:bis-export-controls-2022".to_string(),
                effect_id: "clm:controls-durably-slow-china".to_string(),
                mechanism: "Dossier B's own causal link from the shared event to the shared \
                            claim."
                    .to_string(),
                confidence: "low".to_string(),
            }],
            temporal_relations: vec![TemporalRelation {
                before_id: "evt:bis-export-controls-2022".to_string(),
                after_id: "evt:b-only-event".to_string(),
                relation: "before".to_string(),
            }],
            gate_log: vec![],
            dropped_sources: vec![],
        };
        payload_b.validate().expect("payload B is valid");
        store
            .ingest(&payload_b, None, CREATED_AT)
            .expect("ingest B");

        let ctx_after = build_context(&store, &report_a.dossier_id, AS_OF_NOW)
            .expect("build_context after B")
            .expect("dossier A still exists");
        let briefing_after = render(&ctx_after);

        for (before, after) in briefing_before
            .sections
            .iter()
            .zip(briefing_after.sections.iter())
        {
            assert_eq!(before.title, after.title);
            if before.title == "Situation Summary" {
                continue;
            }
            assert_eq!(
                before.body, after.body,
                "section {} changed after ingesting an unrelated dossier B",
                before.title
            );
        }

        let source_appendix_start = briefing_after
            .markdown
            .find("## Source Appendix")
            .expect("Source Appendix heading present");
        let before_appendix = &briefing_after.markdown[..source_appendix_start];
        for needle in ["ent:", "evt:", "clm:", "evd:", "src:"] {
            assert!(
                !before_appendix.contains(needle),
                "markdown before Source Appendix contains raw id substring {needle:?}"
            );
        }

        let situation_summary = &briefing_after.sections[1];
        assert_eq!(situation_summary.title, "Situation Summary");
        if situation_summary.body.contains(&payload_a.question) {
            assert!(
                situation_summary.body.contains("(contradicts, high)"),
                "B's evidence should render with quality word 'high': {}",
                situation_summary.body
            );
            assert!(
                situation_summary.body.contains("[B Only Source]"),
                "B's evidence should render with B's own source title: {}",
                situation_summary.body
            );
        }
    }
}
