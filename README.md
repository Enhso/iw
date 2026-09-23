# Intelligence Workbench

Given a research question, the system fetches source documents, filters and
extracts a structured research graph from them, stores that graph in a
bitemporal (as-of-queryable) corpus, and renders an 11-section analytical
briefing from it. See `docs/prd.md` (Section 9, Knowledge Representation, and
Section 10, the briefing format) and the cross-repo contract,
`docs/contracts.md` in the `betomcat` repository, for the full specification
this README summarizes.

This system never forecasts. Renderer-authored text never contains a
probability, a percentage, or the words "probability" or "likelihood": the
deterministic Rust briefing renderer cannot emit forecasting language by
construction. The Python worker additionally drops any LLM-authored claim or
causal-link mechanism that uses forecasting language, since nothing else
stops an LLM from writing "70% chance" straight into its output. Source
excerpts are exempt from that check and are quoted verbatim, since a source
may legitimately use a percentage (e.g. market share). The system surfaces
evidence, causal structure, consensus, dissent, and a claim's evidentiary
support, and leaves judgment to the reader.

IW is built to be embedded in a forecasting pipeline (the sibling `betomcat`
repository is one such caller): it resolves a question to a recurring
"family," fetches and filters news/wiki/reference material, extracts a
citable evidence graph, scores how well each claim is actually supported,
and answers "what do we already know, as of when" for a family of related
questions over time.

## Architecture

Two processes, one contract.

- **Rust (`iw-server`)** owns the database, the HTTP API, and briefing
  rendering. It is the only process that opens the mnestic graph store; no
  other process talks to the database directly.
- **Python (`iw-research`)** is a stateless worker. Invoked once per request
  as a subcommand CLI, it fetches source documents (AskNews news+wiki by
  default, Wikipedia+arXiv as the no-key fallback), runs a relevance/prompt-
  injection filter and an LLM extraction pass over them, scores each
  extracted claim's evidentiary support, and prints exactly one JSON
  document to stdout. It keeps no state between invocations and never opens
  the database.

The two processes share one contract: `ExtractionPayload` (schema v2),
implemented independently in `src/model.rs` (Rust) and
`python/src/iw_research/schema.py` (Python), with
`fixtures/payload/semiconductor.json` as a canonical instance both test
suites validate against. Rust spawns the worker as a subprocess
(`src/research.rs`), parses and re-validates its stdout, and ingests it
atomically (`src/store.rs`) as one dossier.

