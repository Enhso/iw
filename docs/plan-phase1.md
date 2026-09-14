# Intelligence Workbench — Phase 1 Vertical Slice Plan

Spec (binding authority): `prompt.txt`. Product requirements: `docs/prd.md` (§9 knowledge graph, §10 report sections). `docs/spec.md`, `docs/plan.md`, `docs/todo.md`, `docs/study.md` are superseded background.

Repository root: `/home/hatim/projects/iw`. Rust crate at the root (`iw-server`), Python worker under `python/`, shared fixtures under `fixtures/`.

## Global Constraints

These bind every task. Reviewers receive this section verbatim.

1. **No forecasting.** The system never emits a probability, percentage, likelihood, or prediction. This applies to renderer-authored text: the rendered briefing's renderer-authored text must not match the regex `\d+\s*%` and must not contain the words `probability` or `likelihood` (case-insensitive). Evidence quality is a `0.0..=1.0` float in storage but is rendered only as the words `high` (>= 0.7), `medium` (>= 0.4), or `low`. Verbatim source excerpts are exempt, since a source may legitimately use a percentage; instead, the Python worker drops any LLM-authored claim or causal link whose `text`/`mechanism` uses probability phrasing (case-insensitive match on `\b(probability|probabilities|likelihood|odds)\b|\d+(\.\d+)?\s*%\s*(chance|probability|likelihood|likely)`).
2. **Database is mnestic, owned by Rust.** Cargo dependency is `mnestic = "0.18"`; its library crate is named `cozo`, so Rust code imports `use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability};`. Engines: `mem` (tests, default) and `sqlite` (persistence). No Docker, no external DB. Python never opens the database.
3. **Python is a stateless worker.** `uv`-managed project in `python/`, invoked as `uv run --directory python iw-research --question "<q>" [--fixture-dir <dir>]`. It prints exactly one JSON document (the ExtractionPayload) to stdout and logs only to stderr.
4. **Shared contract = ExtractionPayload** (below). Rust `src/model.rs` and Python `iw_research/schema.py` both implement it; `fixtures/payload/semiconductor.json` is the canonical instance both test suites consume.
5. **Embeddings are deterministic and keyless**: 256-dimensional feature-hashed vectors computed in Rust (`src/embed.rs`), cosine distance, HNSW indexes on `claim.embedding` and `evidence.embedding`.
6. **Atomic ingestion**: one payload = one `run_script` call containing all `:put` blocks in `{ ... }` chains, so a failure rolls back everything (verified: mnestic rolls back all blocks when a later block errors).
7. **No network in tests.** Rust: no HTTP calls at all (the Python worker is spawned only in `--fixture-dir` mode). Python: `httpx` mocked with `pytest-httpx`; the LLM is mocked or fixture-backed.
8. **Rust rules** (from `CLAUDE.md`): edition 2021; `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must all pass with zero warnings; no `.unwrap()` outside `#[cfg(test)]`/`tests/` (use `?`, `expect("invariant …")`, or explicit errors); `thiserror` for error enums; doc comments (`///`) on every public item documenting params/returns/errors; errors reported with `tracing::error!`/`warn!`, never `println!`; 4-space indent; 100-column lines; no emoji or emoji-like unicode; no commented-out code; no `dbg!`.
9. **Python rules**: Python `>= 3.12`, `uv` only; `polars` for all tabular work (never pandas); `orjson` for JSON; `httpx` for HTTP; pydantic v2 models; type hints on every function; `uv run ruff check .`, `uv run ruff format --check .`, `uv run mypy src` (strict), and `uv run pytest -q` must pass; `logger.error`/`logger.warning` for errors, never `print`; no bare `except:`; no mutable default arguments; 88-column lines; tests are their own files under `python/tests/` and are never deleted.
10. **Identifiers** are prefixed lowercase slugs matching `^(ent|evt|clm|evd|src|dos):[a-z0-9]+(-[a-z0-9]+)*$`, at most 80 characters total. Slug rule: lowercase, non-alphanumerics collapse to a single `-`, leading/trailing `-` stripped. Dossier id = `dos:<slug>-<hash>`: `slug` is `slugify(question.trim())` truncated to 60 characters (no trailing `-`; the literal `q` if that slug is empty), and `hash` is the low 32 bits of the 64-bit FNV-1a hash of the trimmed question, formatted as 8 lowercase hex digits. At most 73 characters total. The hash disambiguates questions that would otherwise collide, either by agreeing on their first 60 slugified characters or by slugifying to `""` (e.g. non-Latin scripts).
11. **Referential integrity**: every `subject_ids`, `actor_ids`, `claim_id`, `source_id`, `cause_id`, `effect_id`, `before_id`, `after_id` value must reference an id declared in the same payload; ids must be unique across all lists. Rust rejects violations with HTTP 422; Python enforces the full Rust contract item by item rather than only dropping dangling references — repairing or dropping malformed ids, dates, quality, and enums, and dropping forecasting language, each logged via `logger.warning` — then drops dangling references and de-duplicates by id (keep first, logged). A shared vector file (`fixtures/contract/vectors.json`) of valid/invalid ids, dates, and quality scores is consumed by both the Rust and Python test suites.
12. **Commits**: one or more commits per task, conventional subject line, body ending with the two trailer lines:
    ```
    Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
    Claude-Session: https://claude.ai/code/session_018cDmnZnqLw6vSfbJ6Kg7FM
    ```
13. **Scope discipline**: one end-to-end slice. No frontend, no chat, no GitHub integration, no PR flow, no personalization, no extra providers, no abstractions for single-use code.

## ExtractionPayload contract (JSON)

```json
{
  "schema_version": 1,
  "question": "Can export controls durably slow China's access to advanced semiconductor manufacturing capability?",
  "entities": [
    {"id": "ent:tsmc", "name": "TSMC", "kind": "organization", "description": "..."}
  ],
  "events": [
    {"id": "evt:bis-export-controls-2022", "name": "...", "occurred_at": "2022-10-07", "description": "...", "actor_ids": ["ent:united-states"]}
  ],
  "sources": [
    {"id": "src:wikipedia-semiconductor-industry-in-china", "title": "...", "url": "https://...", "provider": "wikipedia", "published": "", "retrieved_at": "2026-09-14T00:00:00Z"}
  ],
  "claims": [
    {"id": "clm:controls-durably-slow-china", "text": "...", "kind": "hypothesis", "subject_ids": ["ent:china"]}
  ],
  "evidence": [
    {"id": "evd:smic-7nm-mate-60", "claim_id": "clm:controls-durably-slow-china", "source_id": "src:...", "stance": "contradicts", "excerpt": "...", "quality": 0.6}
  ],
  "causal_links": [
    {"cause_id": "evt:bis-export-controls-2022", "effect_id": "clm:...", "mechanism": "...", "confidence": "medium"}
  ],
  "temporal_relations": [
    {"before_id": "evt:...", "after_id": "evt:...", "relation": "before"}
  ]
}
```

Enumerations (exact strings): `entities[].kind` in `person | organization | country | technology | policy`; `sources[].provider` in `wikipedia | arxiv`; `claims[].kind` in `hypothesis | fact | assumption`; `evidence[].stance` in `supports | contradicts`; `causal_links[].confidence` in `low | medium | high`; `temporal_relations[].relation` in `before | during | after`. `occurred_at` and `published` are `YYYY-MM-DD` or `""` (unknown). `retrieved_at` is an RFC 3339 UTC timestamp. `causal_links` cause/effect ids reference claims (`clm:`) or events (`evt:`). `temporal_relations` ids reference events only. `subject_ids` reference entities or events. `actor_ids` reference entities. Empty lists are valid.

