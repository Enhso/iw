# Intelligence Workbench — Learning Plan

Goal: go from "can read the spec" to "can implement `prompt_plan.md` and debug it" across the three components (Rust/Axum orchestrator, Python/`uv` agent, vanilla-JS frontend) and the GitOps data model.

**Calibration (from `CLAUDE.md`): Python expert, Rust novice.** So the effort split is roughly **Track B (Rust/backend) ≈ 55%**, **Track A (cross-cutting concepts) ≈ 20%**, **Track D (frontend) ≈ 15%**, **Track C (Python tooling) ≈ 10%**. Don't relearn Python; do learn Rust's ownership/async model, git plumbing, and DOM-without-a-framework.

**How to use this:** learn in the order of §7 (Sequenced Path), not top-to-bottom. Each unit ends with a *checkpoint* — a small standalone exercise that proves the concept *before* you apply it to the real codebase. Time bands are rough and pace-dependent.

---

## 1. First: the system's mental model

Internalize these before any code. Everything else is detail hanging off them.

1. **The GitHub repo is the database; the app is stateless.** There is no SQL store, no server-side persistence of dossiers. Every read fetches from the repo via the GitHub REST API; every write commits back. If you lose the server, you lose nothing. Implication: "data access" in this project *is* "HTTP calls to GitHub."
2. **The core loop is GitOps:** user triggers research → agent writes changes to a *branch* (`ai-update/{topic}`) → opens a *pull request* → analyst reviews the *diff* → resolves conflicts → *merges* to `main`. Human edits and AI edits are reconciled by git's merge machinery, not by application logic.
3. **Three processes, two languages, one contract.** Rust serves the UI + API and *orchestrates*. Python does research + LLM synthesis + the PR write. They communicate by Rust spawning the Python CLI as a subprocess and parsing a **single line of JSON from stdout** (frozen in P17). The frontend talks only to Rust, over REST + SSE.
4. **Two LLM call-sites, two styles.** Interactive chat = Rust streaming tokens to the browser over SSE (low latency). Batch research synthesis = Python, returning a JSON map of `filename → new content` (structured output). Same provider, different shapes.

Recap diagram (re-read `spec.md §1`): Browser ⇄ Rust(Axum) ⇄ GitHub API; Rust ⇄ Python(subprocess) ⇄ GitHub API + research APIs + LLM.

**Checkpoint:** without looking, draw the request lifecycle for (a) opening a dossier tab and (b) running research to a merged PR. If you can't name every hop, re-read `spec.md §4`.

---

## 2. Track A — Cross-cutting concepts (language-agnostic)

These are the ideas you need regardless of language. ~½–1 day each, mostly reading + one small drill.

| Concept | What / why it matters here | Where it shows up | Resource |
|---|---|---|---|
| **GitOps / repo-as-DB / statelessness** | The whole persistence model. Understand "single source of truth," idempotent reads, eventual reconciliation via PRs. | Entire architecture; `spec.md §3` | Git book (below) + read `spec.md §3` closely |
| **Async / non-blocking I/O** | Both Tokio and JS are single-threaded event loops doing concurrent I/O. One concept, two runtimes. Know: blocking vs non-blocking, why you never block the loop, futures/promises, cooperative scheduling. | Tokio (B2), Axum (B3), JS fetch/SSE (D) | async-book intro; MDN "Asynchronous JS" |
| **HTTP client/server + REST** | Resources, verbs (GET/PUT/PATCH/DELETE), status codes, JSON bodies, headers, auth. The GitHub API and your own API are both REST. | All endpoints; GitHub calls | MDN HTTP overview; GitHub REST overview |
| **Server-Sent Events (SSE)** | One-way server→client token stream over a long-lived HTTP response (`text/event-stream`, `data:` lines). Chosen over WebSockets because traffic is one-directional. Note: `EventSource` is GET-only, which is why chat streams via `fetch`+`ReadableStream` (P14). | Chat (P13/P14), progress events (P18/P19) | MDN Server-sent events |
| **Bearer-token auth & secret hygiene** | PAT in `Authorization: Bearer …`; secrets only in env/`.env`; never log tokens or URLs containing them; redact in Debug. | Setup (P5), every GitHub call, subprocess env (P18) | GitHub auth docs; `CLAUDE.md` Security |
| **Git plumbing (the part that matters)** | refs vs branches; commit→tree→blob; a branch is a movable pointer; **3-way merge** (ancestor/ours/theirs); conflict markers `<<<<<<< / ======= / >>>>>>>`; what "mergeable" means; PRs as a compare+merge UI over two refs. This is the conceptual core of the PR reviewer. | GitHub client (P7), agent writes (P16/P17), PR reviewer (P20–P22) | Pro Git: "Git Branching" + "Git Internals" chapters |
| **Markdown + YAML frontmatter + sanitization** | Markdown→HTML rendering; frontmatter is a leading `---…---` YAML block carrying metadata; **HTML sanitization** prevents XSS when you `innerHTML` rendered content. | Render (P9), dossier model (P8), workspace (P11) | CommonMark spec (skim); OWASP XSS (skim) |
| **LLM integration patterns** | (a) *Grounding*: stuff full context into the prompt (`spec.md §4.4`). (b) *Streaming*: consume incremental token deltas. (c) *Structured output*: instruct the model to return parseable JSON, then strip fences + parse. | Chat (P13), synthesis (P16) | Your provider's chat-completions + streaming reference |