Optional gates, both built on a third service (TypeSafe's "Jev" evaluator),
run inside the Python worker and **fail open**: missing credentials, a
timeout, an HTTP error, or a malformed answer never fails a run or drops
anything a gate would not otherwise drop, they just log a `"skipped"` or
`"failed"` entry in the payload's `gate_log`.

- **Family routing** (`iw_research/family.py`): classifies a question
  against the live set of recurring "families" (e.g. "ECB rate decisions"),
  matching an existing family or minting a new one.
- **Relevance + injection filtering** (`iw_research/gates.py`): drops
  fetched documents that are off-topic for the question, or that contain
  text addressed to an AI system (prompt injection), before they reach
  extraction.
- **Claim support scoring** (`iw_research/gates.py`): scores how well each
  extracted claim is actually backed by its cited evidence excerpts, on a
  `0.0..1.0` scale.

## Quick start

Sync the Python worker's environment once:

```
cd python && uv sync
```

Run the server from the repository root:

```
cargo run
```

By default it binds `127.0.0.1:8080` with an in-memory store. See
[Configuration](#configuration) below for every environment variable.

### Fixture mode (offline, no network or LLM required)

Set `IW_FIXTURE_DIR` and call an endpoint; the worker reads committed
offline fixtures instead of hitting the network, AskNews, an LLM, or Jev:

```
IW_FIXTURE_DIR=fixtures/offline cargo run
```

```
curl -s -X POST http://127.0.0.1:8080/api/research \
  -H "content-type: application/json" \
  -d '{"question": "Can export controls durably slow China'"'"'s access to advanced semiconductor manufacturing capability?"}'
```

### Live mode

Unset `IW_FIXTURE_DIR` and export credentials for the Python worker (Rust
never reads or logs these; they pass through via the inherited process
environment):

```
export ASKNEWS_API_KEY=ank_...          # optional: news+wiki provider
export TYPESAFE_API_KEY=...             # optional: Jev gates
export LLM_API_BASE=https://openrouter.ai/api/v1
export LLM_API_KEY=sk-or-...
export LLM_MODEL=google/gemma-4-31b-it:free,openai/gpt-6-luna
cargo run
```

```
curl -s -X POST http://127.0.0.1:8080/api/research \
  -H "content-type: application/json" \
  -d '{"question": "Will the ECB cut its deposit rate at the October 2026 meeting?"}'
```

Without `ASKNEWS_API_KEY`, the worker falls back to the keyless Wikipedia +
arXiv providers. Without `TYPESAFE_API_KEY`, both Jev gates are skipped
(every fetched document is kept, and no claim gets a `support` score).
`LLM_API_KEY`/`LLM_MODEL` are required in live mode; see
[LLM model chain](#llm-model-chain-llm_model) for how to choose `LLM_MODEL`.

## API

Base URL `IW_BIND` (default `127.0.0.1:8080`). JSON everywhere. Errors:
`422` validation, `404` unknown id, `502` worker failure, `500` store
failure, all shaped `{"error": str}`. Full byte-level shapes are the
cross-repo contract, `docs/contracts.md` §C in `betomcat`; this is a
summary.

- **`GET /health`** — liveness probe, `200 {"status": "ok"}`.

- **`POST /api/families/classify`** — `{"question", "question_id": str|null,
  "context": {"resolution_criteria", "fine_print", "background"}|null}`.
  Runs the `classify-family` worker against the live families, mints a new
  family if nothing matched (id `fam:<slug>`, suffixed `-2`/`-3` on
  collision), records the question's tag, and returns `{"family_id",
  "label", "decision": "matched"|"minted"|"none", "probability", "method",
  "gate_log"}`.

- **`POST /api/research`** — `{"question", "question_id": str|null,
  "context": {...}|null, "family_id": str|null, "providers": [str]|null,
  "news_since": str|null, "max_news": int, "max_wiki": int}`. Resolves the
  family (and its `last_seen`, used as `news_since` for gap-fill), runs the
  `research` worker, validates and atomically ingests the returned
  `ExtractionPayload`, updates the family's `last_seen`, and returns:

  ```json
  {
    "dossier_id": "dos:...",
    "as_of": "2026-09-22T19:30:12Z",
    "counts": {"entities": .., "events": .., "sources": .., "claims": ..,
               "evidence": .., "causal_links": .., "temporal_relations": ..},
    "briefing": { /* the 11-section Briefing */ },
    "claims": [ClaimView, ...],
    "family_claims": [ClaimView, ...],
    "history": [HistoryItem, ...],
    "gate_log": [{"gate", "status": "ok"|"failed"|"skipped", "detail"}, ...],
    "dropped_sources": [{"url", "reason": "irrelevant"|"injection", "score"}, ...]
  }
  ```

  `claims` is this dossier's own claims, sorted by `support` descending
  (nulls last). `family_claims` is up to 30 claims from *other* dossiers
  tagged to the same family, newest first. `history` is `HistoryItem`s
  tagged to the same family. `ClaimView` is `{"claim_id", "text", "kind",
  "support": float|null, "support_method", "dossier_id", "evidence":
  [{"source_id", "title", "url", "provider", "published", "fetched_at",
  "content_hash", "stance", "excerpt"}]}`.

- **`GET /api/families`** — `[{"id", "label", "description", "created_at",
  "merged_into": str|null, "last_seen": str|null, "question_count"}]`.

- **`GET /api/families/{id}/claims?as_of=T&limit=N`** — `[ClaimView]` as the
  corpus stood at `T` (default now, `limit` default 50).

- **`GET /api/documents?as_of=T&family_id=F&limit=N&include_content=bool`**
  — the latest version of each matching document as of `T`:
  `[{"source_id", "url", "title", "provider", "published", "fetched_at",
  "content_hash", "content": str|null}]` (`content` populated only with
  `include_content=true`).

- **`POST /api/documents`** — `{"documents": [{"url", "title", "provider",
  "published", "fetched_at", "content"}]}`. Ingests raw documents without
  running extraction (e.g. a caller's degraded-mode outbox replay). Rust
  computes each document's content-addressed id and hash. Returns
  `{"ingested": int}`.

- **`GET /api/dossiers/{id}/briefing?as_of=T`** — renders the 11-section
  briefing for an already-ingested dossier as it would have looked at `T`,
  `200 Briefing` or `404` if unknown.

- **`GET /api/history?family_id=F&kind=personal|bot`** — `[HistoryItem]`.

- **`POST /api/history`** — `{"items": [HistoryItem]}`, upserted (each
  write is a new `tt` version, nothing is overwritten). `HistoryItem` is
  `{"id", "kind": "personal"|"bot", "title", "url", "question_type":
  "binary"|"multiple_choice"|"numeric"|"discrete"|"date", "forecast": <JSON:
  float for binary, {option: p} for multiple_choice, {"percentiles": {"5":
  x, ...}} or {"median": x} for numeric/discrete/date>, "resolution":
  str|null, "resolved_at": str|null, "family_id": str|null, "note"}`.

- **`POST /api/families/merge`** — `{"absorbed_id", "into_id"}`. Sets
  `absorbed_id`'s `merged_into` to `into_id` (prospective only: existing
  question tags are untouched; reads resolve the `merged_into` chain to the
  live family). `404` if either id is unknown.

`as_of` on every read endpoint is passed straight through to mnestic's
`:as_of` selector (see [The as-of corpus](#the-as-of-corpus-schema-v2)
below); a timestamp before the corpus's first write returns empty results,
not an error.

## The worker subcommand protocol

Rust writes one JSON request to the worker's **stdin** and reads exactly one
JSON document from **stdout**; the worker's own logs go to **stderr**. Exit
code `0` is success, anything else is failure (Rust surfaces the stderr tail
in the API's `502`).

```
uv run --directory python iw-research research        [--fixture-dir DIR]
uv run --directory python iw-research classify-family  [--fixture-dir DIR]
```

`research`'s stdin is `POST /api/research`'s body plus a resolved `family`
object (`{"id", "label", "last_seen"}|null`); its stdout is an
`ExtractionPayload` (schema v2, see below). `classify-family`'s stdin is
`{"question", "context", "families": [{"id", "label", "description"}, ...]}`
(only live, i.e. not merged-away, families); its stdout is
`{"decision", "family_id"|"label"+"description", "probability",
"jev_confidence", "method", "gate_log"}`.

`--fixture-dir DIR` switches both subcommands to fully offline fixture mode:
documents are read from `DIR` instead of fetched live, and the LLM/Jev
clients return canned answers from `DIR/llm_extraction.json`,
`DIR/llm_family_mint.json`, and `DIR/jev_answers.json` instead of making
network calls. `fixtures/offline/` is the repository's committed fixture
set; `fixtures/payload/semiconductor.json` is `research`'s exact stdout for
that fixture set, byte-compared in `python/tests/test_cli.py` and
re-verified across the real Rust→Python process boundary in
`tests/roundtrip.rs::research_roundtrip_across_python_boundary`.

Within `research`, the worker's own pipeline is: fetch (per requested
provider) → normalize (dedupe by url, truncate, sort) → relevance/injection
filter → LLM extraction, batched across documents and merged → claim
support scoring → assemble the payload, restoring every fetched document
to `sources[]` (including ones the relevance filter dropped: the corpus
keeps everything that was fetched, it just doesn't feed extraction).

## Configuration

All variables are optional; each has a documented default. `LLM_API_BASE`,
`LLM_API_KEY`, `LLM_MODEL`, `ASKNEWS_API_KEY`, `TYPESAFE_API_KEY`, and
`JEV_MODEL` are read only by the Python worker (via its inherited process
environment), never by Rust.

| Variable                  | Default                        | Meaning                                             |
| -------------------------- | ------------------------------- | ---------------------------------------------------- |
| `IW_BIND`                 | `127.0.0.1:8080`               | Socket address the Axum service binds to.            |
| `IW_DB_ENGINE`             | `mem`                           | `mem` (non-persistent, tests) or `sqlite` (deployment). |
| `IW_DB_PATH`               | `data/iw.sqlite`                | Sqlite file path, used when `IW_DB_ENGINE=sqlite`.   |
| `IW_UV_BIN`                | `uv`                            | Path to the `uv` binary.                             |
| `IW_PYTHON_DIR`            | `python`                        | The `uv`-managed worker project directory.           |
| `IW_FIXTURE_DIR`           | unset (live mode)               | Offline fixture directory passed to the worker. May be relative to the server's working directory; resolved to an absolute path before being passed to the worker. |
| `IW_WORKER_CMD`            | unset (`uv run --directory <IW_PYTHON_DIR> iw-research`) | Overrides the worker's command prefix, whitespace-split (e.g. a path to a stub script); the subcommand and `--fixture-dir` are still appended. Lets tests exercise the stdin/stdout protocol without a Python environment on `PATH`. |
| `IW_WORKER_TIMEOUT_SECS`   | `300`                           | Seconds to wait for the worker before timing out (`502`). |
| `LLM_API_BASE`             | `https://api.openai.com/v1`     | The worker's chat-completions API base url.          |
| `LLM_API_KEY`              | required in live mode           | Bearer token for `LLM_API_BASE`.                     |
| `LLM_MODEL`                | required in live mode           | Comma-separated fallback chain; see below.           |
| `ASKNEWS_API_KEY`          | unset (Wikipedia+arXiv fallback) | Enables the `asknews_news`/`asknews_wiki` providers, and makes them the request's default `providers` (contracts.md A1). |
| `TYPESAFE_API_KEY`         | unset (gates skipped)           | Enables the Jev-backed family routing, relevance/injection, and claim-support gates. |
| `JEV_MODEL`                | `jev-latest`                    | The Jev model requested at `/v1/systemone`.          |

**Upgrading.** A sqlite database created before the current schema (the
`tt: TxTime`/blob/family/history relations described below) must be
deleted: mnestic does not migrate an existing schema in place. Delete the
file at `IW_DB_PATH` (default `data/iw.sqlite`, already gitignored under
`/data/`) and let the server recreate it on next startup.

### LLM model chain (`LLM_MODEL`)

`LLM_MODEL` is a comma-separated fallback chain, tried in order on a
request failure, an HTTP error status (e.g. 429/503), or unparsable JSON
content, e.g. `LLM_MODEL=google/gemma-4-31b-it:free,openai/gpt-6-luna`.
Every model in the chain is checked against a frontier denylist
(`anthropic/claude-opus*`, `anthropic/claude-sonnet*`,
`anthropic/claude-fable*`, `openai/gpt-6-astra*`, `openai/gpt-6-sol*`,
`openai/gpt-5.5*`, `*-pro`) before any request is made; the worker refuses
to start with a `LlmError` naming the offending model(s) if any entry
matches (contracts.md B2: "no frontier spend in the worker, ever").

**Recommended chain**, from a live evaluation on 2026-09-23 (two real
questions, "Will either Gulf Cup 27 semi-final on 3 October 2026 be decided
by a penalty shootout?" and "Will the ECB cut its deposit rate at the
October 2026 meeting?", against `openrouter.ai`):

```
LLM_MODEL=google/gemma-4-31b-it:free,openai/gpt-6-luna
```

- `google/gemma-4-31b-it:free` is free and, per the OpenRouter account's
  provider allowlist, the only `:free` model this key can currently reach
  at all (see `docs/BUILD_LOG.md` in `betomcat`). It is worth trying first
  purely on cost — but during this evaluation it failed two different ways
  on real (4-document-batch-sized) extraction prompts: `503 Provider
  returned error` / `"This model is currently experiencing high demand"`
  from Google AI Studio on repeated attempts, and, once that cleared, a
  truncated, invalid-JSON completion (caught by the worker's strict JSON
  parse, so it fails loudly rather than returning a partial payload). A
  minimal, single-document smoke prompt against the same model completed
  cleanly (`finish_reason: "stop"`), so the truncation is specifically a
  function of the larger extraction prompt, not a broken model — but it
  means this model should never be configured alone, only as the first
  link in a chain with a paid fallback.
- `openai/gpt-6-luna` was tested standalone (both live questions succeeded
  on the first attempt) and produced strong output: 28 and 56 claims
  respectively, every claim backed by at least one verbatim evidence
  excerpt, all successfully scored by the claim-support gate, and no
  forecasting-language or question-restatement leakage. Above the ~8-25
  claims/question target on the busier ECB question, because total claim
  count scales with the number of extraction batches (see
  [Batched extraction](#batched-extraction) below) — a real trade-off of
  batching, not a defect: every one of those claims is still individually
  cited and support-scored.

Repeat this evaluation periodically (model availability and pricing shift)
and update this section; `iw_research.llm._FRONTIER_DENYLIST` is the only
place the worker itself enforces a rule about which models are acceptable.

## The as-of corpus (schema v2)

Every mnestic relation in this system carries a trailing `tt: TxTime` key
column, engine-stamped at commit and never supplied by a caller. A `:put`
on a `tt`-stamped relation appends a new version; nothing already written is
ever overwritten. Reading without an `:as_of` selector returns current
state; `:as_of "T"` reproduces the corpus, and therefore the briefing, and
therefore any claim's `support`, exactly as it stood at `T` — with no
leakage from later writes by construction. `src/store.rs`'s bitemporal
relations:

```
blob             {content_hash: String => content: String}            -- plain, immutable
source           {id, tt: TxTime => title, url, provider, published,
                  retrieved_at, content_hash}                         -- a document version
family           {id, tt: TxTime => label, description, created_at, merged_into}
family_seen      {family_id, tt: TxTime => last_seen}
question_family  {question_id, tt: TxTime => family_id, probability: Float?,
                  method, question}
dossier_question {dossier_id, question_id, tt: TxTime}
claim_support    {dossier_id, claim_id, tt: TxTime => support: Float?, method}
history_item     {id, tt: TxTime => kind, title, url, question_type,
                  forecast: Json, resolution, resolved_at, family_id, note}
```

plus every pre-existing relation (`entity`, `event`, `claim`, `evidence`,
`causal_link`, `temporal_relation`, `dossier_item`, ...) gaining the same
trailing `tt`. Document bodies are content-addressed: `blob` is keyed by
`content_hash` (`sha256` of the exact content string) and is never
versioned itself, since the same content re-appearing under a new `source`
version is still the same bytes.

**Vectors live outside the bitemporal relations.** mnestic rejects
`::hnsw create` on a relation with a `tt: TxTime` key column (verified in
`src/store.rs::hnsw_index_creation_is_rejected_on_a_txtime_relation`, both
storage engines). Claim and evidence embeddings therefore live in plain,
un-versioned side relations, `claim_vecs {id => embedding}` and
`evidence_vecs {id => embedding}`, each HNSW-indexed as before: a vector is
a pure function of its text, so it needs no history of its own, and the two
indexes can still be searched globally (`as_of` scoping for a *search
result* comes from resolving the returned ids against the bitemporal
relations afterward, not from the index itself).

**Storage engine in deployment is `sqlite`** (`IW_DB_ENGINE=sqlite`); the
in-memory default (`mem`) is for tests and any run that does not need to
persist across restarts.

## How briefings are generated

Ingestion writes one dossier's entities, events, sources, claims, evidence,
causal links, and temporal relations into mnestic in a single atomic
script: if any block fails, mnestic rolls back the whole ingestion and no
partial dossier is left behind.

Briefing synthesis (`src/briefing.rs`) then runs a fixed set of Datalog
queries (`src/schema.rs`) against the graph *as of* the requested time and
renders their results into the 11 PRD Section 10 sections with a
deterministic Rust renderer, not an LLM:

- **Causal chains** are found by recursive Datalog traversal of
  `causal_link` edges.
- **Cruxes** are claims with both supporting and contradicting evidence,
  ranked by contestation plus downstream causal reach.
- **Consensus** claims have at least two independent supporting sources and
  no contradictions.
- **Evidence and claim similarity** ("closest to the question") comes from
  the two HNSW vector indexes described above, searched globally rather
  than scoped to one dossier.
- Evidence `quality` is stored as a `0.0..=1.0` float but only ever
  rendered as the word `high`, `medium`, or `low`. Claim `support` (the
  Jev-scored gate, distinct from `quality`) is surfaced as its own field on
  `ClaimView`, not folded into the prose renderer.

## Testing

Rust:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`cargo test` includes integration tests that spawn the real Python worker
as a subprocess in fixture mode and prove its output round-trips through
ingestion and briefing rendering (`tests/roundtrip.rs`); not `#[ignore]`d,
so they require `uv` on `PATH` with the worker's virtualenv already synced
(`cd python && uv sync`).

Python:

```
cd python
uv run ruff check .
uv run ruff format --check .
uv run mypy src
uv run pytest -q
```

Python tests mock all network calls (`pytest-httpx`) and the LLM/Jev
clients; no network access is required.

## Design decisions and limits

- **Deterministic, keyless embeddings.** Similarity search uses
  256-dimensional feature-hashed vectors computed in Rust, not an embedding
  API. This makes retrieval reproducible and free of an external
  dependency, at the cost of weaker semantic matching than a learned
  embedding model would give.
- **Deterministic prose, not LLM-generated prose.** The briefing renderer
  is a plain Rust template over query results. This guarantees the
  no-forecasting constraint holds by construction and makes output
  reproducible for the same graph, at the cost of a more mechanical, less
  fluent read than an LLM-authored briefing.
- **Fixture-backed end-to-end testing.** The committed `fixtures/offline/`
  directory and `fixtures/payload/semiconductor.json` let the full
  Rust → Python → Rust path be tested without network access or a live
  LLM, at the cost of that path only ever being exercised against one
  fixed question in CI.
- **One dossier per question, deterministically.** A dossier's id is
  `dos:<slug>-<8 hex digit hash of the question>`: a slugified, truncated
  (60 characters) form of the question, followed by 8 hex digits of an
  FNV-1a hash of the full trimmed question. The hash disambiguates
  questions that agree on their first 60 slugified characters, or that
  have no ASCII alphanumeric characters at all. Re-asking the same
  question re-ingests into the same dossier rather than creating a
  duplicate.
- **Nodes are shared, edges and membership are per dossier, vector search
  is global.** Entities, events, sources, claims, and evidence are global
  records keyed by id and can be shared across dossiers (an LLM routinely
  re-derives the same slug for the same real-world entity). Edges
  (`event_actor`, `claim_subject`, `causal_link`, `temporal_relation`) and
  dossier membership (`dossier_item`) are scoped by `dossier_id`, since an
  edge or a membership claim is an assertion made inside one dossier. The
  two HNSW vector indexes span every ingested dossier, so "claims closest
  to the question" and "evidence closest to the question" can surface
  material from other dossiers; this is intentional for the current
  single-user, single-corpus scope.
- **Every Jev gate fails open, by design.** Family routing, the
  relevance/injection filter, and claim support scoring all degrade to
  "skip this gate" rather than failing the run when `TYPESAFE_API_KEY` is
  unset or the call errors. A forecasting pipeline calling this API cannot
  be taken down by a third-party evaluator outage; it just loses that
  gate's signal for the affected request (visible in `gate_log`).
- **The relevance filter keeps a floor of documents, not just a
  threshold.** A Jev relevance score below `RELEVANCE_DROP_THRESHOLD`
  (0.15) is usually dropped, but if fewer than `MIN_KEPT_DOCUMENTS` (5)
  documents pass that threshold, the highest-scoring remainder is promoted
  to keep the total at 5 (or all fetched documents, if fewer than 5 exist).
  Background, history, and schedule material routinely scores low against
  a literal "does this bear on how the question resolves" reading, but is
  exactly the material a forecaster needs for base rates; the floor stops
  a narrow relevance read from starving extraction down to a handful of
  documents. Injection drops are never promoted, regardless of how few
  documents that leaves — see `python/src/iw_research/gates.py`.

### Batched extraction

A single `research` question can have far more kept (post-relevance-filter)
documents than one LLM completion call can meaningfully extract from,
especially against a free-tier model's output token budget. Once the kept
document count exceeds `BATCH_SIZE` (4), `iw_research.extract.extract_batched`
splits them into `BATCH_SIZE`-sized groups, runs one extraction call per
group concurrently, and merges the resulting payloads:

- Entities, events, claims, evidence, and sources are merged by id and
  de-duplicated (`ExtractionPayload.normalized()`): an LLM routinely
  re-derives the same slug for the same real-world entity or re-cited
  source across batches, so this is expected and desired.
- `causal_links` and `temporal_relations` carry no `id` field for that
  dedup to key on, so `extract._merge_payloads` separately drops exact
  duplicates of those two lists by full content, after concatenation.
- Total claim count scales roughly with the number of batches, since each
  batch's own extraction call independently aims for a rich set of claims
  from its own documents; a question with many kept documents can
  therefore produce more than the nominal ~8-25 claims/question target.
  This is treated as an acceptable trade-off (every claim is still
  individually cited and support-scored) rather than something the merge
  step artificially caps.
- Near-duplicate claims that restate the same fact in different words
  across overlapping source articles are **not** deduplicated (their ids,
  derived from claim text, differ) — that would need semantic/embedding
  similarity, out of scope for this worker.

## What Phase 2 would add

- A frontend and a persistent chat/session layer over the API.
- Additional source providers beyond AskNews, Wikipedia, and arXiv.
- Personalization beyond the `history_item` relation already in schema v2
  (e.g. per-user corpora, rather than one shared corpus).
- Semantic (embedding-based) de-duplication of near-identical claims
  produced by batched extraction (see above).

None of these are in scope for the current phase; see Global Constraint 13
in `docs/plan-phase1.md`.