## mnestic schema (verbatim; validated against mnestic 0.18.0)

Run as ONE script (all blocks chained) only when `::relations` does not already list a relation named `claim`:

```
{ :create dossier {id: String => question: String, created_at: String} }
{ :create dossier_item {dossier_id: String, item_id: String} }
{ :create entity {id: String => name: String, kind: String, description: String} }
{ :create event {id: String => name: String, occurred_at: String, description: String} }
{ :create event_actor {dossier_id: String, event_id: String, entity_id: String} }
{ :create source {id: String => title: String, url: String, provider: String, published: String, retrieved_at: String} }
{ :create claim {id: String => text: String, kind: String, embedding: <F32; 256>} }
{ :create claim_subject {dossier_id: String, claim_id: String, subject_id: String} }
{ :create evidence {id: String => claim_id: String, source_id: String, stance: String, excerpt: String, quality: Float, embedding: <F32; 256>} }
{ :create causal_link {dossier_id: String, cause_id: String, effect_id: String => mechanism: String, confidence: String} }
{ :create temporal_relation {dossier_id: String, before_id: String, after_id: String => relation: String} }
{ ::hnsw create claim:claim_vec {dim: 256, m: 16, dtype: F32, fields: [embedding], distance: Cosine, ef_construction: 64} }
{ ::hnsw create evidence:evidence_vec {dim: 256, m: 16, dtype: F32, fields: [embedding], distance: Cosine, ef_construction: 64} }
```

Ingestion pattern (one script, chained blocks; each `$param` is a `DataValue::List` of row lists; vectors are passed as plain `DataValue::List` of `DataValue::from(f64)` and mnestic converts them):

```
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
```

`dossier_item` rows link the dossier id to EVERY id in the payload (entities, events, sources, claims, evidence). Membership is how graph queries are scoped; vector queries are global on purpose (knowledge compounds across dossiers). Nodes (`entity`, `event`, `source`, `claim`, `evidence`) are global, shared records keyed by id; edges (`event_actor`, `claim_subject`, `causal_link`, `temporal_relation`) carry a `dossier_id` column instead, because an edge is an assertion made inside one dossier and ids are routinely shared across related dossiers.

Verified syntax facts: a second `::hnsw create` on the same index fails with `index_already_exists` (hence the `::relations` guard); vector search is `~claim:claim_vec{id, text | query: q, k: 5, ef: 64, bind_distance: dist}, q = vec($q)`; recursion with `length(p) < 6` and `append(p, c)` works; `not rule[x, _]` negation works in rule bodies; `count(e)` / `count_unique(s)` aggregates work in rule heads; `:order`, `:limit` are query options.

## Datalog queries (verbatim; `$dossier_id` and `$q` are parameters)

Every scoped query over a shared node relation (`entity`, `event`, `source`, `claim`, `evidence`) starts with:
```
member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
```
Edges (`event_actor`, `claim_subject`, `causal_link`, `temporal_relation`) carry their own `dossier_id` column instead and are filtered directly on it, since nodes are shared across dossiers but an edge is an assertion made inside one.

- `ENTITIES`: `?[id, name, kind, description] := member[id], *entity{id, name, kind, description}` + `:order kind, name`
- `EVENTS`: `?[id, name, occurred_at, description] := member[id], *event{id, name, occurred_at, description}` + `:order occurred_at, name`
- `EVENT_ACTORS`: `?[event_id, entity_id] := *event_actor{dossier_id: $dossier_id, event_id, entity_id}`
- `SOURCES`: `?[id, title, url, provider, published, retrieved_at] := member[id], *source{id, title, url, provider, published, retrieved_at}` + `:order provider, title`
- `CLAIM_SUBJECTS`: `?[claim_id, subject_id] := *claim_subject{dossier_id: $dossier_id, claim_id, subject_id}`
- `EVIDENCE`: `?[id, claim_id, source_id, stance, excerpt, quality] := member[id], *evidence{id, claim_id, source_id, stance, excerpt, quality}` + `:order claim_id, id`
- `CLAIMS_WITH_STANCE` (support/contradict counts, zero-filled; `sup`/`con` also require the evidence item `e` to be a dossier member):
  ```
  member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
  sup[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'supports'}, member[e]
  con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}, member[e]
  ?[id, text, kind, ns, nc] := member[id], *claim{id, text, kind}, sup[id, ns], con[id, nc]
  ?[id, text, kind, ns, nc] := member[id], *claim{id, text, kind}, sup[id, ns], not con[id, _], nc = 0
  ?[id, text, kind, ns, nc] := member[id], *claim{id, text, kind}, not sup[id, _], con[id, nc], ns = 0
  ?[id, text, kind, ns, nc] := member[id], *claim{id, text, kind}, not sup[id, _], not con[id, _], ns = 0, nc = 0
  :order id
  ```
- `CAUSAL_LINKS`: `?[cause_id, effect_id, mechanism, confidence] := *causal_link{dossier_id: $dossier_id, cause_id, effect_id, mechanism, confidence}` + `:order cause_id, effect_id`
- `CAUSAL_CHAINS` (traversal rule, capped at 6 nodes, guarded against revisiting a node already on the path):
  ```
  link[a, b] := *causal_link{dossier_id: $dossier_id, cause_id: a, effect_id: b}
  chain[a, b, path] := link[a, b], a != b, path = [a, b]
  chain[a, c, path] := chain[a, b, p], length(p) < 6, link[b, c], !is_in(c, p), path = append(p, c)
  ?[a, c, path, n] := chain[a, c, path], n = length(path)
  :order -n, a, c
  ```
- `CRUXES` (crux discovery rule: contested claims ranked by contestation plus downstream causal reach; `a != c` in `reach` means a claim is never its own downstream effect):
  ```
  member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
  link[a, b] := *causal_link{dossier_id: $dossier_id, cause_id: a, effect_id: b}
  reach[a, b] := link[a, b], a != b
  reach[a, c] := reach[a, b], link[b, c], a != c
  down[a, count(b)] := reach[a, b]
  sup[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'supports'}, member[e]
  con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}, member[e]
  scored[c, ns, nc, nd, s] := member[c], *claim{id: c}, sup[c, ns], con[c, nc], down[c, nd], s = ns + nc + 2 * nd
  scored[c, ns, nc, nd, s] := member[c], *claim{id: c}, sup[c, ns], con[c, nc], not down[c, _], nd = 0, s = ns + nc
  ?[id, text, ns, nc, nd, score] := scored[id, ns, nc, nd, score], *claim{id, text}
  :order -score, id
  :limit 3
  ```
- `CONSENSUS` (>= 2 distinct supporting sources, zero contradictions; `sup_src`/`con` also require the evidence item `e` to be a dossier member):
  ```
  member[id] := *dossier_item{dossier_id: $dossier_id, item_id: id}
  sup_src[c, count_unique(s)] := member[c], *evidence{id: e, claim_id: c, source_id: s, stance: 'supports'}, member[e]
  con[c, count(e)] := member[c], *evidence{id: e, claim_id: c, stance: 'contradicts'}, member[e]
  ?[id, text, n] := sup_src[id, n], n >= 2, not con[id, _], *claim{id, text}
  :order -n, id
  ```