**Checkpoint (git):** in a scratch repo, create a branch, make conflicting edits on both branches, merge, and resolve the conflict by hand. You must be able to read the markers fluently — the reviewer UI (P22) reconstructs exactly these.

---

## 3. Track B — Rust + the backend stack (the main lift)

Goal is *working competence*, not mastery. Resist rabbit-holes (lifetimes beyond the basics, macro internals, unsafe). Learn the subset this project uses.

### B0 — Rust fundamentals (~3–6 days)
Learn, in this order, only what the backend uses:
- **Ownership, borrowing, moves; `&T` / `&mut T`; lifetimes (just enough to read signatures).** This is the genuinely new mental shift coming from Python. Budget the most time here.
- **`Result<T,E>` and `Option<T>`; the `?` operator; `match`; `if let` / `while let`.** Error propagation is everywhere; `.unwrap()` is banned outside tests (`CLAUDE.md`).
- **Enums + exhaustive pattern matching** (e.g. `AppError`), **structs**, **traits** (think interfaces), **derive macros** (`Debug`, `Clone`, `Serialize`).
- **Modules & `use`; `Cargo.toml`, crates, features; `cargo build/run/test/clippy/fmt`.**
- **Common types:** `String`/`&str`, `Vec<T>`, `HashMap`/`BTreeMap`, `Arc`, iterators/combinators.
- **Newtype pattern** (`Owner(String)`, `Sha(String)`) and why it prevents bugs.

Resources: *The Rust Book* ch. 1–10, 13 (closures/iterators), 17 (traits/objects as needed); *Rustlings* for muscle memory; *Rust by Example* for lookups.

**Checkpoint:** write + test a `parse_dossier_frontmatter(&str) -> Result<Meta, MyError>` that splits a `---…---` block and returns a struct. This is literally P8's core function — you're pre-building it as a kata.

### B1 — Error handling idiom (~½ day)
- `thiserror` for library error enums; `anyhow` + `.context()` for app-level glue.
- The project's keystone pattern: an `AppError` enum that implements Axum's `IntoResponse` so every handler returns `Result<impl IntoResponse, AppError>` and failures become clean HTTP responses (verbatim in `spec.md §6.1`).
Resources: docs.rs/thiserror, docs.rs/anyhow.

### B2 — Async Rust + Tokio (~3–5 days; second-biggest lift)
- `async fn`, `.await`, what a `Future` is, the runtime/executor, `#[tokio::main]`.
- `tokio::spawn` vs **`tokio::task::spawn_blocking`** (offload CPU-bound work — markdown render, base64, diff — off the reactor; required by `CLAUDE.md`).
- Shared state: `Arc<RwLock<T>>` (config that mutates after setup), prefer `RwLock` over `Mutex` when reads dominate.
- Channels: **`tokio::sync::broadcast`** (the SSE progress fan-out in P18), `mpsc` awareness.
- `Stream` (async iterator) — underpins the SSE token stream.
Resources: **Tokio tutorial** (do it end-to-end), async-book for the model.

**Checkpoint:** an async function that fires 3 concurrent HTTP GETs with `try_join_all` and returns when all finish — mirrors `list_dossiers` (P8).

