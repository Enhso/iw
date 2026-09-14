//! mnestic Datalog schema DDL, the atomic ingestion script, and every named
//! query used by [`crate::store::GraphStore`].
//!
//! Every constant in this module is transcribed verbatim (modulo line
//! wrapping to respect the 100-column limit; mnestic's script parser
//! ignores insignificant whitespace) from the "mnestic schema", "Ingestion
//! pattern", and "Datalog queries" sections of `docs/plan-phase1.md`.

/// Creates every mnestic relation and both HNSW vector indexes used by this
/// store, in one chained script.
///
/// Run only when `::relations` does not already list a relation named
/// `claim` (see [`crate::store::GraphStore::init_schema`]): re-running
/// `::hnsw create` on an existing index fails with `index_already_exists`.
pub const SCHEMA_DDL: &str = r#"
{ :create dossier {id: String => question: String, created_at: String} }
{ :create dossier_item {dossier_id: String, item_id: String} }
{ :create entity {id: String => name: String, kind: String, description: String} }
{ :create event {id: String => name: String, occurred_at: String, description: String} }
{ :create event_actor {event_id: String, entity_id: String} }
{ :create source {id: String => title: String, url: String, provider: String,
  published: String, retrieved_at: String} }
{ :create claim {id: String => text: String, kind: String, embedding: <F32; 256>} }
{ :create claim_subject {claim_id: String, subject_id: String} }
{ :create evidence {id: String => claim_id: String, source_id: String, stance: String,
  excerpt: String, quality: Float, embedding: <F32; 256>} }
{ :create causal_link {cause_id: String, effect_id: String => mechanism: String,
  confidence: String} }
{ :create temporal_relation {before_id: String, after_id: String => relation: String} }
{ ::hnsw create claim:claim_vec {dim: 256, m: 16, dtype: F32, fields: [embedding],
  distance: Cosine, ef_construction: 64} }
{ ::hnsw create evidence:evidence_vec {dim: 256, m: 16, dtype: F32, fields: [embedding],
  distance: Cosine, ef_construction: 64} }
"#;

/// Recursion cap for [`CAUSAL_CHAINS`] traversal: a chain path may grow to
/// at most this many nodes before the `length(p) < 6` guard stops
/// expansion. The literal `6` in [`CAUSAL_CHAINS`] must stay in sync with
/// this constant if it is ever changed.
pub const MAX_CHAIN_NODES: usize = 6;

/// Ingests one dossier atomically.
///
/// Chained `{ ... }` blocks in a single script: a `dossier` row, a
/// `dossier_item` row for every entity/event/source/claim/evidence id in
/// the payload, and every `entity`, `event`, `event_actor`, `source`,
/// `claim`, `claim_subject`, `evidence`, `causal_link`, and
/// `temporal_relation` row. A failure in any block rolls back every block.
///
/// Parameters (each a `DataValue::List` of row lists): `$dossier`,
/// `$dossier_items`, `$entities`, `$events`, `$event_actors`, `$sources`,
/// `$claims`, `$claim_subjects`, `$evidence`, `$causal_links`,
/// `$temporal_relations`.
pub const INGEST_SCRIPT: &str = r#"
{ ?[id, question, created_at] <- $dossier
  :put dossier {id => question, created_at} }
{ ?[dossier_id, item_id] <- $dossier_items
  :put dossier_item {dossier_id, item_id} }
{ ?[id, name, kind, description] <- $entities
  :put entity {id => name, kind, description} }
{ ?[id, name, occurred_at, description] <- $events
  :put event {id => name, occurred_at, description} }
{ ?[event_id, entity_id] <- $event_actors
  :put event_actor {event_id, entity_id} }
{ ?[id, title, url, provider, published, retrieved_at] <- $sources
  :put source {id => title, url, provider, published, retrieved_at} }
{ ?[id, text, kind, embedding] <- $claims
  :put claim {id => text, kind, embedding} }
{ ?[claim_id, subject_id] <- $claim_subjects
  :put claim_subject {claim_id, subject_id} }
{ ?[id, claim_id, source_id, stance, excerpt, quality, embedding] <- $evidence
  :put evidence {id => claim_id, source_id, stance, excerpt, quality, embedding} }
{ ?[cause_id, effect_id, mechanism, confidence] <- $causal_links
  :put causal_link {cause_id, effect_id, mechanism, confidence} }
{ ?[before_id, after_id, relation] <- $temporal_relations
  :put temporal_relation {before_id, after_id, relation} }