- `TEMPORAL_RELATIONS`: `?[before_id, after_id, relation] := *temporal_relation{dossier_id: $dossier_id, before_id, after_id, relation}` + `:order before_id, after_id`
- `SIMILAR_EVIDENCE` (vector leg, global; binds `quality` from the index and joins `source_title` globally, so the renderer never needs a dossier-scoped lookup for a globally-nearest row): `?[id, claim_id, source_id, source_title, stance, excerpt, quality, dist] := ~evidence:evidence_vec{id, claim_id, source_id, stance, excerpt, quality | query: q, k: 8, ef: 64, bind_distance: dist}, q = vec($q), *source{id: source_id, title: source_title}` + `:order dist, id`
- `SIMILAR_CLAIMS` (vector leg, global): `?[id, text, dist] := ~claim:claim_vec{id, text | query: q, k: 5, ef: 64, bind_distance: dist}, q = vec($q)` + `:order dist, id`

## File structure

```
Cargo.toml                      iw-server crate (lib + bin)
src/lib.rs                      pub mod app, briefing, config, embed, error, model, research, schema, store
src/error.rs                    AppError (thiserror) + axum IntoResponse           [Task 1]
src/model.rs                    ExtractionPayload + validate() + slug helpers      [Task 1]
src/embed.rs                    EMBED_DIM=256, embed(text) -> Vec<f32>             [Task 1]
src/schema.rs                   SCHEMA_DDL, INGEST_SCRIPT, query consts            [Task 3]
src/store.rs                    GraphStore: open, init_schema, ingest, query fns   [Task 3]
src/briefing.rs                 BriefingContext, build_context, render, 11 sections[Task 4]
src/research.rs                 ResearchWorker: spawn uv, parse payload            [Task 5]
src/config.rs                   Config from env                                    [Task 5]
src/app.rs                      AppState, router(), handlers                       [Task 5]
src/main.rs                     tracing init, config, serve                        [Task 5]
tests/roundtrip.rs              integration tests                                  [Task 5]
python/pyproject.toml, python/.python-version, python/src/iw_research/{__init__,schema,normalize,llm,extract,cli}.py, python/src/iw_research/sources/{__init__,wikipedia,arxiv}.py, python/tests/test_*.py   [Task 2]
fixtures/offline/{wikipedia_search.json, wikipedia_extracts/<slug>.json, arxiv.xml, llm_extraction.json}   [Task 2]
fixtures/payload/semiconductor.json                                                [Task 2]
README.md                                                                          [Task 5]
```

---

## Task 1: Rust crate foundation — contract types, errors, embeddings

**Goal:** a lib-only crate that compiles clean and owns the ExtractionPayload contract, the application error type, and the deterministic embedder.

**Files:** `Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/model.rs`, `src/embed.rs`.

### Cargo.toml

```toml
[package]
name = "iw-server"
version = "0.1.0"
edition = "2021"
description = "Intelligence Workbench Phase 1: mnestic-backed research ingestion and briefing synthesis"
license = "MIT"

[dependencies]
anyhow = "1"
axum = "0.8"
mnestic = "0.18"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
chrono = "0.4"

[dev-dependencies]
http-body-util = "0.1"
tempfile = "3"
tower = { version = "0.5", features = ["util"] }
```

`src/lib.rs` for this task declares only `pub mod embed; pub mod error; pub mod model;` with a crate-level `//!` doc comment; later tasks append modules.

### src/model.rs

Serde structs mirroring the contract exactly (field names as in the JSON): `ExtractionPayload { schema_version: u32, question: String, entities: Vec<Entity>, events: Vec<Event>, sources: Vec<Source>, claims: Vec<Claim>, evidence: Vec<Evidence>, causal_links: Vec<CausalLink>, temporal_relations: Vec<TemporalRelation> }` and the item structs `Entity { id, name, kind, description }`, `Event { id, name, occurred_at, description, actor_ids: Vec<String> }`, `Source { id, title, url, provider, published, retrieved_at }`, `Claim { id, text, kind, subject_ids: Vec<String> }`, `Evidence { id, claim_id, source_id, stance, excerpt, quality: f64 }`, `CausalLink { cause_id, effect_id, mechanism, confidence }`, `TemporalRelation { before_id, after_id, relation }`. All `String` unless noted. Derive `Debug, Clone, PartialEq, Serialize, Deserialize`. Enumerated fields stay `String` and are validated (no Rust enums; keeps the contract flat and the Datalog literal).

Public functions:
- `pub fn slugify(input: &str) -> String` — Global Constraint 10 rule.
- `pub fn dossier_id_for(question: &str) -> String` — `"dos:"` + first 60 chars of `slugify(question)` with any trailing `-` removed.
- `impl ExtractionPayload { pub fn validate(&self) -> Result<(), PayloadError> }` — checks: `schema_version == 1`; `question` non-empty after trim; every id matches Global Constraint 10 regex (implement by hand, no regex crate: prefix in the allowed set, then `[a-z0-9-]` with no leading/trailing/double `-`, length <= 80) and uses the prefix matching its list (`ent:` entities, `evt:` events, `src:` sources, `clm:` claims, `evd:` evidence); ids unique across all lists; enumerations valid; `quality` in `0.0..=1.0`; referential integrity per Global Constraint 11 (`subject_ids` -> entity or event, `actor_ids` -> entity, `claim_id` -> claim, `source_id` -> source, `cause_id`/`effect_id` -> claim or event, `before_id`/`after_id` -> event); dates are `""` or `YYYY-MM-DD` (10 chars, digits with `-` at positions 4 and 7).
- `pub fn all_item_ids(&self) -> Vec<String>` — ids of entities, events, sources, claims, evidence in that order (used by ingestion for `dossier_item`).

`PayloadError` is a `thiserror` enum with variants carrying the offending id/field, e.g. `UnknownReference { field: &'static str, id: String }`, `DuplicateId(String)`, `InvalidId(String)`, `InvalidEnum { field: &'static str, value: String }`, `InvalidDate { field: &'static str, value: String }`, `QualityOutOfRange { id: String, quality: f64 }`, `UnsupportedSchemaVersion(u32)`, `EmptyQuestion`.