### B3 — Axum (~2–4 days)
- `Router`, route methods, path/query/JSON **extractors**, **`State`** injection, returning `Json`/`Html`/`impl IntoResponse`.
- **Tower middleware / layers**: `TraceLayer`, timeout, compression (`tower-http`); ordering matters (and so does putting `/api/*` above the static `ServeDir` fallback).
- **`axum::response::sse::Sse`** for streaming — chat (P13) and `/api/events` (P18).
- Serving static files with `ServeDir` + SPA fallback (P3).
Resources: docs.rs/axum (read the top-level module docs), and the **axum `examples/` directory** (look specifically at `sse`, `error-handling`, `todos`, `static-file-server`).

**Checkpoint:** a ~40-line Axum app: `GET /health` (JSON), `GET /stream` (SSE emitting 5 ticks then a `done` event), `with_state` holding a counter behind `Arc<RwLock>`. Proves B2+B3 together — it's the spine of P1/P13/P18.

### B4 — reqwest (HTTP client) (~1 day)
- Build one shared `Client` (connection reuse); set default headers/User-Agent/timeout.
- `RequestBuilder`: `.bearer_auth()`/headers, `.json(&body)`, `.send().await?`, `.json::<T>().await?`, status checks.
- A private "authorized request" helper to stay DRY across GitHub calls.
Resources: docs.rs/reqwest.

### B5 — serde (~1 day)
- `#[derive(Serialize, Deserialize)]`; `#[serde(rename = "last-updated")]` for hyphenated keys; defaults; `serde_json` for the JSON boundary; a maintained YAML crate for frontmatter (**confirm the current crate name/version** — flagged in `prompt_plan.md`).
- Pattern: model *only the fields you consume* from GitHub responses.
Resources: serde.rs.

### B6 — Supporting crates (~½ day, reference as needed)
| Crate | Role | Appears in |
|---|---|---|
| `tracing` + `tracing-subscriber` | structured logging/spans; `error!`/`info!` (never `println!`) | P1 onward |
| `secrecy` | wrap PAT/LLM key as `SecretString`; guards Debug leaks | P4/P5/P13 |
| `dotenvy` | load `.env` into env | P4/P5 |
| `base64` | decode/encode GitHub Contents API payloads | P6/P7 |
| `pulldown-cmark` | Markdown→HTML | P9 |
| `ammonia` | sanitize that HTML (XSS) | P9 |
| `similar` | unified text diff (read-only view) | P20 |
| `diffy` | **3-way merge** → clean result or conflict-marked text | P20 |
| `chrono` | parse/format `last-updated` timestamps | P8 |
| `futures` | `try_join_all`, stream combinators | P8/P13 |

### B7 — Testing in Rust (~1 day)
- `#[cfg(test)] mod tests` + `#[test]` / `#[tokio::test]`; Arrange-Act-Assert.
- **`wiremock`**: stand up a fake GitHub/LLM server, point the client's base URL at it (`GITHUB_API_BASE` override), assert on requests and feed canned responses. **Every external call is mocked** — no live HTTP in tests (`CLAUDE.md`).
Resources: docs.rs/wiremock; the Rust Book testing chapter.

**Checkpoint:** mock a GitHub `GET /repos/{o}/{r}/contents/{path}` with a base64 body and test that your `get_file_content` decodes it — this is a P6 test.

### B8 — Subprocess orchestration (~½ day)
- `tokio::process::Command`: set `current_dir`, pass **secrets via `.env(...)` not argv**, capture stdout/stderr, check exit status, parse the single-line JSON contract from P17 into a typed struct.
Resources: docs.rs/tokio (process module).

---

## 4. Track C — Python agent stack (you know Python; learn the tooling)

Skip language basics. Focus on the unfamiliar tools and the project's strict discipline (~2–3 days total).

