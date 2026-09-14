# Intelligence Workbench

Phase 1: a single, end-to-end analytical slice. Given a research question,
the system fetches source documents, extracts a structured research graph
from them, stores that graph, and renders an 11-section analytical
briefing from it. See PRD Section 9 (system overview) and Section 10 (the
briefing format) for the full specification.

This system never forecasts. It never emits a probability, a percentage,
or the words "probability" or "likelihood". It surfaces evidence, causal
structure, consensus, dissent, and open questions, and leaves judgment to
the reader.

## Architecture

Two processes, one contract.

- **Rust (`iw-server`)** owns the database, the HTTP API, and briefing
  rendering. It is the only process that opens the mnestic graph store; no
  other process talks to the database directly.
- **Python (`iw-research`)** is a stateless worker. Invoked once per
  question, it fetches source documents (Wikipedia, arXiv), asks an LLM to
  extract entities, events, claims, evidence, causal links, and temporal
  relations from them, and prints exactly one JSON document to stdout. It
  keeps no state between invocations and never opens the database.

The two processes share one contract: `ExtractionPayload`, implemented
independently in `src/model.rs` (Rust) and `iw_research/schema.py`
(Python), with `fixtures/payload/semiconductor.json` as the canonical
instance both test suites validate against. Rust spawns the worker as a
subprocess (`src/research.rs`), parses and re-validates its stdout, and
ingests it atomically (`src/store.rs`) as one dossier.

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

### Fixture mode (offline, no LLM required)

Set `IW_FIXTURE_DIR` and ask the fixture question; the worker reads
committed offline fixtures instead of hitting the network or an LLM:

```
IW_FIXTURE_DIR=fixtures/offline cargo run
```

```
curl -s -X POST http://127.0.0.1:8080/api/questions \
  -H "content-type: application/json" \
  -d "{\"question\": \"Can export controls durably slow China's access to advanced semiconductor manufacturing capability?\"}"
```

### Live mode

Unset `IW_FIXTURE_DIR` and export LLM credentials for the Python worker
(Rust never reads or logs these; they pass through via the inherited
process environment):

```
export LLM_API_BASE=https://api.openai.com/v1
export LLM_API_KEY=sk-...
export LLM_MODEL=gpt-4o-mini
cargo run
```

```
curl -s -X POST http://127.0.0.1:8080/api/questions \
  -H "content-type: application/json" \
  -d '{"question": "Will the EU AI Act reshape open-weight model releases?"}'
```

## API

- `GET /health` - liveness probe, `200 {"status": "ok"}`.
- `POST /api/ingest` - validates and ingests an `ExtractionPayload`
  directly (bypasses the research worker), `200 {"dossier_id", "counts"}`
  or `422` on a validation failure.
- `GET /api/dossiers/{id}/briefing` - renders the 11-section briefing for
  an already-ingested dossier, `200 Briefing` or `404` if unknown.
- `POST /api/questions` - `{"question": "..."}`; runs the research worker,
  ingests its output, and returns `200 {"dossier_id", "counts",
  "briefing"}`. `422` on an empty question, `502` if the worker fails or
  returns an invalid payload.

## Configuration

All variables are optional; each has a default.

| Variable                  | Default              | Meaning                                             |
| -------------------------- | --------------------- | ---------------------------------------------------- |
| `IW_BIND`                 | `127.0.0.1:8080`     | Socket address the service binds to.                |
| `IW_DB_ENGINE`             | `mem`                 | `mem` (non-persistent) or `sqlite`.                  |
| `IW_DB_PATH`               | `data/iw.sqlite`      | Sqlite file path, used when `IW_DB_ENGINE=sqlite`.   |
| `IW_UV_BIN`                | `uv`                  | Path to the `uv` binary.                             |
| `IW_PYTHON_DIR`            | `python`              | The `uv`-managed worker project directory.           |
| `IW_FIXTURE_DIR`           | unset (live mode)     | Offline fixture directory passed to the worker.      |
| `IW_WORKER_TIMEOUT_SECS`   | `300`                 | Seconds to wait for the worker before timing out.    |
| `LLM_API_BASE`             | `https://api.openai.com/v1` | Read only by the Python worker, in live mode. |
| `LLM_API_KEY`              | required in live mode | Read only by the Python worker, never by Rust.       |
| `LLM_MODEL`                | required in live mode | Read only by the Python worker, never by Rust.       |

## How briefings are generated

Ingestion writes one dossier's entities, events, sources, claims,
evidence, causal links, and temporal relations into mnestic in a single
atomic script: if any block fails, mnestic rolls back the whole ingestion
and no partial dossier is left behind.

Briefing synthesis (`src/briefing.rs`) then runs a fixed set of Datalog
queries (`src/schema.rs`) against that graph and renders their results
into the 11 PRD Section 10 sections with a deterministic Rust renderer,
not an LLM:

- **Causal chains** are found by recursive Datalog traversal of
  `causal_link` edges.
- **Cruxes** are claims with both supporting and contradicting evidence,
  ranked by contestation plus downstream causal reach.
- **Consensus** claims have at least two independent supporting sources
  and no contradictions.
- **Evidence and claim similarity** ("closest to the question") comes
  from two HNSW vector indexes over 256-dimensional feature-hashed
  embeddings (`src/embed.rs`), searched globally rather than scoped to
  one dossier.
- Evidence `quality` is stored as a `0.0..=1.0` float but only ever
  rendered as the word `high`, `medium`, or `low`.

## Testing

Rust:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`cargo test` includes an integration test that spawns the real Python
worker as a subprocess in fixture mode and proves its output round-trips
through ingestion and briefing rendering identically to the committed
payload fixture; it is not `#[ignore]`d and requires `uv` on `PATH` with
the worker's virtualenv already synced (`cd python && uv sync`).

Python:

```
cd python
uv run ruff check .
uv run ruff format --check .
uv run mypy src
uv run pytest -q
```

Python tests mock all network calls (`pytest-httpx`) and the LLM; no
network access is required.

## Design decisions and limits

- **Deterministic, keyless embeddings.** Similarity search uses
  256-dimensional feature-hashed vectors computed in Rust, not an
  embedding API. This makes retrieval reproducible and free of an
  external dependency, at the cost of weaker semantic matching than a
  learned embedding model would give.
- **Deterministic prose, not LLM-generated prose.** The briefing renderer
  is a plain Rust template over query results. This guarantees the
  no-forecasting constraint holds by construction and makes output
  reproducible for the same graph, at the cost of a more mechanical,
  less fluent read than an LLM-authored briefing.
- **Fixture-backed end-to-end testing.** The committed
  `fixtures/offline/` directory and `fixtures/payload/semiconductor.json`
  let the full Rust -> Python -> Rust path be tested without network
  access or a live LLM, at the cost of that path only ever being
  exercised against one fixed question in CI.
- **One dossier per question, deterministically.** A dossier's id is
  derived from its question text (`dos:` + a slugified, truncated hash of
  the question), so re-asking the same question re-ingests into the same
  dossier rather than creating a duplicate.
- **Vector search is global, not per-dossier.** The two HNSW indexes span
  every ingested dossier, so "claims closest to the question" can surface
  material from other dossiers. This is intentional for Phase 1's
  single-user, single-corpus scope.

## What Phase 2 would add

- A frontend and a persistent chat/session layer over the API.
- Additional source providers beyond Wikipedia and arXiv.
- Personalization (saved dossiers, per-user history).
- A GitHub integration or PR-review flow.
- Multiple concurrent LLM providers with routing/fallback.

None of these are in scope for Phase 1; see Global Constraint 13 in
`docs/plan-phase1.md`.