### src/error.rs

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid payload: {0}")]
    InvalidPayload(#[from] crate::model::PayloadError),
    #[error("graph store error: {0}")]
    Store(String),
    #[error("research worker error: {0}")]
    Research(String),
    #[error("not found: {0}")]
    NotFound(String),
}
```
`impl IntoResponse for AppError` maps `InvalidPayload` -> 422, `Store` -> 500, `Research` -> 502, `NotFound` -> 404, body `Json(json!({"error": self.to_string()}))`, and logs 5xx with `tracing::error!` before responding. (Task 3 and Task 5 add `From` impls for their own error types; do not pre-add them.)

### src/embed.rs

- `pub const EMBED_DIM: usize = 256;`
- `pub fn embed(text: &str) -> Vec<f32>`: lowercase the text; tokens = maximal runs of `char::is_alphanumeric`; drop tokens shorter than 2 chars; features = every token plus every bigram `"{t_i} {t_i+1}"`; for each feature `h = fnv1a64(feature)`, `idx = (h % 256) as usize`, `sign = if (h >> 63) == 0 { 1.0 } else { -1.0 }`, `v[idx] += sign`; L2-normalize; if the norm is 0 (no features) return a vector with `v[0] = 1.0` and the rest 0.
- `fn fnv1a64(s: &str) -> u64` (offset `0xcbf29ce484222325`, prime `0x100000001b3`, over UTF-8 bytes).
- `pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32` = `1 - dot(a, b)` for unit vectors (document the precondition).

### Tests (unit, in each module)

- `model`: valid fixture-like payload passes; each violation class fails with the matching variant (dangling `claim_id`, duplicate id, bad prefix for list, bad enum, quality 1.5, bad date, wrong schema version, empty question); `slugify("  TSMC & ASML: EUV!! ") == "tsmc-asml-euv"`; `dossier_id_for` truncation and no trailing `-`; JSON round-trip of a payload preserves field names (`serde_json::to_value` then back).
- `embed`: same input twice is identical; length 256; norm within `1e-4` of 1; `cosine_distance` between "export controls on lithography tools" and "lithography tool export restrictions" is smaller than between the first and "rice harvest in the Mekong delta"; empty string yields the fallback unit vector.
- `error`: each variant maps to the expected status code (build the response and check `.status()`).

**Verification:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` all clean. Commit.

---

## Task 2: Python research and extraction worker

**Goal:** `uv run --directory python iw-research --question "<q>" --fixture-dir fixtures/offline` prints a valid ExtractionPayload; live mode queries Wikipedia and arXiv over HTTP and an OpenAI-compatible chat-completions LLM; all tests mocked.

**Files:** everything under `python/` and `fixtures/` per the file structure.

### pyproject.toml

```toml
[project]
name = "iw-research"
version = "0.1.0"
description = "Intelligence Workbench research and extraction worker"
requires-python = ">=3.12"
dependencies = ["httpx>=0.28", "orjson>=3.10", "polars>=1.30", "pydantic>=2.10"]

[project.scripts]
iw-research = "iw_research.cli:main"

[dependency-groups]
dev = ["mypy>=1.15", "pytest>=8.3", "pytest-httpx>=0.35", "ruff>=0.11"]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
packages = ["src/iw_research"]

[tool.ruff]
line-length = 88
target-version = "py312"

[tool.ruff.lint]
select = ["E", "F", "I", "UP", "B"]

[tool.mypy]
strict = true
files = ["src"]

[tool.pytest.ini_options]
testpaths = ["tests"]
```
`python/.python-version` contains `3.12`. Create the environment with `uv sync` (run from `python/`). Do not commit `.venv` or `uv.lock`? — DO commit `uv.lock` (reproducible resolution).

### schema.py

Pydantic v2 models named exactly as the Rust structs (`Entity`, `Event`, `Source`, `Claim`, `Evidence`, `CausalLink`, `TemporalRelation`, `ExtractionPayload`) with `Literal[...]` types for the enumerations and `schema_version: Literal[1] = 1`. Helpers:
- `slugify(text: str) -> str` (Global Constraint 10).
- `make_id(prefix: str, text: str) -> str` -> `f"{prefix}:{slugify(text)[:60].rstrip('-')}"`.
- `ExtractionPayload.normalized() -> ExtractionPayload`: de-duplicate every list by id (keep first); drop `subject_ids`/`actor_ids` entries that do not resolve; drop evidence whose `claim_id`/`source_id` is unknown; drop causal links / temporal relations with unknown endpoints; each drop logs `logger.warning("dropping <what> <id>: unknown reference <ref>")`. Returns a new model.
- `ExtractionPayload.check_integrity() -> None` raising `ValueError` on the first dangling reference or duplicate id (used by tests and by the CLI after normalization as a final assertion).

### sources/wikipedia.py and sources/arxiv.py

`SourceDocument` (frozen dataclass in `sources/__init__.py`): `id: str, provider: Literal["wikipedia", "arxiv"], title: str, url: str, published: str, retrieved_at: str, text: str`.

- `wikipedia.search_and_fetch(client: httpx.Client, query: str, limit: int, retrieved_at: str) -> list[SourceDocument]`:
  - search: `GET https://en.wikipedia.org/w/api.php` params `action=query, list=search, srsearch=<query>, srlimit=<limit>, format=json`;
  - per hit, extract: `GET https://en.wikipedia.org/w/api.php` params `action=query, prop=extracts|info, explaintext=1, exsectionformat=plain, inprop=url, titles=<title>, format=json` (one request per title); text = the page's `extract`, url = `fullurl` (fallback `https://en.wikipedia.org/wiki/<title with spaces as _>`);
  - id = `make_id("src", "wikipedia-" + title)`; `published = ""`.
  - Send header `User-Agent: IntelligenceWorkbench/0.1 (research worker; httpx)`.
- `arxiv.search(client: httpx.Client, query: str, limit: int, retrieved_at: str) -> list[SourceDocument]`:
  - `GET https://export.arxiv.org/api/query` params `search_query=all:<query>, start=0, max_results=<limit>`; parse Atom with `xml.etree.ElementTree` (namespace `http://www.w3.org/2005/Atom`); per entry: title (whitespace-collapsed), summary as text, `id` element as url, `published[:10]`; id = `make_id("src", "arxiv-" + <last path segment of the id url>)`.
- Fixture loaders (same modules): `wikipedia.load_fixture(fixture_dir: Path, retrieved_at: str) -> list[SourceDocument]` reads `wikipedia_search.json` (a real-shaped search response) and `wikipedia_extracts/<slugify(title)>.json` (real-shaped extract responses) and reuses the SAME parsing functions as the live path; `arxiv.load_fixture(fixture_dir, retrieved_at)` parses `arxiv.xml` with the same parser. Parsing must be factored so live and fixture paths share it (`parse_search_response`, `parse_extract_response`, `parse_atom`).
- All HTTP errors (`httpx.HTTPError`, bad status via `raise_for_status()`) are caught per source, logged with `logger.error(..., exc_info=True)`, and that source returns `[]` — the run continues with the other source. If BOTH sources return nothing the CLI exits 1 with an error log.

### normalize.py

`normalize(documents: list[SourceDocument]) -> pl.DataFrame` builds a frame with columns `id, provider, title, url, published, retrieved_at, text`; drops rows whose `text` is empty after strip; de-duplicates on `url` keeping first; truncates `text` to `MAX_DOC_CHARS = 6000`; sorts by `provider, title`. `to_documents(frame: pl.DataFrame) -> list[SourceDocument]` converts back. `to_sources(frame) -> list[Source]` builds the payload `Source` rows.

### llm.py

- `class ChatClient` with `__init__(self, base_url: str, api_key: str, model: str, client: httpx.Client | None = None, timeout: float = 120.0)`; method `complete_json(self, system: str, user: str) -> dict[str, object]` POSTs `{base_url}/chat/completions` with `Authorization: Bearer <key>`, body `{"model", "messages": [system, user], "temperature": 0, "response_format": {"type": "json_object"}}`; parses `choices[0].message.content` with `orjson`; if the content is wrapped in a ```json fence, strip the fence first. Raises `LlmError` (custom `Exception`) on HTTP or parse failures. Never logs the key or the URL with the key.
- `class FixtureChatClient` with `__init__(self, path: Path)`; `complete_json` returns the parsed JSON file regardless of input.
- `def client_from_env() -> ChatClient` reads `LLM_API_BASE` (default `https://api.openai.com/v1`), `LLM_API_KEY` (required), `LLM_MODEL` (required); raises `LlmError` naming the missing variable(s).
- `Completer = Protocol` with `complete_json` so `extract.py` accepts either.