| Tool | What's new vs. your habits | Appears in | Resource |
|---|---|---|---|
| **`uv`** | Replaces pip/venv/poetry: `uv init`, `uv add`, `uv run`, lockfile, `.venv` management. Why: speed + reproducibility. | P2, all agent prompts | docs.astral.sh/uv |
| **`ruff` + `mypy`** | Lint+format (ruff) and **strict static typing** (mypy, no `Any`). The project gates on these. Treat type hints as mandatory, not optional. | P2 onward | docs.astral.sh/ruff; mypy docs |
| **`httpx`** | Modern requests-replacement with timeouts and a sync+async API; mocked in tests. | P15/P16 | python-httpx.org |
| **`polars`** | Not pandas. Learn: eager vs lazy frames, the **expression API** (`pl.col(...).…`), joins, dedup, `unique`. Used to normalize/dedupe multi-source research rows. `CLAUDE.md` rules: don't print row-count+schema next to a frame; never ingest >10 rows in an analysis helper. | P15 | docs.pola.rs (user guide + Python API ref) |
| **`orjson`** | Faster JSON; returns `bytes`. Used for the stdout contract and LLM-JSON parsing. | P16/P17 | github.com/ijl/orjson |
| **`pytest` + `unittest.mock`** | `patch`/`MagicMock` to stub every network call; save tests as discrete files; keep generated outputs; isolate + gitignore the output dir. | all agent tests | pytest docs; unittest.mock docs |

**Agent-specific patterns to study (not a library, but the actual design):**
- **Provider abstraction** (P15): keyless sources (Wikipedia, arXiv) run by default; keyed sources (Tavily, Semantic Scholar) activate only when their env key exists; a failing provider logs and is skipped, never aborts the run.
- **Structured LLM output** (P16): prompt for a JSON map, strip code fences, `orjson.loads`, return only changed files.
- **The error protocol** (P17, `spec.md §6.2`): wrap the run; on exception → `logger.error(exc_info=True)` to **stderr** + `raise SystemExit(1)`; keep **stdout clean** so Rust never parses partial JSON.

**Checkpoint:** write `fetch_research(query)` that calls two `httpx`-mocked providers, builds a Polars frame, drops duplicate URLs, and is fully tested with `unittest.mock`. That's P15 standalone.

---

## 5. Track D — Frontend without a framework (a distinct skill)

The constraint (`CLAUDE.md` + `spec.md`): Pico CSS + custom CSS + **vanilla ES6+ JS**, no React/Svelte/Vue/jQuery. If you're used to frameworks (or to avoiding frontend), this is real new ground. ~2–3 days.

**The "no framework" mindset:** you manage the DOM and state yourself — query elements, build nodes, attach listeners, hold state in plain JS objects/arrays, re-render by hand. There is no virtual DOM and no reactivity; that's the point (simplicity, zero build step for JS).

Topics, each mapped to where it's used:

| Topic | Use in project | Resource (MDN unless noted) |
|---|---|---|
| **ES modules** (`import`/`export`, `type="module"`) | `app.js` structure | JS Guide → Modules |
| **`fetch` + JSON** | every API call | Fetch API |
| **`ReadableStream` reader** | consume chat SSE (because `EventSource` is GET-only, P14) | Streams API |
| **`EventSource`** | `/api/events` progress stream (P19) | EventSource / Server-sent events |
| **Hash routing** (`location.hash`, `hashchange`) | `#/`, `#/dossier/:id`, `#/prs/:n` (P11/P22) | (no single page — implement from `Location` + events) |
| **`<dialog>`** | New-Dossier modal, confirm-discard (P10/P22) | HTML element `<dialog>` |
| **`<details>`/`<summary>`** | forecasts AI-estimate block (`spec.md §3`) | HTML element `<details>` |
| **CSS custom properties** | theme palette tokens | Using CSS custom properties |
| **`prefers-color-scheme` + `data-theme`** | light/dark toggle (P3) | @media prefers-color-scheme |
| **`localStorage`** | persist theme choice | Window.localStorage |
| **CSS Grid** | `2fr 1fr` split-pane + responsive collapse (P11/P12) | CSS grid layout |
| **`Intl.DateTimeFormat`** | render `last-updated` (P10) | Intl.DateTimeFormat |
| **Pico CSS + override layer** | base styles, then custom CSS *after* Pico; never use Pico defaults as-is | picocss.com/docs |

**Security note to internalize (P11):** you `innerHTML` the rendered Markdown only because Rust already sanitized it with `ammonia`. Server-side sanitization is the invariant that makes client injection safe; if that ever changes, the frontend is an XSS hole.

**Checkpoint:** a single static page that (a) toggles `data-theme` and persists it, (b) `fetch`es a JSON list and renders cards into a CSS grid, (c) opens a `<dialog>`. That's P3+P10 condensed, no build tools.

---

## 6. Learning-unit → prompt mapping

Each unit "unlocks" the prompts you can then implement competently. Use this to know when to stop studying and start building.