"#;

/// Returns every entity in a dossier as `(id, name, kind, description)`,
/// ordered by `kind`, `name`. Parameter: `$dossier_id`.
pub const ENTITIES: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
?[id, name, kind, description] := member[id], *entity{id, name, kind, description}
:order kind, name
"#;

/// Returns every event in a dossier as `(id, name, occurred_at,
/// description)`, ordered by `occurred_at`, `name`. Parameter:
/// `$dossier_id`.
pub const EVENTS: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
?[id, name, occurred_at, description] := member[id], *event{id, name, occurred_at, description}
:order occurred_at, name
"#;

/// Returns every `(event_id, entity_id)` participation pair for events in
/// a dossier. Parameter: `$dossier_id`.
pub const EVENT_ACTORS: &str = r#"
member[event_id] := *dossier_item{dossier_id: $dossier_id, item_id: event_id}
?[event_id, entity_id] := member[event_id], *event_actor{event_id, entity_id}
"#;

/// Returns every source in a dossier as `(id, title, url, provider,
/// published, retrieved_at)`, ordered by `provider`, `title`. Parameter:
/// `$dossier_id`.
pub const SOURCES: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
?[id, title, url, provider, published, retrieved_at] :=
  member[id], *source{id, title, url, provider, published, retrieved_at}
:order provider, title
"#;

/// Returns every `(claim_id, subject_id)` pair for claims in a dossier.
/// Parameter: `$dossier_id`.
pub const CLAIM_SUBJECTS: &str = r#"
member[claim_id] := *dossier_item{dossier_id: $dossier_id, item_id: claim_id}
?[claim_id, subject_id] := member[claim_id], *claim_subject{claim_id, subject_id}
"#;

/// Returns every evidence row in a dossier as `(id, claim_id, source_id,
/// stance, excerpt, quality)`, ordered by `claim_id`, `id`. Parameter:
/// `$dossier_id`.
pub const EVIDENCE: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
?[id, claim_id, source_id, stance, excerpt, quality] :=
  member[id], *evidence{id, claim_id, source_id, stance, excerpt, quality}
:order claim_id, id
"#;

/// Returns every claim in a dossier with zero-filled support/contradict
/// evidence counts, as `(id, text, kind, supports, contradicts)`, ordered
/// by `id`. Parameter: `$dossier_id`.
pub const CLAIMS_WITH_STANCE: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
sup[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'supports'}
con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}
?[id, text, kind, ns, nc] := member[id], *claim{id, text, kind}, sup[id, ns], con[id, nc]
?[id, text, kind, ns, nc] :=
  member[id], *claim{id, text, kind}, sup[id, ns], not con[id, _], nc = 0
?[id, text, kind, ns, nc] :=
  member[id], *claim{id, text, kind}, not sup[id, _], con[id, nc], ns = 0
?[id, text, kind, ns, nc] :=
  member[id], *claim{id, text, kind}, not sup[id, _], not con[id, _], ns = 0, nc = 0
:order id
"#;

/// Returns every causal link in a dossier as `(cause_id, effect_id,
/// mechanism, confidence)`, ordered by `cause_id`, `effect_id`. Parameter:
/// `$dossier_id`.
pub const CAUSAL_LINKS: &str = r#"
member[cause_id] := *dossier_item{dossier_id: $dossier_id, item_id: cause_id}
?[cause_id, effect_id, mechanism, confidence] :=
  member[cause_id], *causal_link{cause_id, effect_id, mechanism, confidence}