### extract.py

- `SYSTEM_PROMPT` constant: instructs the model to act as an intelligence analyst extracting a knowledge graph; embeds the contract JSON shape, the enumerations, the id rules (prefixed slugs), the requirement that every `source_id` be one of the provided source ids, that evidence carry verbatim excerpts, that hypotheses be phrased as claims that could be true or false, that contradicting evidence be sought deliberately, and: "Never output probabilities, percentages, or predictions."
- `build_user_prompt(question: str, documents: list[SourceDocument]) -> str` lists each document as `=== SOURCE <id> | <provider> | <title> | <url> ===` followed by its text.
- `extract(question: str, documents: list[SourceDocument], completer: Completer) -> ExtractionPayload`: calls `complete_json` to get the raw dict, then builds `{**raw, "schema_version": 1, "question": question, "sources": [<Source rows built from documents>]}` (the model never authors `sources` or `question`; any it returns are overwritten), validates with `ExtractionPayload.model_validate`, applies `normalized()`, then `check_integrity()`.

### cli.py

`main(argv: list[str] | None = None) -> int` with argparse: `--question` (required), `--fixture-dir` (Path, optional), `--max-wikipedia` (int, 3), `--max-arxiv` (int, 3). Logging: `logging.basicConfig(level=INFO, stream=sys.stderr)`. `retrieved_at` = `"2026-09-14T00:00:00Z"` in fixture mode, else `datetime.now(UTC).isoformat(timespec="seconds").replace("+00:00", "Z")`. Pipeline: gather documents (fixture or live) -> `normalize` -> `to_documents` -> `extract` with `FixtureChatClient(fixture_dir / "llm_extraction.json")` or `client_from_env()` -> `sys.stdout.write(orjson.dumps(payload.model_dump(), option=orjson.OPT_INDENT_2).decode())` + newline -> return 0. Any exception: `logger.error("research run failed: %s", exc, exc_info=True)`; return 1. `if __name__ == "__main__": raise SystemExit(main())`.

### fixtures/offline (authored by this task)

Question: `Can export controls durably slow China's access to advanced semiconductor manufacturing capability?`