| Once you've done… | You can implement… |
|---|---|
| B0, B1, B6(tracing) | P1 |
| C(uv, ruff, mypy, pytest) | P2 |
| D(modules, theme, CSS) | P3 |
| B1, B5, B6(secrecy, dotenvy), B2(Arc/RwLock) | P4, P5 |
| B4, B5, B7, A(git plumbing, GitHub REST) | P6, P7 |
| B0(structs/parsing), B5, A(markdown/frontmatter) | P8 |
| B3, B6(pulldown-cmark, ammonia), B2(spawn_blocking) | P9 |
| D(fetch, grid, dialog, Intl) | P10, P11, P12 |
| B2(Stream), B3(SSE), B4, A(LLM grounding/streaming) | P13 |
| D(ReadableStream) | P14 |
| C(httpx, polars, mock) | P15 |
| C(httpx, orjson), A(GitHub git refs/pulls, structured LLM output) | P16 |
| C(error protocol), A(the JSON contract) | P17 |
| B8, B2(broadcast/SSE) | P18 |
| B7, D(EventSource), all of P7/P9/P18 | P19 |
| B6(similar, diffy), A(3-way merge) | P20 |
| B4, B7 | P21 |
| D(routing, dialog, conflict-marker highlighting) | P22 |
| B4, B5, D(forms) | P23 |
| everything | P24 |

---

## 7. Sequenced path (suggested order)

Interleave so you're never studying in a vacuum; each block ends with you shipping the mapped prompts.

1. **Foundations (Track A §1–2 + Git checkpoint).** Build the mental model; do the git-conflict drill. *No code in the repo yet.*
2. **Rust core (B0 → B1) + checkpoint.** Pre-build the frontmatter parser kata. **Ship P1.**
3. **Python tooling (Track C tour) + checkpoint.** **Ship P2.** Frontend basics (Track D core) + checkpoint. **Ship P3.**
4. **Async + Axum (B2 → B3) + the SSE-spine checkpoint.** **Ship P4–P5.**
5. **HTTP + serde + testing (B4, B5, B7) + GitHub-API reading (Track A git/REST).** **Ship P6–P7.**
6. **Domain model (B0 parsing applied) + render crates (B6).** **Ship P8–P9.**
7. **Frontend build-out (Track D applied).** **Ship P10–P12.**
8. **Streaming chat (B2 Stream, B3 SSE, A LLM patterns; D ReadableStream).** **Ship P13–P14.**
9. **Python agent (Track C applied + agent patterns §4).** **Ship P15–P17.**
10. **Orchestration (B8, broadcast).** **Ship P18–P19.**
11. **PR reviewer (diff/merge crates B6, A 3-way merge; D routing + marker UI).** **Ship P20–P22.**
12. **Config UI + QA.** **Ship P23–P24.**

Rough total before you're productive on the real code: the foundations + Rust-core + async/Axum blocks (steps 1–4) are the gate — invest there. The rest is mostly applying patterns you've already drilled.

---

## 8. Curated resources (canonical entry points)

Links are entry points; navigate to specifics. Confirm crate/library versions yourself — versions move and I'm not asserting them.

**Rust language & async**
- The Rust Programming Language — https://doc.rust-lang.org/book/
- Rustlings (exercises) — https://github.com/rust-lang/rustlings
- Rust by Example — https://doc.rust-lang.org/rust-by-example/
- std library docs — https://doc.rust-lang.org/std/
- Asynchronous Programming in Rust (async-book) — https://rust-lang.github.io/async-book/
- Tokio tutorial — https://tokio.rs/tokio/tutorial

**Backend crates**
- axum — https://docs.rs/axum/latest/axum/ · examples — https://github.com/tokio-rs/axum/tree/main/examples
- tower-http — https://docs.rs/tower-http/latest/tower_http/
- reqwest — https://docs.rs/reqwest/latest/reqwest/
- serde — https://serde.rs/
- thiserror — https://docs.rs/thiserror/latest/thiserror/ · anyhow — https://docs.rs/anyhow/latest/anyhow/
- tracing — https://docs.rs/tracing/latest/tracing/
- secrecy — https://docs.rs/secrecy/latest/secrecy/ · dotenvy — https://docs.rs/dotenvy/latest/dotenvy/
- pulldown-cmark — https://docs.rs/pulldown-cmark/latest/pulldown_cmark/ · ammonia — https://docs.rs/ammonia/latest/ammonia/
- similar — https://docs.rs/similar/latest/similar/ · diffy — https://docs.rs/diffy/latest/diffy/
- wiremock — https://docs.rs/wiremock/latest/wiremock/

