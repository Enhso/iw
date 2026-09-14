//! mnestic Datalog schema DDL, the atomic ingestion script, and every named
//! query used by [`crate::store::GraphStore`].
//!
//! Every constant in this module is transcribed verbatim (modulo line
//! wrapping to respect the 100-column limit; mnestic's script parser
//! ignores insignificant whitespace) from the "mnestic schema", "Ingestion
//! pattern", and "Datalog queries" sections of `docs/plan-phase1.md`.
//!
//! Nodes (`entity`, `event`, `source`, `claim`, `evidence`) are global,
//! shared records keyed by id. Edges (`event_actor`, `claim_subject`,
//! `causal_link`, `temporal_relation`) carry a `dossier_id` column, because
//! an edge is an assertion made inside one dossier: the same event or claim
//! id can be shared across dossiers without their edges leaking into each
//! other. The two HNSW vector legs stay global on purpose.

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
{ :create event_actor {dossier_id: String, event_id: String, entity_id: String} }
{ :create source {id: String => title: String, url: String, provider: String,
  published: String, retrieved_at: String} }
{ :create claim {id: String => text: String, kind: String, embedding: <F32; 256>} }
{ :create claim_subject {dossier_id: String, claim_id: String, subject_id: String} }
{ :create evidence {id: String => claim_id: String, source_id: String, stance: String,
  excerpt: String, quality: Float, embedding: <F32; 256>} }
{ :create causal_link {dossier_id: String, cause_id: String, effect_id: String =>
  mechanism: String, confidence: String} }
{ :create temporal_relation {dossier_id: String, before_id: String, after_id: String =>
  relation: String} }
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
/// the payload, every `entity`, `event`, `source`, `claim`, and `evidence`
/// row (global, shared nodes), and every `event_actor`, `claim_subject`,
/// `causal_link`, and `temporal_relation` row (edges, each carrying the
/// dossier id as its first cell since an edge is an assertion made inside
/// one dossier). A failure in any block rolls back every block.
///
/// Parameters (each a `DataValue::List` of row lists): `$dossier`,
/// `$dossier_items`, `$entities`, `$events`, `$event_actors`, `$sources`,
/// `$claims`, `$claim_subjects`, `$evidence`, `$causal_links`,
/// `$temporal_relations`. Every row in `$event_actors`, `$claim_subjects`,
/// `$causal_links`, and `$temporal_relations` is prefixed with the dossier
/// id as its first cell.
pub const INGEST_SCRIPT: &str = r#"
{ ?[id, question, created_at] <- $dossier
  :put dossier {id => question, created_at} }
{ ?[dossier_id, item_id] <- $dossier_items
  :put dossier_item {dossier_id, item_id} }
{ ?[id, name, kind, description] <- $entities
  :put entity {id => name, kind, description} }
{ ?[id, name, occurred_at, description] <- $events
  :put event {id => name, occurred_at, description} }
{ ?[dossier_id, event_id, entity_id] <- $event_actors
  :put event_actor {dossier_id, event_id, entity_id} }
{ ?[id, title, url, provider, published, retrieved_at] <- $sources
  :put source {id => title, url, provider, published, retrieved_at} }
{ ?[id, text, kind, embedding] <- $claims
  :put claim {id => text, kind, embedding} }
{ ?[dossier_id, claim_id, subject_id] <- $claim_subjects
  :put claim_subject {dossier_id, claim_id, subject_id} }
{ ?[id, claim_id, source_id, stance, excerpt, quality, embedding] <- $evidence
  :put evidence {id => claim_id, source_id, stance, excerpt, quality, embedding} }
{ ?[dossier_id, cause_id, effect_id, mechanism, confidence] <- $causal_links
  :put causal_link {dossier_id, cause_id, effect_id => mechanism, confidence} }
{ ?[dossier_id, before_id, after_id, relation] <- $temporal_relations
  :put temporal_relation {dossier_id, before_id, after_id => relation} }
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

/// Returns every `(event_id, entity_id)` participation pair recorded
/// directly under `dossier_id`, since `event_actor` is an edge relation
/// keyed by `dossier_id` rather than dossier membership. Parameter:
/// `$dossier_id`.
pub const EVENT_ACTORS: &str = r#"
?[event_id, entity_id] := *event_actor{dossier_id: $dossier_id, event_id, entity_id}
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

/// Returns every `(claim_id, subject_id)` pair recorded directly under
/// `dossier_id`, since `claim_subject` is an edge relation keyed by
/// `dossier_id` rather than dossier membership. Parameter: `$dossier_id`.
pub const CLAIM_SUBJECTS: &str = r#"
?[claim_id, subject_id] := *claim_subject{dossier_id: $dossier_id, claim_id, subject_id}
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
/// by `id`. Both `sup` and `con` also require the evidence item itself to
/// be a dossier member, so evidence sharing a claim id with another
/// dossier never inflates these counts. Parameter: `$dossier_id`.
pub const CLAIMS_WITH_STANCE: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
sup[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'supports'}, member[e]
con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}, member[e]
?[id, text, kind, ns, nc] := member[id], *claim{id, text, kind}, sup[id, ns], con[id, nc]
?[id, text, kind, ns, nc] :=
  member[id], *claim{id, text, kind}, sup[id, ns], not con[id, _], nc = 0