- `wikipedia_search.json`: MediaWiki-shaped search response with 3 hits titled `Semiconductor industry in China`, `United States export controls on semiconductors`, `Extreme ultraviolet lithography`.
- `wikipedia_extracts/<slugify(title)>.json`: MediaWiki-shaped extract responses; each `extract` is 400-900 words of plausible, neutral, fixture-authored prose (not copied from Wikipedia) covering: SMIC and its 7nm-class output, the October 2022 and October 2023 BIS rules, ASML's EUV export licensing, DUV multipatterning workarounds, Chinese subsidy programs, yield and cost constraints.
- `arxiv.xml`: Atom feed with 3 entries whose ids are `http://arxiv.org/abs/9999.00001v1` .. `9999.00003v1` (deliberately impossible ids), fixture-authored abstracts on (1) export-control effectiveness modelling, (2) domestic substitution dynamics under sanctions, (3) lithography multipatterning cost scaling. `published` dates in 2024-2025.
- `llm_extraction.json`: the fixture "LLM output" — an ExtractionPayload WITHOUT `sources` and `question` (extract.py injects them), containing at least: 7 entities across kinds `country` (China, United States, Taiwan), `organization` (SMIC, TSMC, ASML, BIS), `technology` (EUV lithography, DUV multipatterning), `policy` (October 2022 export controls); 5 events with real dates (2019 ASML EUV license withheld; 2022-10-07 BIS rule; 2023-08-29 Huawei Mate 60 Pro launch with SMIC 7nm; 2023-10-17 rule update; 2024-12-02 rule update) each with actor ids; 7 claims: 3 `hypothesis` (H1 controls durably slow China's advanced-node capability; H2 controls accelerate indigenous substitution; H3 DUV multipatterning sustains a 7nm-class workaround at high cost), 3 `fact`, 1 `assumption`; 12 evidence items referencing the 6 fixture source ids with stances arranged so that: H1 and H3 each have BOTH supporting and contradicting evidence (contested), at least one `fact` claim has supporting evidence from 2 distinct sources and no contradictions (consensus), and the `assumption` claim has no evidence (blind spot); 6 causal links forming at least one chain of 4 nodes (e.g. 2022 rule -> EUV access denied -> DUV workaround -> higher cost per wafer); 3 temporal relations among the events. Quality values vary between 0.4 and 0.9. No text anywhere in the fixtures may contain a percentage sign or the words probability/likelihood.
- `fixtures/payload/semiconductor.json`: generated by running `uv run --directory python iw-research --question "<question>" --fixture-dir fixtures/offline > fixtures/payload/semiconductor.json` and committed. Source ids in `llm_extraction.json` must be exactly the ids the loaders derive from the fixture files.

### Tests (python/tests)

- `test_schema.py`: slugify/make_id; `normalized()` drops dangling references with a warning (`caplog`) and de-duplicates; `check_integrity()` raises on a dangling reference; pydantic rejects a bad enum.
- `test_wikipedia.py`: `pytest-httpx` mocks the search and extract calls; asserts document fields, id form, and that an HTTP 500 on extract yields `[]` plus an error log.
- `test_arxiv.py`: mocks the Atom response; asserts parsing, id form, `published` slicing; malformed XML yields `[]` with an error log.
- `test_normalize.py`: empty-text drop, url de-dup keep-first, truncation to 6000, sort order, `to_sources` shape.
- `test_llm.py`: `ChatClient.complete_json` against a mocked endpoint (json body and fenced body); missing env raises `LlmError` naming the variable; key never appears in the log output.
- `test_extract.py`: with a stub completer returning `llm_extraction.json` content, `extract` yields a payload whose `sources` are the fixture documents, passes `check_integrity`, and has the counts stated above; a completer returning a dangling `claim_id` gets that evidence dropped.
- `test_cli.py`: `main(["--question", Q, "--fixture-dir", "<repo>/fixtures/offline"])` returns 0 and stdout (via `capsys`) parses to JSON equal to `fixtures/payload/semiconductor.json`; a missing fixture dir returns 1 and logs an error; the stdout contains nothing but the JSON document.

**Verification:** from `python/`: `uv run ruff check . && uv run ruff format --check . && uv run mypy src && uv run pytest -q` all clean. Commit (include `uv.lock`).

---

## Task 3: mnestic graph store — schema, atomic ingestion, Datalog and vector queries

**Goal:** Rust owns the database: opens `mem`/`sqlite`, initializes the schema idempotently, ingests a validated payload atomically, and exposes typed query methods for every query in the plan.

**Files:** `src/schema.rs`, `src/store.rs`; register both in `src/lib.rs`; add `From<StoreError> for AppError` in `src/error.rs` (maps to `AppError::Store(err.to_string())`).

### src/schema.rs

`pub const SCHEMA_DDL: &str`, `pub const INGEST_SCRIPT: &str`, and one `pub const` per query listed under "Datalog queries" (`ENTITIES`, `EVENTS`, `EVENT_ACTORS`, `SOURCES`, `CLAIM_SUBJECTS`, `EVIDENCE`, `CLAIMS_WITH_STANCE`, `CAUSAL_LINKS`, `CAUSAL_CHAINS`, `CRUXES`, `CONSENSUS`, `TEMPORAL_RELATIONS`, `SIMILAR_EVIDENCE`, `SIMILAR_CLAIMS`) — verbatim from the plan, each with a `///` doc comment stating what it returns and its parameters. `pub const MAX_CHAIN_NODES: usize = 6;` documents the recursion cap (the literal `6` stays in the script text).

### src/store.rs

```rust
pub struct GraphStore { db: DbInstance }
#[derive(Debug, thiserror::Error)]
pub enum StoreError { #[error("database error: {0}")] Db(String), #[error("unexpected row shape in {query}: {detail}")] RowShape { query: &'static str, detail: String } }
impl GraphStore {
    pub fn open_memory() -> Result<Self, StoreError>;                   // DbInstance::new("mem", "", "")
    pub fn open_sqlite(path: &Path) -> Result<Self, StoreError>;        // DbInstance::new("sqlite", path, "")
    pub fn init_schema(&self) -> Result<(), StoreError>;                // run "::relations"; if any row's name == "claim" return Ok; else run SCHEMA_DDL
    pub fn ingest(&self, payload: &ExtractionPayload, created_at: &str) -> Result<IngestReport, StoreError>;
    // query methods, all taking &self and returning Result<Vec<Row>, StoreError>:
    pub fn entities(&self, dossier_id: &str) -> Result<Vec<EntityRow>, StoreError>;
    pub fn events(&self, dossier_id: &str) -> Result<Vec<EventRow>, StoreError>;
    pub fn event_actors(&self, dossier_id: &str) -> Result<Vec<(String, String)>, StoreError>;
    pub fn sources(&self, dossier_id: &str) -> Result<Vec<SourceRow>, StoreError>;
    pub fn claim_subjects(&self, dossier_id: &str) -> Result<Vec<(String, String)>, StoreError>;
    pub fn evidence(&self, dossier_id: &str) -> Result<Vec<EvidenceRow>, StoreError>;
    pub fn claims_with_stance(&self, dossier_id: &str) -> Result<Vec<ClaimStanceRow>, StoreError>;
    pub fn causal_links(&self, dossier_id: &str) -> Result<Vec<CausalLinkRow>, StoreError>;
    pub fn causal_chains(&self, dossier_id: &str) -> Result<Vec<CausalChainRow>, StoreError>;
    pub fn cruxes(&self, dossier_id: &str) -> Result<Vec<CruxRow>, StoreError>;
    pub fn consensus(&self, dossier_id: &str) -> Result<Vec<ConsensusRow>, StoreError>;
    pub fn temporal_relations(&self, dossier_id: &str) -> Result<Vec<TemporalRow>, StoreError>;
    pub fn similar_evidence(&self, query_text: &str) -> Result<Vec<SimilarEvidenceRow>, StoreError>;
    pub fn similar_claims(&self, query_text: &str) -> Result<Vec<SimilarClaimRow>, StoreError>;
    pub fn dossier_question(&self, dossier_id: &str) -> Result<Option<String>, StoreError>;
}
```
Row structs (`Debug, Clone, PartialEq`): `EntityRow { id, name, kind, description }`, `EventRow { id, name, occurred_at, description }`, `SourceRow { id, title, url, provider, published, retrieved_at }`, `EvidenceRow { id, claim_id, source_id, stance, excerpt, quality: f64 }`, `ClaimStanceRow { id, text, kind, supports: i64, contradicts: i64 }`, `CausalLinkRow { cause_id, effect_id, mechanism, confidence }`, `CausalChainRow { start: String, end: String, path: Vec<String> }`, `CruxRow { id, text, supports: i64, contradicts: i64, downstream: i64, score: i64 }`, `ConsensusRow { id, text, sources: i64 }`, `TemporalRow { before_id, after_id, relation }`, `SimilarEvidenceRow { id, claim_id, source_id, stance, excerpt, distance: f64 }`, `SimilarClaimRow { id, text, distance: f64 }`, `IngestReport { dossier_id: String, entities: usize, events: usize, sources: usize, claims: usize, evidence: usize, causal_links: usize, temporal_relations: usize }`.

Implementation notes:
- `ingest` assumes the caller has already run `payload.validate()`; document this precondition in its doc comment and do not validate again. Build every `$param` as `DataValue::List` of `DataValue::List` rows; strings via `DataValue::from(&str)`; floats via `DataValue::from(f64)`; embeddings via `crate::embed::embed(text)` for claims (`text`) and evidence (`excerpt`), passed as `DataValue::List` of `DataValue::from(f64::from(x))`. `dossier_items` = `payload.all_item_ids()` paired with the dossier id. `created_at` is caller-supplied (RFC 3339). Run `INGEST_SCRIPT` with `ScriptMutability::Mutable` exactly once. Re-ingesting the same payload must succeed (`:put` upserts) and must not duplicate rows.
- Query helpers: a private `fn run(&self, query: &'static str, params: BTreeMap<String, DataValue>) -> Result<NamedRows, StoreError>` using `ScriptMutability::Immutable` for reads and mapping the mnestic error via `format!("{err:?}")` (mnestic errors are `miette::Report`; use Debug or `to_string()`; never panic). Private accessors `fn str_at(row: &[DataValue], idx: usize, query: &'static str) -> Result<String, StoreError>`, `int_at`, `float_at`, `str_list_at` that produce `StoreError::RowShape` on mismatch (mnestic returns `DataValue::Num(Num::Int)` for counts and `Num::Float` for distances; `get_int()`/`get_float()` cover both; `DataValue::List` for paths).
- Vector params: `embed(query_text)` -> `DataValue::List` -> script uses `vec($q)`.
- `Debug` for `GraphStore` should not dump the database (derive nothing; implement `Debug` printing `GraphStore { .. }`).

### Tests (`#[cfg(test)]` in store.rs)

Load `fixtures/payload/semiconductor.json` via `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/payload/semiconductor.json"))`.
- `init_schema` twice on the same store succeeds (idempotent guard).
- ingest the fixture into `mem`: report counts equal the fixture list lengths; `entities` / `events` / `sources` / `evidence` row counts match; `claims_with_stance` has one row per claim with zero-filled counts for the evidence-less claim; `causal_chains` contains a path of length >= 4; `cruxes` returns at most 3 rows, all with `supports > 0 && contradicts > 0`, sorted by score descending; `consensus` returns the intended fact claim(s) with `sources >= 2`; `temporal_relations` count matches; `similar_evidence("SMIC 7nm Huawei Mate 60")` returns 8 rows sorted by distance whose first row's `excerpt` mentions SMIC or Mate 60 (choose the fixture excerpt that makes this hold); `similar_claims` returns 5 rows sorted by distance; `dossier_question` returns the question; re-ingesting leaves every count unchanged.
- a handcrafted 1-entity payload with every list empty except entities ingests fine (verified against mnestic 0.18.0: `?[a, b] <- $rows` with an empty `DataValue::List` inside a `:put` block succeeds, so the script is static).
- atomicity: after `init_schema`, call the private mutable run helper (tests live in the same module) with a chained script whose first block `:put`s one entity and whose second block `:put`s into a nonexistent relation; assert the call errors and `entities` for that dossier stays empty (documents that the single-script ingest design is transactional).
- `open_sqlite` in a `tempfile::tempdir()`: init, ingest, reopen the same path, `dossier_question` still returns the question.
- graph scoping: ingest the fixture as dossier A, then a small unrelated handcrafted payload as dossier B; `entities(A)` never contains B's entity and vice versa; `similar_claims` (global) can return claims from both.

**Verification:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` clean. Commit.

---

## Task 4: Briefing synthesis — graph plus vector context to the 11 PRD sections

**Goal:** build a `BriefingContext` from `GraphStore` queries (graph legs scoped to the dossier, vector legs seeded by the question) and render the 11 sections in PRD §10 order deterministically, with no forecasting language.

**Files:** `src/briefing.rs`; register in `src/lib.rs`.

### Types

```rust
pub const SECTION_TITLES: [&str; 11] = [
    "Executive Overview", "Situation Summary", "Historical Context", "Causal Model",
    "Competing Hypotheses", "Consensus and Dissent", "Crux Analysis", "Counterfactual Analysis",
    "Evidence Assessment", "Open Questions", "Source Appendix",
];
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] pub struct Section { pub title: String, pub body: String }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] pub struct Briefing { pub dossier_id: String, pub question: String, pub sections: Vec<Section>, pub markdown: String }
#[derive(Debug, Clone, PartialEq)] pub struct BriefingContext { pub dossier_id, pub question, pub entities: Vec<EntityRow>, pub events: Vec<EventRow>, pub event_actors: Vec<(String, String)>, pub sources: Vec<SourceRow>, pub claim_subjects: Vec<(String, String)>, pub evidence: Vec<EvidenceRow>, pub claims: Vec<ClaimStanceRow>, pub causal_links: Vec<CausalLinkRow>, pub chains: Vec<CausalChainRow>, pub cruxes: Vec<CruxRow>, pub consensus: Vec<ConsensusRow>, pub temporal: Vec<TemporalRow>, pub similar_evidence: Vec<SimilarEvidenceRow>, pub similar_claims: Vec<SimilarClaimRow> }
pub fn build_context(store: &GraphStore, dossier_id: &str) -> Result<Option<BriefingContext>, StoreError>;   // Ok(None) when dossier_question(dossier_id) is None
pub fn render(ctx: &BriefingContext) -> Briefing;
pub fn quality_word(quality: f64) -> &'static str;   // high >= 0.7, medium >= 0.4, else low
```

### Section rules (deterministic; every section non-empty — write "No <thing> recorded." when a list is empty)

Use ASCII only in the markdown (`->` for causal arrows, `-` bullets). Refer to entities/events/claims by display name/text, never by raw id, except in the Source Appendix. A private `fn name_of(ctx, id) -> String` resolves any id to its entity name, event name, or claim text.

1. **Executive Overview**: one paragraph: the question; then "This dossier holds N entities, N events, N claims, N evidence items from N sources."; then "The situation turns on:" followed by the crux texts (up to 3) as a bullet list; then one line naming how many claims have consensus support and how many are contested.
2. **Situation Summary**: "Evidence closest to the question:" then the `similar_evidence` rows (max 8) as bullets `- (<stance>, <quality word>) <excerpt> [<source title>]`; then "Actors:" grouped by entity kind (`country`, `organization`, `technology`, `policy`, `person`, in that order, omitting empty kinds) as `- <kind>: name, name`.
3. **Historical Context**: "Timeline:" bullets `- <occurred_at or "undated"> - <event name>: <description> (actors: a, b)` sorted by date; then "Temporal relations:" bullets `- <before name> <relation> <after name>`.
4. **Causal Model**: "Mechanisms:" bullets `- <cause name> -> <effect name>: <mechanism> (confidence: <low|medium|high>)`; then "Longest causal chains:" the 5 longest chains as `- a -> b -> c` (names). Chains are de-duplicated by path.
5. **Competing Hypotheses**: for each claim with kind `hypothesis` (in `claims` order): `### H<n>: <text>` then "Supporting evidence:" bullets `- <excerpt> [<source title>, <quality word>]` and "Contradicting evidence:" bullets likewise, or "None recorded." Evidence comes from `ctx.evidence` filtered by claim id; source titles resolved from `ctx.sources`.
6. **Consensus and Dissent**: "Consensus (supported by at least two independent sources, uncontradicted):" bullets of `consensus` claim texts with `(N sources)`; "Dissent (claims with contradicting evidence):" for each claim with `contradicts > 0`: `- <text> - contradicted by: <source titles of its contradicting evidence>`; "Minority viewpoints worth considering:" the contradicting excerpts of the top crux (or "None recorded.").
7. **Crux Analysis**: for each crux (max 3): `### Crux <n>: <text>` then lines `- Supporting evidence items: <ns>`, `- Contradicting evidence items: <nc>`, `- Downstream effects in the causal graph: <nd>` and "Depends on this crux:" bullets of direct effect names (from `causal_links` where `cause_id == crux.id`), or "No modelled downstream effects."
8. **Counterfactual Analysis**: for each crux: `- If it were false that "<crux text>": the following would not follow: <downstream effect names via chains starting at the crux, de-duplicated, or "nothing modelled">`; then "Missed turning points:" for each event that is a `cause_id` in `causal_links`: `- Had "<event name>" not occurred: <effect names>`.
9. **Evidence Assessment**: "Sources by provider:" bullets `- <provider>: N`; "Evidence quality:" bullets `- high: N, medium: N, low: N` (counts of `quality_word` over `ctx.evidence`); "Stance balance:" `- supporting: N, contradicting: N`; "Blind spots:" bullets of claim texts with zero evidence (or "None recorded."); "Potential biases:" a fixed line: "All sources are secondary (encyclopedic or preprint); no primary documents, official statements, or industry data were consulted." when every provider is wikipedia/arxiv.
10. **Open Questions**: bullets: for each crux `- Which way does "<crux text>" resolve, and what evidence would settle it?`; for each blind-spot claim `- "<text>" is unverified: no evidence recorded.`; then "Information that would matter most:" one bullet per contested claim: `- New evidence on "<text>" from a source other than: <its current source titles>`.
11. **Source Appendix**: numbered list `1. <title> (<provider>, published <date or "n/a">, retrieved <retrieved_at>) - <url>` sorted by provider then title; then a trailing line `Identifiers: <id>, <id>, ...` listing source ids.