:order cause_id, effect_id
"#;

/// Returns every causal chain in a dossier (recursive traversal capped at
/// [`MAX_CHAIN_NODES`] nodes) as `(start, end, path, length)`, longest
/// first, then ordered by `start`, `end`. Parameter: `$dossier_id`.
pub const CAUSAL_CHAINS: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
link[a, b] := member[a], *causal_link{cause_id: a, effect_id: b}
chain[a, b, path] := link[a, b], path = [a, b]
chain[a, c, path] := chain[a, b, p], length(p) < 6, link[b, c], path = append(p, c)
?[a, c, path, n] := chain[a, c, path], n = length(path)
:order -n, a, c
"#;

/// Returns up to 3 crux claims in a dossier: claims with both support and
/// contradiction, ranked by contestation plus downstream causal reach, as
/// `(id, text, supports, contradicts, downstream, score)`, highest score
/// first, then ordered by `id`. Parameter: `$dossier_id`.
pub const CRUXES: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
link[a, b] := member[a], *causal_link{cause_id: a, effect_id: b}
reach[a, b] := link[a, b]
reach[a, c] := reach[a, b], link[b, c]
down[a, count(b)] := reach[a, b]
sup[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'supports'}
con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}
scored[c, ns, nc, nd, s] := member[c], *claim{id: c}, sup[c, ns], con[c, nc], down[c, nd],
  s = ns + nc + 2 * nd
scored[c, ns, nc, nd, s] :=
  member[c], *claim{id: c}, sup[c, ns], con[c, nc], not down[c, _],
  nd = 0, s = ns + nc
?[id, text, ns, nc, nd, score] := scored[id, ns, nc, nd, score], *claim{id, text}
:order -score, id
:limit 3
"#;

/// Returns claims in a dossier with at least 2 distinct supporting sources
/// and zero contradictions, as `(id, text, sources)`, most-sourced first,
/// then ordered by `id`. Parameter: `$dossier_id`.
pub const CONSENSUS: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
sup_src[c, count_unique(s)] :=
  member[c], *evidence{claim_id: c, source_id: s, stance: 'supports'}
con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}
?[id, text, n] := sup_src[id, n], n >= 2, not con[id, _], *claim{id, text}
:order -n, id
"#;

/// Returns every temporal relation in a dossier as `(before_id, after_id,
/// relation)`, ordered by `before_id`, `after_id`. Parameter:
/// `$dossier_id`.
pub const TEMPORAL_RELATIONS: &str = r#"
member[before_id] := *dossier_item{dossier_id: $dossier_id, item_id: before_id}
?[before_id, after_id, relation] :=
  member[before_id], *temporal_relation{before_id, after_id, relation}
:order before_id, after_id
"#;

/// Returns the 8 nearest evidence rows to a query embedding, globally
/// across every dossier, as `(id, claim_id, source_id, stance, excerpt,
/// distance)`, nearest first, then ordered by `id`. Parameter: `$q` (a
/// 256-dimensional embedding, see [`crate::embed::embed`]).
pub const SIMILAR_EVIDENCE: &str = r#"
?[id, claim_id, source_id, stance, excerpt, dist] :=
  ~evidence:evidence_vec{id, claim_id, source_id, stance, excerpt | query: q, k: 8, ef: 64,
  bind_distance: dist}, q = vec($q)
:order dist, id
"#;

/// Returns the 5 nearest claims to a query embedding, globally across
/// every dossier, as `(id, text, distance)`, nearest first, then ordered
/// by `id`. Parameter: `$q` (a 256-dimensional embedding, see
/// [`crate::embed::embed`]).
pub const SIMILAR_CLAIMS: &str = r#"
?[id, text, dist] :=
  ~claim:claim_vec{id, text | query: q, k: 5, ef: 64, bind_distance: dist}, q = vec($q)
:order dist, id
"#;