?[id, text, kind, ns, nc] :=
  member[id], *claim{id, text, kind}, not sup[id, _], con[id, nc], ns = 0
?[id, text, kind, ns, nc] :=
  member[id], *claim{id, text, kind}, not sup[id, _], not con[id, _], ns = 0, nc = 0
:order id
"#;

/// Returns every causal link recorded directly under `dossier_id`, as
/// `(cause_id, effect_id, mechanism, confidence)`, ordered by `cause_id`,
/// `effect_id`. `causal_link` is an edge relation keyed by `dossier_id`
/// rather than dossier membership. Parameter: `$dossier_id`.
pub const CAUSAL_LINKS: &str = r#"
?[cause_id, effect_id, mechanism, confidence] :=
  *causal_link{dossier_id: $dossier_id, cause_id, effect_id, mechanism, confidence}
:order cause_id, effect_id
"#;

/// Returns every causal chain in a dossier (recursive traversal over
/// `dossier_id`-scoped links, capped at [`MAX_CHAIN_NODES`] nodes and
/// guarded against revisiting a node already on the path) as `(start, end,
/// path, length)`, longest first, then ordered by `start`, `end`.
/// Parameter: `$dossier_id`.
pub const CAUSAL_CHAINS: &str = r#"
link[a, b] := *causal_link{dossier_id: $dossier_id, cause_id: a, effect_id: b}
chain[a, b, path] := link[a, b], a != b, path = [a, b]
chain[a, c, path] := chain[a, b, p], length(p) < 6, link[b, c], !is_in(c, p), path = append(p, c)
?[a, c, path, n] := chain[a, c, path], n = length(path)
:order -n, a, c
"#;

/// Returns up to 3 crux claims in a dossier: claims with both support and
/// contradiction, ranked by contestation plus downstream causal reach, as
/// `(id, text, supports, contradicts, downstream, score)`, highest score
/// first, then ordered by `id`. `sup` and `con` also require the evidence
/// item to be a dossier member; `reach` excludes a node reaching itself, so
/// a crux never reports itself among its own downstream effects.
/// Parameter: `$dossier_id`.
pub const CRUXES: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
link[a, b] := *causal_link{dossier_id: $dossier_id, cause_id: a, effect_id: b}
reach[a, b] := link[a, b], a != b
reach[a, c] := reach[a, b], link[b, c], a != c
down[a, count(b)] := reach[a, b]
sup[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'supports'}, member[e]
con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}, member[e]
scored[c, ns, nc, nd, s] := member[c], *claim{id: c}, sup[c, ns], con[c, nc], down[c, nd],
  s = ns + nc + 2 * nd
scored[c, ns, nc, nd, s] :=
  member[c], *claim{id: c}, sup[c, ns], con[c, nc], not down[c, _], nd = 0, s = ns + nc
?[id, text, ns, nc, nd, score] := scored[id, ns, nc, nd, score], *claim{id, text}
:order -score, id
:limit 3
"#;

/// Returns claims in a dossier with at least 2 distinct supporting sources
/// and zero contradictions, as `(id, text, sources)`, most-sourced first,
/// then ordered by `id`. `sup_src` and `con` also require the evidence item
/// to be a dossier member. Parameter: `$dossier_id`.
pub const CONSENSUS: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
sup_src[c, count_unique(s)] :=
  member[c], *evidence{id: e, claim_id: c, source_id: s, stance: 'supports'}, member[e]
con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}, member[e]
?[id, text, n] := sup_src[id, n], n >= 2, not con[id, _], *claim{id, text}
:order -n, id
"#;

/// Returns every temporal relation recorded directly under `dossier_id`, as
/// `(before_id, after_id, relation)`, ordered by `before_id`, `after_id`.
/// `temporal_relation` is an edge relation keyed by `dossier_id` rather
/// than dossier membership. Parameter: `$dossier_id`.
pub const TEMPORAL_RELATIONS: &str = r#"
?[before_id, after_id, relation] :=
  *temporal_relation{dossier_id: $dossier_id, before_id, after_id, relation}
:order before_id, after_id
"#;

/// Returns the 8 nearest evidence rows to a query embedding, globally
/// across every dossier, as `(id, claim_id, source_id, source_title,
/// stance, excerpt, quality, distance)`, nearest first, then ordered by
/// `id`. `quality` is bound from the vector index itself and `source_title`
/// is resolved with a global join on `source`, so the renderer never needs
/// to look these rows up in a dossier-scoped context. Parameter: `$q` (a
/// 256-dimensional embedding, see [`crate::embed::embed`]).
pub const SIMILAR_EVIDENCE: &str = r#"
?[id, claim_id, source_id, source_title, stance, excerpt, quality, dist] :=
  ~evidence:evidence_vec{id, claim_id, source_id, stance, excerpt, quality | query: q, k: 8,
  ef: 64, bind_distance: dist}, q = vec($q), *source{id: source_id, title: source_title}
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