`markdown` = `# Briefing: <question>\n\n` + for each section `## <title>\n\n<body>\n\n` (trim trailing whitespace, single trailing newline). `sections` holds the same 11 bodies in order.

### Tests

Build a store, ingest `fixtures/payload/semiconductor.json`, `build_context`, `render`:
- exactly 11 sections with titles equal to `SECTION_TITLES` in order; markdown contains each `## <title>` in order (check ascending byte offsets);
- markdown does not match `\d+\s*%` (implement a tiny scanner, no regex crate) and contains neither `probability` nor `likelihood` case-insensitively;
- Crux Analysis names each `cruxes` row's text; Competing Hypotheses has one `### H` heading per hypothesis; Source Appendix contains every source url; Historical Context lists events in ascending date order; Counterfactual Analysis contains "If it were false that";
- `quality_word` boundaries (0.7 -> high, 0.69 -> medium, 0.4 -> medium, 0.39 -> low);
- `build_context` on an unknown dossier returns `Ok(None)`;
- an empty-ish dossier (1 entity, nothing else) still renders 11 non-empty sections containing "No " placeholders where applicable.

**Verification:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` clean. Commit.

---

## Task 5: Axum service, research worker subprocess, end-to-end tests, README

**Goal:** the running slice: `POST /api/questions` spawns the Python worker, ingests, and returns the briefing; integration tests prove Question -> Extraction -> Ingestion -> Retrieval -> Briefing both from the fixture payload and across the real Rust->Python process boundary.

**Files:** `src/config.rs`, `src/research.rs`, `src/app.rs`, `src/main.rs`, `tests/roundtrip.rs`, `README.md`; update `src/lib.rs` and `src/error.rs` (`From<ResearchError> for AppError` -> `AppError::Research(err.to_string())`).

### src/config.rs

```rust
#[derive(Debug, Clone)]
pub struct Config { pub bind: String, pub db_engine: DbEngine, pub db_path: PathBuf, pub uv_bin: PathBuf, pub python_dir: PathBuf, pub fixture_dir: Option<PathBuf>, pub worker_timeout: Duration }
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum DbEngine { Memory, Sqlite }
impl Config { pub fn from_env() -> Result<Self, ConfigError>; }
```
Env vars: `IW_BIND` (default `127.0.0.1:8080`), `IW_DB_ENGINE` (`mem` default | `sqlite`), `IW_DB_PATH` (default `data/iw.sqlite`), `IW_UV_BIN` (default `uv`), `IW_PYTHON_DIR` (default `python`), `IW_FIXTURE_DIR` (unset -> live mode), `IW_WORKER_TIMEOUT_SECS` (default `300`). `ConfigError` (thiserror) for unparsable values. LLM variables are NOT read by Rust; they pass through to the child process via inherited environment.

### src/research.rs

```rust
#[derive(Debug, Clone)] pub struct ResearchWorker { pub uv_bin: PathBuf, pub python_dir: PathBuf, pub fixture_dir: Option<PathBuf>, pub timeout: Duration }
#[derive(Debug, thiserror::Error)] pub enum ResearchError { #[error("failed to spawn research worker: {0}")] Spawn(String), #[error("research worker timed out after {0:?}")] Timeout(Duration), #[error("research worker exited with {status}: {stderr_tail}")] Failed { status: String, stderr_tail: String }, #[error("research worker returned invalid JSON: {0}")] InvalidJson(String), #[error("research worker payload rejected: {0}")] InvalidPayload(#[from] PayloadError) }
impl ResearchWorker { pub async fn run(&self, question: &str) -> Result<ExtractionPayload, ResearchError>; }
```
`run`: `tokio::process::Command::new(&self.uv_bin).args(["run", "--directory", <python_dir>, "iw-research", "--question", question])` plus `["--fixture-dir", <dir>]` when set; `stdout`/`stderr` piped; wrapped in `tokio::time::timeout`; on non-zero exit, `stderr_tail` = last 2000 chars of stderr; `tracing::warn!` the stderr tail on failure (stderr never contains secrets by construction; the key is never echoed). Parse stdout with `serde_json::from_slice::<ExtractionPayload>`, then `payload.validate()?`.

### src/app.rs

```rust
#[derive(Clone)] pub struct AppState { pub store: Arc<GraphStore>, pub worker: Arc<ResearchWorker> }
pub fn router(state: AppState) -> axum::Router;
```
Routes (Axum 0.8 path syntax `{id}`):
- `GET /health` -> `200 {"status":"ok"}`
- `POST /api/ingest` body `ExtractionPayload` -> validate (422 on error) -> `spawn_blocking` ingest with `created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)` -> `200 {"dossier_id": ..., "counts": IngestReport}`.
- `GET /api/dossiers/{id}/briefing` -> `spawn_blocking` (`build_context` + `render`) -> `200 Briefing` JSON or `404 {"error":"not found: dossier <id>"}`.
- `POST /api/questions` body `{"question": "..."}` -> `worker.run` -> ingest (as above) -> build+render -> `200 {"dossier_id": ..., "counts": IngestReport, "briefing": Briefing}`. Empty/whitespace question -> 422.
All handlers are `async fn ... -> Result<impl IntoResponse, AppError>`; blocking mnestic calls go through `tokio::task::spawn_blocking` (map `JoinError` to `AppError::Store`). Add `tower_http`? NO — keep to the declared dependencies; use `axum`'s built-in `Json` extractor/response only.

### src/main.rs

Initialize `tracing_subscriber` with `EnvFilter` (`RUST_LOG`, default `info`); `Config::from_env()`; open the store (`Memory` or `Sqlite` at `db_path`, creating parent dirs); `init_schema`; build `AppState`; `tracing::info!` the bind address (never log env values other than bind/engine/path); `axum::serve` on a `TcpListener`. `main` returns `anyhow::Result<()>` using `.context(...)`.

### tests/roundtrip.rs (integration)

Use `tower::ServiceExt::oneshot` against `router(state)` with a `mem` store; read bodies with `http_body_util::BodyExt::collect`.
1. `health_ok`.
2. `ingest_then_briefing_from_fixture_payload`: POST the fixture payload to `/api/ingest` -> 200, counts match fixture lengths; GET `/api/dossiers/{dossier_id}/briefing` -> 200; assert 11 sections in `SECTION_TITLES` order, the crux text from the payload appears, no forbidden forecasting patterns, and every source url appears in the Source Appendix.
3. `ingest_rejects_dangling_reference` -> 422 with an `error` string mentioning the bad id.
4. `briefing_unknown_dossier_404`.
5. `question_roundtrip_across_python_boundary` (the end-to-end proof): build `ResearchWorker { uv_bin: "uv", python_dir: <manifest>/python, fixture_dir: Some(<manifest>/fixtures/offline), timeout: 300s }`; POST `{"question": <fixture question>}` to `/api/questions` -> 200; `dossier_id == dossier_id_for(question)`; `counts` match the fixture; `briefing.sections.len() == 11`; the briefing markdown equals the markdown obtained by test 2's path for the same payload (proves the worker output equals the committed payload fixture). This test spawns the real `uv` process in fixture mode; it must not be `#[ignore]`d. If `uv` is missing the test fails with a clear message.
6. `question_worker_failure_is_502`: worker with `fixture_dir` pointing at a nonexistent directory -> 502 with `error` starting `research worker error`.

### README.md

Sections: what this is (Phase 1 slice, PRD §9/§10, no forecasting); architecture (Rust owns mnestic; Python stateless worker; contract); quick start (`uv sync` in `python/`, `cargo run`, example `curl` for `/api/questions` in fixture mode via `IW_FIXTURE_DIR=fixtures/offline`, and live mode with `LLM_API_BASE`/`LLM_API_KEY`/`LLM_MODEL`); how briefings are generated (Datalog rules for chains/cruxes/consensus, HNSW vector legs, deterministic renderer); testing commands for both stacks; design decisions and limits (deterministic hashed embeddings, deterministic prose renderer instead of LLM prose, fixture-backed end-to-end test, one dossier per question with deterministic id, vector legs global); what Phase 2 would add. No emoji.

**Verification:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` clean (including the subprocess test), and `python/` gates still clean. Commit.
