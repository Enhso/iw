//! mnestic Datalog schema v2 DDL, the atomic ingestion script, and every
//! named query used by [`crate::store::GraphStore`].
//!
//! Schema v2 (`docs/contracts.md` §D in the `betomcat` repository) gives
//! every relation a trailing `tt: TxTime` key column: the engine stamps
//! `tt` at commit and rejects a caller-supplied value, `:put` appends a new
//! version instead of overwriting, and a bare read returns current state
//! while `:as_of $as_of` (bound to an RFC 3339 string, or the literal
//! `"NOW"`) reproduces state as of a past instant. Every query below binds
//! `$as_of` as a block-level `:as_of` option; callers that want "now" pass
//! `"NOW"`.
//!
//! `::hnsw create` is rejected on a TxTime relation (verified against
//! mnestic 0.18.0, see the build log), so `claim` and `evidence` no longer
//! carry an `embedding` column: the vectors live in the plain, unversioned
//! side relations `claim_vec` and `evidence_vec` (a pure function of the
//! claim/evidence text, so they need no history), and the two HNSW indexes
//! are built on those instead. A vector-leg query still respects `:as_of`,
//! since it joins each hit back to the tt-stamped `claim`/`evidence`
//! relation inside the same `:as_of`-pinned query: a hit whose node did not
//! exist yet as of `$as_of` has no join partner and is silently dropped,
//! so a past-dated read can never surface future content (verified with a
//! throwaway probe against mnestic 0.18.0 before this module was written;
//! see the build log).
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
///
/// **Upgrading from schema v1**: this is a breaking schema change (v1's
/// `claim`/`evidence` carried `embedding` directly and no relation had
/// `tt`). A sqlite database created under v1 must be deleted; mnestic does
/// not migrate an existing schema in place.
pub const SCHEMA_DDL: &str = r#"
{ :create dossier {id: String, tt: TxTime => question: String, created_at: String} }
{ :create dossier_item {dossier_id: String, item_id: String, tt: TxTime} }
{ :create dossier_question {dossier_id: String, question_id: String, tt: TxTime} }
{ :create entity {id: String, tt: TxTime => name: String, kind: String, description: String} }
{ :create event {id: String, tt: TxTime =>
  name: String, occurred_at: String, description: String} }
{ :create event_actor {dossier_id: String, event_id: String, entity_id: String, tt: TxTime} }
{ :create source {id: String, tt: TxTime => title: String, url: String, provider: String,
  published: String, retrieved_at: String, content_hash: String} }
{ :create blob {content_hash: String => content: String} }
{ :create claim {id: String, tt: TxTime => text: String, kind: String} }
{ :create claim_vec {id: String => embedding: <F32; 256>} }
{ :create claim_subject {dossier_id: String, claim_id: String, subject_id: String, tt: TxTime} }
{ :create evidence {id: String, tt: TxTime => claim_id: String, source_id: String,
  stance: String, excerpt: String, quality: Float} }
{ :create evidence_vec {id: String => embedding: <F32; 256>} }
{ :create causal_link {dossier_id: String, cause_id: String, effect_id: String, tt: TxTime =>
  mechanism: String, confidence: String} }
{ :create temporal_relation {dossier_id: String, before_id: String, after_id: String,
  tt: TxTime => relation: String} }
{ :create claim_support {dossier_id: String, claim_id: String, tt: TxTime =>
  support: Float?, method: String} }
{ :create family {id: String, tt: TxTime =>
  label: String, description: String, created_at: String, merged_into: String?} }
{ :create family_seen {family_id: String, tt: TxTime => last_seen: String} }
{ :create question_family {question_id: String, tt: TxTime =>
  family_id: String, probability: Float?, method: String, question: String} }
{ :create history_item {id: String, tt: TxTime => kind: String, title: String, url: String,
  question_type: String, forecast: Json, resolution: String?, resolved_at: String?,
  family_id: String?, note: String} }
{ ::hnsw create claim_vec:idx {dim: 256, m: 16, dtype: F32, fields: [embedding],
  distance: Cosine, ef_construction: 64} }
{ ::hnsw create evidence_vec:idx {dim: 256, m: 16, dtype: F32, fields: [embedding],
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
/// `dossier_question` row (only when a question id was supplied), a
/// `dossier_item` row for every entity/event/source/claim/evidence id in
/// the payload, every `entity`/`event`/`source`/`claim`/`evidence` row
/// (global, shared nodes) plus each source's `blob` row and each
/// claim's/evidence's `claim_vec`/`evidence_vec` embedding row, every
/// `event_actor`/`claim_subject`/`causal_link`/`temporal_relation` row
/// (edges, each carrying the dossier id as its first cell), and one
/// `claim_support` row per claim. `:put` on every `tt`-stamped relation
/// appends a new version rather than overwriting, so re-ingesting the same
/// dossier (e.g. because a source's content changed) keeps every prior
/// version reachable via `:as_of`. A failure in any block rolls back every
/// block.
///
/// Parameters (each a `DataValue::List` of row lists): `$dossier`,
/// `$dossier_questions`, `$dossier_items`, `$entities`, `$events`,
/// `$event_actors`, `$sources`, `$blobs`, `$claims`, `$claim_vecs`,
/// `$claim_subjects`, `$evidence`, `$evidence_vecs`, `$causal_links`,
/// `$temporal_relations`, `$claim_supports`.
pub const INGEST_SCRIPT: &str = r#"
{ ?[id, question, created_at] <- $dossier
  :put dossier {id => question, created_at} }
{ ?[dossier_id, question_id] <- $dossier_questions
  :put dossier_question {dossier_id, question_id} }
{ ?[dossier_id, item_id] <- $dossier_items
  :put dossier_item {dossier_id, item_id} }
{ ?[id, name, kind, description] <- $entities
  :put entity {id => name, kind, description} }
{ ?[id, name, occurred_at, description] <- $events
  :put event {id => name, occurred_at, description} }
{ ?[dossier_id, event_id, entity_id] <- $event_actors
  :put event_actor {dossier_id, event_id, entity_id} }
{ ?[id, title, url, provider, published, retrieved_at, content_hash] <- $sources
  :put source {id => title, url, provider, published, retrieved_at, content_hash} }
{ ?[content_hash, content] <- $blobs
  :put blob {content_hash => content} }
{ ?[id, text, kind] <- $claims
  :put claim {id => text, kind} }
{ ?[id, embedding] <- $claim_vecs
  :put claim_vec {id => embedding} }
{ ?[dossier_id, claim_id, subject_id] <- $claim_subjects
  :put claim_subject {dossier_id, claim_id, subject_id} }
{ ?[id, claim_id, source_id, stance, excerpt, quality] <- $evidence
  :put evidence {id => claim_id, source_id, stance, excerpt, quality} }
{ ?[id, embedding] <- $evidence_vecs
  :put evidence_vec {id => embedding} }
{ ?[dossier_id, cause_id, effect_id, mechanism, confidence] <- $causal_links
  :put causal_link {dossier_id, cause_id, effect_id => mechanism, confidence} }
{ ?[dossier_id, before_id, after_id, relation] <- $temporal_relations
  :put temporal_relation {dossier_id, before_id, after_id => relation} }
{ ?[dossier_id, claim_id, support, method] <- $claim_supports
  :put claim_support {dossier_id, claim_id => support, method} }
"#;

/// Returns every entity in a dossier as of `$as_of`, as `(id, name, kind,
/// description)`, ordered by `kind`, `name`. Parameters: `$dossier_id`,
/// `$as_of`.
pub const ENTITIES: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
?[id, name, kind, description] := member[id], *entity{id, name, kind, description}
:as_of $as_of
:order kind, name
"#;

/// Returns every event in a dossier as of `$as_of`, as `(id, name,
/// occurred_at, description)`, ordered by `occurred_at`, `name`.
/// Parameters: `$dossier_id`, `$as_of`.
pub const EVENTS: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
?[id, name, occurred_at, description] := member[id], *event{id, name, occurred_at, description}
:as_of $as_of
:order occurred_at, name
"#;

/// Returns every `(event_id, entity_id)` participation pair recorded
/// directly under `dossier_id` as of `$as_of`, since `event_actor` is an
/// edge relation keyed by `dossier_id` rather than dossier membership.
/// Parameters: `$dossier_id`, `$as_of`.
pub const EVENT_ACTORS: &str = r#"
?[event_id, entity_id] := *event_actor{dossier_id: $dossier_id, event_id, entity_id}
:as_of $as_of
"#;

/// Returns every source in a dossier as of `$as_of`, as `(id, title, url,
/// provider, published, retrieved_at, content_hash)`, ordered by
/// `provider`, `title`. Parameters: `$dossier_id`, `$as_of`.
pub const SOURCES: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
?[id, title, url, provider, published, retrieved_at, content_hash] :=
  member[id], *source{id, title, url, provider, published, retrieved_at, content_hash}
:as_of $as_of
:order provider, title
"#;

/// Returns every source globally as of `$as_of` (no dossier scoping), as
/// `(id, title, url, provider, published, retrieved_at, content_hash)`,
/// ordered by `provider`, `title`, capped at `$limit`. Backs `GET
/// /api/documents` when no `family_id` filter is given. Parameters:
/// `$as_of`, `$limit`.
pub const ALL_SOURCES: &str = r#"
?[id, title, url, provider, published, retrieved_at, content_hash] :=
  *source{id, title, url, provider, published, retrieved_at, content_hash}
:as_of $as_of
:order provider, title
:limit $limit
"#;

/// Looks up a source's `content` by `content_hash`. `blob` is plain
/// (content-addressed, immutable), so this needs no `:as_of`. Parameter:
/// `$content_hash`.
pub const BLOB_CONTENT: &str = r#"
?[content] := *blob{content_hash: $content_hash, content}
"#;

/// Returns every `(claim_id, subject_id)` pair recorded directly under
/// `dossier_id` as of `$as_of`, since `claim_subject` is an edge relation
/// keyed by `dossier_id` rather than dossier membership. Parameters:
/// `$dossier_id`, `$as_of`.
pub const CLAIM_SUBJECTS: &str = r#"
?[claim_id, subject_id] := *claim_subject{dossier_id: $dossier_id, claim_id, subject_id}
:as_of $as_of
"#;

/// Returns every evidence row in a dossier as of `$as_of`, as `(id,
/// claim_id, source_id, stance, excerpt, quality)`, ordered by `claim_id`,
/// `id`. Parameters: `$dossier_id`, `$as_of`.
pub const EVIDENCE: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
?[id, claim_id, source_id, stance, excerpt, quality] :=
  member[id], *evidence{id, claim_id, source_id, stance, excerpt, quality}
:as_of $as_of
:order claim_id, id
"#;

/// Returns every claim in a dossier with zero-filled support/contradict
/// evidence counts, as of `$as_of`, as `(id, text, kind, supports,
/// contradicts)`, ordered by `id`. Both `sup` and `con` also require the
/// evidence item itself to be a dossier member, so evidence sharing a
/// claim id with another dossier never inflates these counts. Parameters:
/// `$dossier_id`, `$as_of`.
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
:as_of $as_of
:order id
"#;

/// Returns every claim's support score in a dossier as of `$as_of`, as
/// `(claim_id, support, method)`. `support` is `null` when the gate did
/// not run or failed. Parameters: `$dossier_id`, `$as_of`.
pub const CLAIM_SUPPORT: &str = r#"
?[claim_id, support, method] := *claim_support{dossier_id: $dossier_id, claim_id, support, method}
:as_of $as_of
"#;

/// Returns every causal link recorded directly under `dossier_id` as of
/// `$as_of`, as `(cause_id, effect_id, mechanism, confidence)`, ordered by
/// `cause_id`, `effect_id`. `causal_link` is an edge relation keyed by
/// `dossier_id` rather than dossier membership. Parameters: `$dossier_id`,
/// `$as_of`.
pub const CAUSAL_LINKS: &str = r#"
?[cause_id, effect_id, mechanism, confidence] :=
  *causal_link{dossier_id: $dossier_id, cause_id, effect_id, mechanism, confidence}
:as_of $as_of
:order cause_id, effect_id
"#;

/// Returns every causal chain in a dossier as of `$as_of` (recursive
/// traversal over `dossier_id`-scoped links, capped at [`MAX_CHAIN_NODES`]
/// nodes and guarded against revisiting a node already on the path) as
/// `(start, end, path, length)`, longest first, then ordered by `start`,
/// `end`. Parameters: `$dossier_id`, `$as_of`.
pub const CAUSAL_CHAINS: &str = r#"
link[a, b] := *causal_link{dossier_id: $dossier_id, cause_id: a, effect_id: b}
chain[a, b, path] := link[a, b], a != b, path = [a, b]
chain[a, c, path] := chain[a, b, p], length(p) < 6, link[b, c], !is_in(c, p), path = append(p, c)
?[a, c, path, n] := chain[a, c, path], n = length(path)
:as_of $as_of
:order -n, a, c
"#;

/// Returns up to 3 crux claims in a dossier as of `$as_of`: claims with
/// both support and contradiction, ranked by contestation plus downstream
/// causal reach, as `(id, text, supports, contradicts, downstream,
/// score)`, highest score first, then ordered by `id`. `sup` and `con`
/// also require the evidence item to be a dossier member; `reach` excludes
/// a node reaching itself, so a crux never reports itself among its own
/// downstream effects. Parameters: `$dossier_id`, `$as_of`.
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
:as_of $as_of
:order -score, id
:limit 3
"#;

/// Returns claims in a dossier as of `$as_of` with at least 2 distinct
/// supporting sources and zero contradictions, as `(id, text, sources)`,
/// most-sourced first, then ordered by `id`. `sup_src` and `con` also
/// require the evidence item to be a dossier member. Parameters:
/// `$dossier_id`, `$as_of`.
pub const CONSENSUS: &str = r#"
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
sup_src[c, count_unique(s)] :=
  member[c], *evidence{id: e, claim_id: c, source_id: s, stance: 'supports'}, member[e]
con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}, member[e]
?[id, text, n] := sup_src[id, n], n >= 2, not con[id, _], *claim{id, text}
:as_of $as_of
:order -n, id
"#;

/// Returns every temporal relation recorded directly under `dossier_id` as
/// of `$as_of`, as `(before_id, after_id, relation)`, ordered by
/// `before_id`, `after_id`. `temporal_relation` is an edge relation keyed
/// by `dossier_id` rather than dossier membership. Parameters:
/// `$dossier_id`, `$as_of`.
pub const TEMPORAL_RELATIONS: &str = r#"
?[before_id, after_id, relation] :=
  *temporal_relation{dossier_id: $dossier_id, before_id, after_id, relation}
:as_of $as_of
:order before_id, after_id
"#;

/// Returns the 8 nearest evidence rows to a query embedding as of
/// `$as_of`, globally across every dossier, as `(id, claim_id, source_id,
/// source_title, stance, excerpt, quality, distance)`, nearest first, then
/// ordered by `id`. The HNSW leg runs over the plain, unversioned
/// `evidence_vec` side relation (mnestic rejects an HNSW index on a
/// `tt`-stamped relation); joining each hit back to `*evidence{..}` and
/// `*source{..}` inside this same `:as_of`-pinned query drops any hit
/// whose evidence or source did not exist yet as of `$as_of`, so a
/// past-dated query cannot surface future content even though the vector
/// index itself carries no history. Parameters: `$q` (a 256-dimensional
/// embedding, see [`crate::embed::embed`]), `$as_of`.
pub const SIMILAR_EVIDENCE: &str = r#"
?[id, claim_id, source_id, source_title, stance, excerpt, quality, dist] :=
  ~evidence_vec:idx{id | query: q, k: 8, ef: 64, bind_distance: dist}, q = vec($q),
  *evidence{id, claim_id, source_id, stance, excerpt, quality},
  *source{id: source_id, title: source_title}
:as_of $as_of
:order dist, id
"#;

/// Returns the 5 nearest claims to a query embedding as of `$as_of`,
/// globally across every dossier, as `(id, text, distance)`, nearest
/// first, then ordered by `id`. Same future-content exclusion mechanism as
/// [`SIMILAR_EVIDENCE`]: the HNSW leg runs over the plain `claim_vec`
/// relation, and the join back to `*claim{..}` under `:as_of` drops
/// anything that did not exist yet. Parameters: `$q`, `$as_of`.
pub const SIMILAR_CLAIMS: &str = r#"
?[id, text, dist] :=
  ~claim_vec:idx{id | query: q, k: 5, ef: 64, bind_distance: dist}, q = vec($q), *claim{id, text}
:as_of $as_of
:order dist, id
"#;

/// Returns every `(dossier_id, created_at)` pair globally as of `$as_of`,
/// used to order `family_claims` newest-dossier-first (`docs/contracts.md`
/// §C2). Parameter: `$as_of`.
pub const ALL_DOSSIERS: &str = r#"
?[id, created_at] := *dossier{id, created_at}
:as_of $as_of
"#;

/// Returns every `(dossier_id, question_id)` tag globally as of `$as_of`.
/// Parameter: `$as_of`.
pub const ALL_DOSSIER_QUESTIONS: &str = r#"
?[dossier_id, question_id] := *dossier_question{dossier_id, question_id}
:as_of $as_of
"#;

/// Returns every `(question_id, family_id, probability, method)` tag
/// globally as of `$as_of`. Parameter: `$as_of`.
pub const ALL_QUESTION_FAMILIES: &str = r#"
?[question_id, family_id, probability, method] :=
  *question_family{question_id, family_id, probability, method}
:as_of $as_of
"#;

/// Returns every `(id, label, description, created_at, merged_into)`
/// family globally as of `$as_of`. Parameter: `$as_of`.
pub const ALL_FAMILIES: &str = r#"
?[id, label, description, created_at, merged_into] :=
  *family{id, label, description, created_at, merged_into}
:as_of $as_of
"#;

/// Returns every `(family_id, last_seen)` pair globally as of `$as_of`.
/// Parameter: `$as_of`.
pub const ALL_FAMILY_SEEN: &str = r#"
?[family_id, last_seen] := *family_seen{family_id, last_seen}
:as_of $as_of
"#;

/// Returns every history item globally as of `$as_of`, as `(id, kind,
/// title, url, question_type, forecast, resolution, resolved_at,
/// family_id, note)`. Parameter: `$as_of`.
pub const ALL_HISTORY: &str = r#"
?[id, kind, title, url, question_type, forecast, resolution, resolved_at, family_id, note] :=
  *history_item{id, kind, title, url, question_type, forecast, resolution, resolved_at,
  family_id, note}
:as_of $as_of
"#;