**Git & GitHub API**
- Pro Git (free book) — https://git-scm.com/book/en/v2  (read "Git Branching" and "Git Internals")
- GitHub REST overview — https://docs.github.com/en/rest
- Repo contents — https://docs.github.com/en/rest/repos/contents
- Git refs — https://docs.github.com/en/rest/git/refs
- Pull requests — https://docs.github.com/en/rest/pulls
- Personal access tokens — https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens

**Web platform (frontend)**
- Fetch API — https://developer.mozilla.org/en-US/docs/Web/API/Fetch_API
- Streams API (ReadableStream) — https://developer.mozilla.org/en-US/docs/Web/API/Streams_API
- Server-sent events — https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events
- EventSource — https://developer.mozilla.org/en-US/docs/Web/API/EventSource
- JS modules — https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Modules
- `<dialog>` — https://developer.mozilla.org/en-US/docs/Web/HTML/Element/dialog
- `<details>` — https://developer.mozilla.org/en-US/docs/Web/HTML/Element/details
- CSS custom properties — https://developer.mozilla.org/en-US/docs/Web/CSS/Using_CSS_custom_properties
- CSS Grid — https://developer.mozilla.org/en-US/docs/Web/CSS/CSS_grid_layout
- `prefers-color-scheme` — https://developer.mozilla.org/en-US/docs/Web/CSS/@media/prefers-color-scheme
- `Intl.DateTimeFormat` — https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Intl/DateTimeFormat
- Pico CSS docs — https://picocss.com/docs

**Python tooling**
- uv — https://docs.astral.sh/uv/ · ruff — https://docs.astral.sh/ruff/ · mypy — https://mypy.readthedocs.io/en/stable/
- polars — https://docs.pola.rs/ (user guide) · Python API — https://docs.pola.rs/api/python/stable/reference/index.html
- httpx — https://www.python-httpx.org/ · orjson — https://github.com/ijl/orjson
- pytest — https://docs.pytest.org/en/stable/ · unittest.mock — https://docs.python.org/3/library/unittest.mock.html

---

## 9. Depth calibration — what to skip vs. go deep

**Skip / skim (you already have it or don't need it):**
- Python syntax, OOP, packaging beyond `uv` usage.
- Rust lifetimes beyond reading function signatures; macros internals; `unsafe`; trait-object edge cases.
- Frontend build tooling/bundlers (there is none — plain files), CSS frameworks beyond Pico.
- WebSockets (project uses SSE), GraphQL (GitHub REST only here).

**Go deep (genuinely new and load-bearing):**
- **Rust ownership/borrowing** — the single biggest adjustment from Python; most early bugs live here.
- **Async Rust + Tokio runtime mechanics** — `.await`, `spawn_blocking`, `Arc<RwLock>`, `broadcast`, `Stream`.
- **Axum extractors + `State` + tower layers + SSE.**
- **Git plumbing + 3-way merge + GitHub refs/contents/pulls** — the conceptual heart of the GitOps loop and the reviewer.
- **DOM-without-a-framework** — manual rendering, fetch+stream consumption, hash routing.
- **The Rust↔Python subprocess JSON contract + error protocol** — the integration seam where things actually break.

**Highest-risk-to-skip:** the async model and git's merge semantics. Misunderstanding either produces bugs that look like everything-is-on-fire (blocked reactor; corrupted branch state) rather than a clean error.

---

## 10. Validating you're "up to speed"

You're ready to own the build when you can, unprompted:
- Explain why the app stays stateless and what each GitHub call replaces.
- Trace a chat request from keystroke → SSE token → screen, naming every layer.
- Write an Axum handler returning `Result<impl IntoResponse, AppError>` with a `State` extractor and a mocked-HTTP test.
- Read and hand-resolve a 3-way conflict, and explain what `diffy::merge` returns in each case.
- Describe the exact stdout/stderr contract between Rust and the Python agent and why stdout must stay clean.
- Build a static page that streams a `fetch` body into the DOM with no framework.

When all six are reflexive, start at P1 and follow the `todo.md` checklist.
