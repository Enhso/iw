# Intelligence Workbench — Build Blueprint & Prompt Plan

**Goal:** Ship the Intelligence Workbench (`spec.md` v1.0): a stateless Rust/Axum orchestrator + Python/`uv` research agent + vanilla-JS frontend that treat a user's GitHub repository as the single source of truth for analytical dossiers, driven by a GitOps-style "research → PR → review → merge" loop.

**Architecture:** A monorepo with three components. (1) A Rust **Axum** server serves the static frontend, exposes a REST + SSE API, talks to the **GitHub REST API** via `reqwest`, and spawns the Python agent as a subprocess. (2) A Python **agent** (managed by `uv`) performs free-tier research, normalizes results with **Polars**, synthesizes dossier Markdown via an LLM, commits changes to an `ai-update/[topic]` branch, and opens a PR. (3) A **vanilla-JS + customized Pico CSS** frontend with a dashboard, split-pane dossier workspace, chat sidebar, and a visual PR reviewer.

**Tech Stack:** Rust (Axum, Tokio, `reqwest`, `serde`/`serde_json`, `thiserror`, `anyhow`, `tracing`, `secrecy`, `dotenvy`, `pulldown-cmark`, `ammonia`, `similar`, `diffy`, `wiremock` for tests). Python (`uv`, `polars`, `orjson`, `httpx`, `pytest`, `mypy`, `ruff`). Frontend (Pico CSS + custom SCSS, ES6+ vanilla JS, no frameworks/jQuery).

---

## Design decisions made up front (spec left these open)

These are baked into every prompt below. Each is isolated enough to revisit later.

1. **All GitHub access is via the REST API for *both* components — no local git working tree.** The spec mentions "cloning"; we instead read via the Contents/Trees API and write by creating a branch ref + committing files through the Contents API. Rationale: statelessness, no `git` binary dependency in the subprocess, and trivial HTTP mocking in tests. GitHub still computes `mergeable` status server-side, so conflict detection is unaffected.
2. **Interactive chat LLM lives in Rust (SSE streaming); batch research-synthesis LLM lives in Python.** Rationale: chat needs low-latency token streaming, which Axum SSE handles cleanly; the spec's diagram keeps the *research* synthesis in the agent, which is non-interactive and batch-friendly.
3. **Markdown → HTML rendering happens server-side in Rust** (`pulldown-cmark` + `ammonia` sanitization). Rationale: keeps JS minimal and framework-free, avoids a client-side Markdown dependency, and matches the "deep computation in Rust" directive in `CLAUDE.md`.
4. **Conflict view uses a real 3-way merge.** Rust fetches `ancestor` (merge-base), `ours` (main), `theirs` (head) for the conflicting file and runs `diffy::merge`. A clean merge means auto-mergeable; a conflict returns standard `<<<<<<< / ======= / >>>>>>>`-marked text that the editor highlights. The read-only diff uses `similar`.
5. **Credential storage for v1 is a gitignored `.env` file** (PAT held in memory as `secrecy::SecretString`), not an OS keychain. This satisfies `CLAUDE.md`'s "secrets only in `.env`" rule. True at-rest encryption / keychain integration is explicitly deferred. **This is a v1 security simplification — call it out in the README.**
6. **The LLM provider is OpenAI-compatible Chat Completions** by default, configured via `LLM_API_BASE` / `LLM_API_KEY` / `LLM_MODEL` env vars, so it works against most providers (including local). Swapping providers is a config change.

---

## Methodology

The plan was produced by: (a) drafting the architecture above; (b) decomposing into **11 phases** (chunks); (c) decomposing each phase into **right-sized steps**; (d) reviewing for safety (each step compiles/tests independently) and momentum (each step delivers a visible capability); (e) emitting **24 prompts**, each of which ends by **wiring its output into the running app**. No prompt produces code that a later prompt doesn't consume. TDD throughout: tests are written alongside (or before) implementation, every external dependency is mocked, and each prompt ends with a commit.

---

## Blueprint — Phase Overview (the chunking)

| Phase | Theme | Prompts | Exit capability |
|---|---|---|---|
| 0 | Scaffolding & toolchain | P1–P3 | Server runs, agent CLI runs, frontend loads with theme toggle |
| 1 | Config & GitHub auth | P4–P5 | First-launch setup captures + validates + persists PAT/owner/repo |
| 2 | GitHub REST client (Rust) | P6–P7 | Typed, tested read + write wrappers over the GitHub API |
| 3 | Dossier domain model | P8 | Frontmatter parsing + list/read dossiers from the repo |
| 4 | Read API + Dashboard | P9–P10 | Dashboard renders the dossier card deck from live data |
| 5 | Workspace UI | P11–P12 | Split-pane workspace with 7 Markdown tabs + chat shell |
| 6 | Chat | P13–P14 | Streaming chat grounded in the full dossier |
| 7 | Python research agent | P15–P17 | `run_research` fetches → synthesizes → opens a PR |
| 8 | Research orchestration | P18–P19 | "New Dossier" + "Research" spawn the agent and surface the PR |
| 9 | Visual PR reviewer | P20–P22 | Diff/conflict view → resolve → merge/discard loop |
| 10 | Config UI + final QA | P23–P24 | Profile/style editing + full pre-commit verification pass |

---

## Step Decomposition (chunks → steps) with right-sizing rationale

Each step below maps 1:1 to a prompt. "Right-sized" target: one focused unit of work that compiles and passes its tests on its own, but large enough to add a usable slice.

- **P1** Rust/Axum skeleton — *too small alone if split into "add dep" steps; bundled into one runnable server.*
- **P2** Python/`uv` skeleton — *separate component, so its own prompt; not yet called by Rust (wired in P18).* 
- **P3** Frontend static scaffold + theme toggle.
- **P4** Settings + `AppState` + `GET /api/setup/status`.
- **P5** `POST /api/setup` (validate PAT against GitHub) + first-launch UI gate.
- **P6** GitHub **read** client + live `GET /api/health/github` (immediate consumer, avoids orphaning).
- **P7** GitHub **write** client (only the methods Rust itself calls; the agent does its own writes). *Consumers are P18–P21; noted explicitly.*
- **P8** Dossier model: frontmatter parse + `list_dossiers` + `read_dossier_files`. *Consumed by P9.*
- **P9** Read endpoints + server-side Markdown render. *Consumed by P10/P11.*
- **P10** Dashboard card deck + New-Dossier button (modal only, submit deferred to P19).
- **P11** Hash router + split-pane + 7 tabs.
- **P12** Typography/responsive polish + inert chat sidebar markup.
- **P13** Chat context builder + Rust LLM client + SSE endpoint.
- **P14** Chat UI wiring (consume SSE). *Completes the chat slice.*
- **P15** Agent research-fetch + Polars normalization. *Isolated, fully mocked.*
- **P16** Agent GitHub-REST + LLM-synthesis layers. *Isolated, fully mocked.*
- **P17** `run_research` orchestration + CLI + JSON stdout contract. *Fixes the Rust↔Python contract.*
- **P18** Rust spawns the agent (`tokio::process`) + SSE progress. *Consumes P17's contract.*
- **P19** New-Dossier skeleton-on-`main` + trigger research + modal submit. *Consumes P18/P9/P7.*
- **P20** PR read endpoints (list/detail/diff/conflict). *Consumes P7 + `similar`/`diffy`.*
- **P21** PR mutation endpoints (resolve/merge/discard). *Consumes P7.*
- **P22** PR reviewer UI (diff + conflict editor + footer). *Consumes P20/P21.*
- **P23** Profile/style read+write endpoints + settings panel. *Feeds chat context from P13.*
- **P24** Final integration + pre-commit checklist + README.

Reviewed: no step introduces a >1-layer jump (e.g. UI never appears before its endpoint; an endpoint never appears before the client method it calls). The two "isolated library" prompts (P6/P7, P15/P16) are each consumed by the immediately following endpoint/orchestration prompt, so nothing dangles.

---

# The Prompts

> Feed these to the implementing developer **in order**. Each assumes the repository state produced by all prior prompts. Each references the governing rules in `CLAUDE.md` and ends with tests + a commit. Do not skip ahead; later prompts depend on the exact file paths, type names, and JSON shapes introduced earlier.

---

## Phase 0 — Scaffolding & Toolchain

### Prompt 1 — Rust/Axum server skeleton

```text
Create the monorepo root and the Rust backend for the Intelligence Workbench. Follow CLAUDE.md (Rust Ecosystem, Error Handling, Tools) exactly.

Repo layout to create:
    /                      (git root)
      .gitignore
      README.md            (one paragraph: what this is; "WIP")
      rust-backend/        (cargo project, name: workbench)

.gitignore MUST include at least: /target, .env, .venv/, **/__pycache__/, web/pkg/, test-output/

In rust-backend/ run `cargo init --name workbench` and add dependencies (pin current versions; do not invent versions):
    axum, tokio (features: ["full"]), tower, tower-http (features: ["fs","trace","timeout","compression-full"]),
    serde (features: ["derive"]), serde_json, thiserror, anyhow, tracing, tracing-subscriber (features: ["env-filter"])

Implement:
- src/errors.rs: the `AppError` enum from spec.md §6.1 verbatim (GitHubError(#[from] reqwest::Error) — add reqwest as a dep now, features ["json"]), IoError, AgentError(String)), deriving Error/Debug via thiserror, with the `impl IntoResponse`. Add a unit test asserting AgentError maps to 422.
- src/state.rs: an `AppState` struct (for now just `{ }`, derive Clone) — placeholder to grow later.
- src/main.rs:
    * initialize tracing with `tracing_subscriber` + EnvFilter (default `info`),
    * build an axum Router with `GET /health` -> returns 200 + JSON `{"status":"ok"}`,
    * apply a `tower_http::trace::TraceLayer` and a request timeout layer,
    * bind 127.0.0.1:8787 via a tokio TcpListener and serve.

Rules: handlers are `async` and return `Result<impl IntoResponse, AppError>`. No `.unwrap()` in non-test paths (use `.expect("invariant: ...")` only for true invariants). Errors logged via `tracing::error!`. rustfmt + clippy clean (`-D warnings`). Doc comments on public items. 100-char lines.

Wiring/verify: `cargo run` starts the server; `curl localhost:8787/health` returns the JSON. `cargo test` passes. `cargo clippy -- -D warnings` clean.

Commit: `feat(backend): axum skeleton with health route, tracing, and AppError`
```

### Prompt 2 — Python/`uv` agent skeleton

```text
Add the Python research agent as a uv-managed project. Follow CLAUDE.md (Python Ecosystem, Tools, Testing) exactly.

Create at repo root:
    python/
      pyproject.toml
      src/workbench_agent/__init__.py     (defines __version__ = "0.1.0")
      src/workbench_agent/__main__.py
      tests/test_smoke.py

Use `uv` to create the project and a `.venv` (confirm .venv is gitignored — it already is from P1). Configure pyproject for:
- project name `workbench-agent`, requires-python >=3.11,
- runtime deps: polars, orjson, httpx,
- dev deps (uv dev group): pytest, mypy, ruff,
- ruff config: line-length 88, PEP 8; mypy config: strict-ish (disallow Any where avoidable, warn unused ignores).
Also install ipykernel + ipywidgets into the venv only (NOT as project deps), per CLAUDE.md.

Implement __main__.py as a minimal argparse CLI stub:
    `python -m workbench_agent --version`  -> prints __version__ and exits 0.
Set up module logging now (per spec §6.2): a module logger named "workbench_agent", configured to logging.INFO; use logger.error / logger.info — never print.

tests/test_smoke.py: a pytest test asserting `workbench_agent.__version__` is a non-empty str. Save it as a discrete file (CLAUDE.md). Do not delete generated test files.

Type hints on every function signature; no `Any`. Run `mypy` and `ruff check` clean.

Wiring/verify: `uv run python -m workbench_agent --version` prints `0.1.0`. `uv run pytest` passes. `uv run ruff check` and `uv run mypy src` clean. (This component is standalone for now; Rust invokes it in Prompt 18.)

Commit: `feat(agent): uv project skeleton with CLI stub, logging, and smoke test`
```

### Prompt 3 — Frontend static scaffold + theme toggle

```text
Add the vanilla-JS frontend and have Axum serve it. Follow CLAUDE.md (Web & Front-End) and spec.md §5: Pico CSS + custom stylesheet, vanilla ES6+ JS, NO React/Svelte/Vue/jQuery, adaptive light/dark with a toggle, modern unique typography.

Create:
    web/
      index.html
      app.js
      styles.css        (hand-written; you may keep it as plain CSS — SCSS is optional. Do NOT use Pico defaults as-is.)
    (vendor Pico CSS by downloading pico.min.css into web/vendor/ — do not hotlink a CDN at runtime.)

index.html:
- semantic skeleton: <header> with app title (left) and a theme-toggle button (right), a <main id="app"> root.
- link web/vendor/pico.min.css then web/styles.css (override layer after Pico).
- choose two Google Fonts (a modern sans for headers, a serif for body) and self-host or @import them; document the choice in a CSS comment.

styles.css: define CSS custom properties for the palette and set `:root[data-theme="light"]` / `[data-theme="dark"]` overrides. Header/body font-family wired to the chosen fonts. Keep it modern and distinct from Pico defaults.

app.js (ES module, no frameworks):
- theme module: read saved theme from localStorage (key `workbench-theme`), else match `prefers-color-scheme`; apply to `document.documentElement[data-theme]`; toggle button flips + persists.
- export an `init()` that runs on DOMContentLoaded and renders a temporary "Workbench loading…" placeholder into #app.

Wire into Axum (rust-backend):
- serve the `web/` directory at `/` using `tower_http::services::ServeDir` with a fallback to index.html. Keep `/health` and (future) `/api/*` routes above the static fallback so they take precedence.

Wiring/verify: `cargo run`, open http://localhost:8787 — page loads, fonts applied, theme toggle works and persists across reload. No console errors. grep the JS for `React|jQuery|\$\(` — must be empty.

Commit: `feat(frontend): static scaffold with custom Pico theme and dark/light toggle`
```

---

## Phase 1 — Configuration & GitHub Auth

### Prompt 4 — Settings, AppState, and setup status

```text
Add typed configuration and shared state to the Rust backend. Follow CLAUDE.md (Security, Struct design) — secrets only via env, PAT wrapped in secrecy.

Add deps: dotenvy, secrecy (features ["serde"]).

Implement src/config.rs:
- a `WorkbenchConfig` struct holding: `owner: Option<String>`, `repo: Option<String>`, `github_token: Option<secrecy::SecretString>`. Private fields + accessor methods. Derive nothing that would leak the secret (do NOT derive Debug that prints the token; if you derive Debug, redact the token field).
- `load() -> WorkbenchConfig`: call `dotenvy::dotenv().ok()`, then read env vars `GITHUB_OWNER`, `GITHUB_REPO`, `GITHUB_TOKEN` into the struct (each Optional).
- `is_configured(&self) -> bool`: true iff all three are present.

Update src/state.rs: `AppState { config: Arc<RwLock<WorkbenchConfig>>, http: reqwest::Client }`, derive Clone. The RwLock allows runtime updates after setup (Prompt 5). Build the reqwest::Client once with a sane default User-Agent ("intelligence-workbench") and timeout.

Update src/main.rs: construct `AppState` at startup (`WorkbenchConfig::load()`), `.with_state(state)` on the router.

Add endpoint `GET /api/setup/status` -> `Result<Json<SetupStatus>, AppError>` where `SetupStatus { configured: bool }`, reading `state.config.read()...is_configured()`. Offload nothing here (cheap). Never include the token in any response.

Tests: unit test that `is_configured` is false when any field is None and true when all set.

Wiring/verify: `cargo run`; `curl localhost:8787/api/setup/status` returns `{"configured":false}` (no .env yet). clippy/-D warnings clean. Confirm the token type never appears in logs.

Commit: `feat(backend): config loading via dotenvy/secrecy, AppState, setup-status endpoint`
```

### Prompt 5 — PAT validation + first-launch setup flow

```text
Implement first-launch credential capture and validation (spec.md §4.1). Follow CLAUDE.md (Security: never log token/URLs with secrets).

Backend — add `POST /api/setup`:
- request body `SetupRequest { owner: String, repo: String, token: String }` (serde).
- validate by calling the GitHub API `GET https://api.github.com/user` with header `Authorization: Bearer {token}`, `User-Agent`, and `Accept: application/vnd.github+json`, using `state.http`. On 200, extract `login`. On non-2xx, return `AppError::AgentError("invalid GitHub credentials")` mapped to 422 (or BAD_GATEWAY for upstream/network). NEVER log the token.
- on success: persist to a gitignored `.env` at repo root (write/replace `GITHUB_OWNER`, `GITHUB_REPO`, `GITHUB_TOKEN` lines; preserve other lines), AND update the in-memory `state.config` (acquire write lock, set fields, wrap token in SecretString).
- response: `SetupResponse { login: String }`.

Add a small `src/github/mod.rs` placeholder module now with one function `pub async fn get_authenticated_login(http: &reqwest::Client, token: &SecretString) -> Result<String, AppError>` implementing the validation call above, and call it from the handler (this is the seed of the GitHub client expanded in Prompt 6).

Tests: with `wiremock`, stand up a mock GitHub server, point the function at it (make the base URL injectable — add a `GITHUB_API_BASE` env override defaulting to https://api.github.com), assert 200→login parsed and 401→error. Save as discrete test module.

Frontend (web/app.js + index.html):
- on init, fetch `/api/setup/status`. If `configured:false`, render a setup form (owner, repo, PAT — PAT field type=password) into #app instead of the app shell.
- on submit, POST to `/api/setup`; on success show a brief "Connected as {login}" message then render an empty app-shell placeholder (the dashboard arrives in Prompt 10); on failure show the error inline. Use fetch + native form handling — no frameworks.

Wiring/verify: start fresh (no .env). Load page → setup form. Submit a valid PAT (or wiremock in tests) → .env written, status now `{"configured":true}`, reload skips the form. Confirm `.env` is gitignored and the token never prints to the server log.

Commit: `feat: first-launch GitHub PAT setup with validation and secure persistence`
```

---

## Phase 2 — GitHub REST Client (Rust)

### Prompt 6 — GitHub read client + live health check

```text
Build the typed GitHub READ client in rust-backend. Follow CLAUDE.md (Rust API guidelines, newtypes, errors, tests with mocks). Keep using `reqwest` per spec.md §4.1 (do NOT pull in octocrab; if you want it later it's an isolated swap).

In src/github/ create:
- newtypes (newtype pattern, CLAUDE.md): `Owner(String)`, `Repo(String)`, `Branch(String)`, `Sha(String)`, each with `as_str()`; derive Debug/Clone/PartialEq.
- a `GitHubClient` struct holding `{ http: reqwest::Client, base: String, token: SecretString }`, constructed from `&AppState` (base from `GITHUB_API_BASE` env, default https://api.github.com). Provide a private helper that builds an authorized `RequestBuilder` (sets Authorization/Accept/User-Agent) to keep DRY.
- serde models for the responses you parse (only the fields you use): `RepoTreeEntry { path, r#type, sha }`, `TreeResponse { tree: Vec<RepoTreeEntry> }`, `ContentResponse { content: String, encoding: String, sha: Sha, path: String }`.

Methods (all `async`, return `Result<_, AppError>`):
- `get_authenticated_login(&self) -> Result<String, AppError>` (move the Prompt-5 logic here; have the handler call this).
- `get_tree(&self, owner, repo, branch_or_sha, recursive: bool) -> Result<TreeResponse, AppError>` (GET /repos/{o}/{r}/git/trees/{ref}?recursive=1).
- `get_file_content(&self, owner, repo, path: &str, r#ref: Option<&str>) -> Result<FileContent, AppError>` where `FileContent { text: String, sha: Sha }`; GET /repos/{o}/{r}/contents/{path}{?ref}; base64-decode the `content` field (add `base64` dep) and return UTF-8 text. Offload base64 decode of large files to `spawn_blocking` if needed.

Tests (wiremock): mock the trees and contents endpoints with canned fixtures (a small base64 payload), assert parsing/decoding. One test per method. Discrete `#[cfg(test)]` module.

Wiring (live consumer so nothing is orphaned): add `GET /api/health/github` -> constructs a `GitHubClient` from state (error if not configured) and returns `Json({ "login": <get_authenticated_login> })`. Replace the inline Prompt-5 call with `GitHubClient::get_authenticated_login`.

Verify: `cargo test` green; with a real configured .env, `curl /api/health/github` returns your login. clippy -D warnings clean.

Commit: `feat(backend): typed GitHub read client (trees, contents) with mocked tests`
```

### Prompt 7 — GitHub write client (only what Rust calls)

```text
Extend `GitHubClient` with the WRITE methods the Rust backend itself needs. (The Python agent performs its own branch/commit/PR writes — Prompt 16 — so do NOT implement create-branch or create-PR here.) Follow CLAUDE.md error rules; mock everything in tests.

Add serde models for the fields used: `PullRequest { number: u64, title: String, html_url: String, state: String, head: PrRef, base: PrRef, mergeable: Option<bool>, merge_commit_sha: Option<Sha> }` with `PrRef { r#ref: String, sha: Sha }`; `PullFile { filename: String, status: String }`; `ContentWriteResponse { content: ContentMeta, commit: CommitMeta }` (capture the new file `sha` and commit `sha`).

Methods (async, `Result<_, AppError>`):
- `create_or_update_file(&self, owner, repo, path, content_utf8: &str, message: &str, branch: &str, sha: Option<&Sha>) -> Result<Sha, AppError>` — PUT /repos/{o}/{r}/contents/{path}; body has base64 `content`, `message`, `branch`, and `sha` when updating an existing file. Returns the new blob sha.
- `list_pull_requests(&self, owner, repo, state: &str) -> Result<Vec<PullRequest>, AppError>` — GET /pulls?state=open&per_page=100.
- `get_pull_request(&self, owner, repo, number: u64) -> Result<PullRequest, AppError>`.
- `get_pull_request_files(&self, owner, repo, number) -> Result<Vec<PullFile>, AppError>`.
- `merge_pull_request(&self, owner, repo, number, commit_title: Option<&str>) -> Result<(), AppError>` — PUT /pulls/{n}/merge.
- `close_pull_request(&self, owner, repo, number) -> Result<(), AppError>` — PATCH /pulls/{n} with `{"state":"closed"}`.
- `delete_ref(&self, owner, repo, ref_name: &str) -> Result<(), AppError>` — DELETE /git/refs/{ref} (e.g. `heads/ai-update/topic`).

Tests (wiremock): one per method with canned responses incl. a PR with `mergeable: false`. Verify request bodies for the PUT/PATCH/merge calls (correct base64, branch, sha).

Wiring: these methods are exercised by tests now and consumed by Prompt 18 (research), Prompt 19 (new-dossier skeleton), and Prompts 20–21 (PR reviewer). They live on the already-wired `GitHubClient`, so there is no standalone/orphaned code.

Verify: `cargo test` green; clippy -D warnings clean; mypy n/a.

Commit: `feat(backend): GitHub write client (contents PUT, PR list/get/files/merge/close, ref delete)`
```

---

## Phase 3 — Dossier Domain Model

### Prompt 8 — Frontmatter parsing + dossier listing/reading

```text
Model dossiers and read them from the repo. Follow spec.md §3 (schemas) and the unit-test shape in §7.1, plus CLAUDE.md (types, tests).

Add a YAML dep (a maintained crate, e.g. `serde_yml`; confirm current name/version). Add `chrono` (features ["serde"]) for the `last-updated` timestamp.

In src/dossier/ create:
- `DossierMeta` (serde, from YAML frontmatter): fields matching overview.md frontmatter — `last_updated: Option<DateTime<Utc>>` (rename "last-updated"), `confidence: Option<String>`, `status: Option<String>`, `category: Option<String>`, `topics: Vec<String>` (default empty). Use serde rename attributes for hyphenated keys.
- `fn parse_dossier_frontmatter(markdown: &str) -> Result<DossierMeta, AppError>` — split the leading `---\n ... \n---` block, parse the YAML body with serde_yml; if no frontmatter, return a default `DossierMeta`. This is the function named in spec.md §7.1 — keep that exact name.
- `DossierSummary { id: String, title: String, meta: DossierMeta }` (serde Serialize) for the dashboard.
- The canonical tab list as a constant: `const DOSSIER_FILES: [&str; 7] = ["overview","causal_models","timeline","entities","evidence_assessment","sources","forecasts"]` (file stems; on disk they are `*.md`). Provide a helper mapping a UI tab id to its filename.

Functions using `GitHubClient` (async):
- `list_dossiers(client, owner, repo) -> Result<Vec<DossierSummary>, AppError>`: get the recursive tree of `dossiers/`, find each immediate subdirectory `id`, fetch `dossiers/{id}/overview.md`, parse frontmatter, derive `title` from the first `# ` heading (fallback to id). Fetch overviews concurrently (`futures::future::try_join_all`; add `futures`).
- `read_dossier_files(client, owner, repo, id) -> Result<BTreeMap<String, String>, AppError>`: read all 7 files for a dossier (missing file -> empty string, not an error), keyed by stem.

Tests:
- the spec.md §7.1 test: `parse_dossier_frontmatter("---\nstatus: Active\nconfidence: Medium\n---\n# Title")` yields status "Active", confidence "Medium".
- a no-frontmatter input yields defaults.
- (wiremock) `list_dossiers` against a mocked tree+contents returns the expected summaries.

Wiring: consumed by the read API in Prompt 9 (next). No live endpoint yet.

Verify: `cargo test` green; clippy -D warnings clean.

Commit: `feat(backend): dossier model, frontmatter parsing, list/read from repo`
```

---

## Phase 4 — Read API + Dashboard

### Prompt 9 — Read endpoints + server-side Markdown rendering

```text
Expose dossier read endpoints and render Markdown to sanitized HTML server-side (design decision #3). Follow CLAUDE.md (Axum handler signatures).

Add deps: pulldown-cmark, ammonia.

In src/render.rs: `fn render_markdown(md: &str) -> String` — strip leading YAML frontmatter (reuse the split logic from Prompt 8; refactor that split into a shared helper to stay DRY), convert with pulldown-cmark (enable tables, footnotes, strikethrough), then sanitize the HTML with ammonia (allow standard formatting + tables + <details>/<summary>, which spec.md §3 forecasts.md uses). Unit-test that a simple doc renders and that a `<script>` is stripped.

Endpoints (all return `Result<_, AppError>`; construct `GitHubClient` from state; 503/AppError if not configured):
- `GET /api/dossiers` -> `Json<Vec<DossierSummary>>` via `list_dossiers`.
- `GET /api/dossiers/:id` -> `Json` of `{ id, meta, files: { stem -> raw_markdown } }` via `read_dossier_files` + parse of overview frontmatter.
- `GET /api/dossiers/:id/render/:tab` -> `Html<String>`: map `:tab` to a filename, read that one file, `render_markdown`. Validate `:tab` is in the known set; unknown -> 404.

Heavy Markdown rendering of large files: wrap `render_markdown` in `tokio::task::spawn_blocking` per CLAUDE.md (Axum: offload CPU-bound work).

Tests (wiremock for GitHub + direct unit tests for render): assert `/api/dossiers` shape and that an unknown tab 404s.

Wiring: these endpoints are consumed by the dashboard (Prompt 10) and workspace (Prompt 11). Add them to the router above the static fallback.

Verify: with a real repo containing one dossier, `curl /api/dossiers` returns summaries and `/api/dossiers/<id>/render/overview` returns HTML. `cargo test` green.

Commit: `feat(backend): dossier read endpoints + sanitized server-side markdown rendering`
```

### Prompt 10 — Dashboard UI

```text
Build the Dashboard (spec.md §5.1): responsive card deck of dossiers. Vanilla JS + custom Pico, no frameworks.

In web/app.js, add a `dashboard` view module:
- `async function renderDashboard(root)`: fetch `/api/dossiers`; render a responsive grid (CSS grid card deck — style in web/styles.css using your palette variables) where each card shows: title, category, status (as a colored pill), confidence, and last-updated (formatted from the ISO timestamp with native Intl.DateTimeFormat). Handle three states explicitly: loading (skeleton/placeholder), empty ("No dossiers yet — create one"), error (inline message).
- a prominent "New Dossier" button in the header/toolbar that opens a modal (native <dialog>) with two fields: Title and Initial Question. For now the modal's submit is a no-op stub that closes the dialog and logs the values — the real submit lands in Prompt 19. (Leave a clearly-marked TODO referencing Prompt 19.)

Update `init()` so that when setup status is `configured:true`, it calls `renderDashboard(#app)` (replacing the Prompt-5 placeholder).

Accessibility/HID (CLAUDE.md): cards are keyboard-focusable; the dialog traps focus and closes on Escape.

Wiring/verify: configured app loads straight into the dashboard; cards render from live `/api/dossiers`; empty and error states reachable (e.g. point at an empty repo). "New Dossier" opens/closes the modal. No console errors; no framework usage.

Commit: `feat(frontend): dashboard card deck with new-dossier modal (submit stubbed)`
```

---

## Phase 5 — Workspace UI

### Prompt 11 — Hash router + split-pane workspace + tabs

```text
Add client-side routing and the split-pane dossier workspace (spec.md §5.2). Vanilla JS, no frameworks.

In web/app.js add a tiny hash router:
- routes: `#/` -> dashboard; `#/dossier/:id` -> workspace. Listen for `hashchange`; on load, dispatch the current hash. Clicking a dashboard card navigates to `#/dossier/{id}`.

Workspace view (`async function renderWorkspace(root, id)`):
- layout container with `grid-template-columns: 2fr 1fr;` at desktop (define the grid + breakpoints in styles.css).
- LEFT pane: a tab strip with exactly these tabs mapping to the render endpoint:
  Overview | Causal Models | Timeline | Entities | Evidence | Sources | Forecasts
  (tab ids must match the backend tab→filename map: overview, causal_models, timeline, entities, evidence_assessment, sources, forecasts).
  Selecting a tab fetches `/api/dossiers/{id}/render/{tab}` and injects the returned HTML into the content area. Cache fetched tab HTML in memory for the session to avoid refetching. Default to the Overview tab.
- RIGHT pane: a placeholder container `#chat-sidebar` (the chat UI markup arrives in Prompt 12, wiring in Prompt 14).
- a "back to dashboard" control.

Render the server HTML safely: it is already sanitized by ammonia server-side; inject via innerHTML is acceptable here precisely because of that server-side sanitization — add a code comment stating this invariant.

Wiring/verify: from the dashboard, clicking a card opens `#/dossier/{id}` with the Overview tab rendered; switching tabs loads each file's HTML; back returns to the dashboard. No console errors.

Commit: `feat(frontend): hash router + split-pane workspace with 7 markdown tabs`
```

### Prompt 12 — Typography/responsive polish + chat sidebar shell

```text
Finish the workspace's visual layer and lay down the (inert) chat UI. spec.md §5.2 + §5: modern sans headers, serif body, legible reading column, responsive collapse to a single stacked column on mobile.

styles.css:
- typographic scale for the rendered Markdown content area (headers use the sans font, body/paragraphs use the serif font, comfortable line-length/max-width, code blocks styled, tables styled, <details> styled). Ensure both light and dark themes are legible (contrast).
- responsive: below a tablet breakpoint, the 2fr/1fr grid collapses to a single column (content first, chat below). Verify the timeline/long lists stack cleanly (spec.md §7.3).

Chat sidebar markup (web — into `#chat-sidebar`, built by app.js):
- a scrollable message-history list (`#chat-history`) and a query input row (`#chat-input` textarea + Send button). Render this now but keep it DISABLED/inert with a "Chat connects in the next step" affordance. No network calls yet (wired in Prompt 14).

Wiring/verify: resize the window from desktop to mobile — layout reflows without overflow/clipping; reading column stays legible in both themes; the chat sidebar is visible but inert. Run the spec.md §7.3 manual checklist (theme switch has no layout jank; no React/jQuery; mobile stacks cleanly).

Commit: `feat(frontend): workspace typography, responsive layout, and inert chat sidebar shell`
```

---

## Phase 6 — Chat

### Prompt 13 — Chat context builder + LLM client + SSE endpoint

```text
Implement grounded streaming chat in Rust (design decision #2). spec.md §4.4 context construction; CLAUDE.md (Axum, errors, secrets via env).

Add deps: axum SSE is built-in; add `futures` if not present; add `eventsource-stream` only if you parse upstream SSE (or parse the provider stream manually).

Config: read `LLM_API_BASE` (default an OpenAI-compatible endpoint), `LLM_API_KEY`, `LLM_MODEL` from env via the config module (extend WorkbenchConfig with these optional fields + accessors; treat LLM_API_KEY as a SecretString; never log it).

src/chat.rs:
- `async fn build_context(client, owner, repo, id) -> Result<String, AppError>`: read all 7 dossier files (Prompt 8), plus `.workbench/profile.json` and `.workbench/analyst_style.md` (fetch via get_file_content; missing -> omit). Assemble the exact payload shape from spec.md §4.4:
      System preamble ("You are an elite intelligence partner...") then
      `=== FILE: <name> ===\n<content>\n` for each of the 7 files, then
      `=== ANALYST PROFILE ===\n<profile.json>` and `=== ANALYST STYLE ===\n<analyst_style.md>`, then the Instructions line ("Do not repeat facts already explicit... expose hidden assumptions, evaluate alternative hypotheses.").
- src/llm.rs: `async fn stream_chat(http, base, key, model, system: &str, user: &str) -> impl Stream<Item=Result<String,AppError>>`: POST to `{base}/chat/completions` with `stream:true`, parse the SSE token deltas, yield text chunks. Map upstream/network failures to AppError. Offload nothing (it's I/O bound).

Endpoint `POST /api/dossiers/:id/chat`:
- body `ChatRequest { message: String, history: Vec<ChatTurn> }` (ChatTurn { role, content }).
- build the system context (build_context) + compose messages (system, prior history, new user message), call stream_chat, and relay as `axum::response::sse::Sse` of `data:` events (one per token chunk), ending with a sentinel `event: done`.

Tests (wiremock): mock the LLM `chat/completions` SSE response with a couple of chunks; assert `stream_chat` yields the expected concatenation. Mock GitHub for `build_context` and assert the payload contains the file delimiters and profile section.

Wiring: consumed by the chat UI in Prompt 14. Add the route above the static fallback.

Verify: `cargo test` green. With real LLM creds + a real dossier, `curl -N` the SSE endpoint and observe streamed tokens. Confirm neither the PAT nor the LLM key is logged.

Commit: `feat(backend): grounded chat context builder + streaming LLM SSE endpoint`
```

### Prompt 14 — Chat UI wiring

```text
Wire the chat sidebar to the SSE endpoint, completing the chat slice. Vanilla JS, native EventSource/fetch streaming — no frameworks.

In web/app.js chat module:
- maintain an in-memory `history` array of {role, content} for the open dossier (reset on dossier change).
- on Send: append the user turn to `#chat-history` (rendered), then POST to `/api/dossiers/{id}/chat` with `{message, history}`. Because EventSource is GET-only, consume the streamed response via `fetch` + a `ReadableStream` reader, parsing `data:` lines incrementally; append tokens to a live-updating assistant message bubble until the `done` sentinel. On completion, push the full assistant turn into `history`.
- handle errors (show an inline error bubble), disable Send while a response streams, re-enable after.
- enable the previously-inert input/Send from Prompt 12.

Render assistant Markdown: request it as plain text and display as-is for v1 (or run it through the existing `/api/.../render` path is overkill) — keep v1 simple: render assistant text with minimal formatting (preserve line breaks). Do NOT add a client-side Markdown library.

Wiring/verify: open a dossier, ask a question grounded in its files, watch tokens stream into the sidebar; multi-turn history is retained within the session and sent on each request; switching dossiers clears history. No framework usage; no console errors.

Commit: `feat(frontend): streaming chat sidebar wired to the dossier SSE endpoint`
```

---

## Phase 7 — Python Research Agent

### Prompt 15 — Research fetch layer + Polars normalization

```text
Build the agent's research-fetch layer. spec.md §4.2 (free-tier APIs) + CLAUDE.md (Python: httpx, polars, orjson, type hints, pytest with mocked network, logger.error).

In python/src/workbench_agent/ create:
- providers.py: a small provider interface. Implement two NO-KEY providers first so the pipeline runs without secrets:
    * `wikipedia_search(query: str, limit: int) -> list[dict]` (REST summary/search API),
    * `arxiv_search(query: str, limit: int) -> list[dict]` (arXiv Atom API; parse with stdlib xml).
  And two OPTIONAL keyed providers, enabled only if their env key is set (read at call time, never hardcode):
    * `tavily_search(query, limit)` (uses TAVILY_API_KEY) and `semantic_scholar_search(query, limit)` (S2 API; key optional).
  Each returns a list of dicts with a common shape: {"title","url","snippet","source","published"}.
- research.py: a `@dataclass(frozen=True) ResearchResult` (or just return a Polars frame) and `def fetch_research(query: str, limit_per_source: int = 5) -> pl.DataFrame`:
    * call all ENABLED providers (skip keyed ones whose env key is absent), collect rows,
    * build a single Polars DataFrame, normalize columns, drop exact-duplicate URLs, sort by source then title. Per CLAUDE.md: when printing the frame in tests/notebooks do not also print row count/schema; never ingest >10 rows at a time in any analysis helper.

All HTTP via httpx with timeouts and try/except around each provider call; on a provider failure, logger.error and continue with the other providers (one bad source must not abort the run).

Tests (python/tests/test_research.py, discrete file): patch httpx calls (unittest.mock) with canned JSON/XML fixtures; assert `fetch_research` returns a frame with the normalized columns and that duplicate URLs are removed and a failing provider is skipped. Mock all network — no real requests. Type-annotate everything; mypy + ruff clean.

Wiring: consumed by `run_research` in Prompt 17. Standalone + tested for now.

Verify: `uv run pytest tests/test_research.py -v` passes; `uv run mypy src` + `uv run ruff check` clean.

Commit: `feat(agent): multi-source research fetch with Polars normalization and mocked tests`
```

### Prompt 16 — Agent GitHub-REST + LLM-synthesis layers

```text
Add the agent's write path (GitHub REST) and its LLM synthesis. Design decision #1 (REST, no git CLI) + #6 (OpenAI-compatible). CLAUDE.md (httpx, orjson, type hints, mocked tests, logger.error, secrets via env).

python/src/workbench_agent/github.py — a `GitHubRepo` class `{ owner, repo, token (from env GITHUB_TOKEN), base (env GITHUB_API_BASE default api.github.com) }` with methods (httpx, all typed, raising a custom `GitHubError`):
- `default_branch_sha() -> str` (GET repo → default_branch; GET ref heads/{default} → sha).
- `create_branch(new_branch: str, from_sha: str) -> None` (POST git/refs `refs/heads/{new}` → {sha}). Idempotency: if it already exists, update it or log+continue per chosen policy.
- `get_file(path: str, ref: str) -> tuple[str, str | None]` returns (utf8_text, blob_sha) — sha None if file absent.
- `put_file(path, content_utf8, message, branch, sha: str | None) -> None` (PUT contents with base64 content + branch + optional sha).
- `create_pull_request(title, head_branch, base_branch, body) -> dict` returns {"number","html_url"}.
Serialize/deserialize JSON with orjson. Never log the token or any URL containing it (CLAUDE.md).

python/src/workbench_agent/synthesis.py:
- `def synthesize(existing_files: dict[str,str], research: pl.DataFrame, profile: str, query: str, model: str) -> dict[str,str]`:
    build a prompt instructing the LLM to update the relevant dossier Markdown files (overview/timeline/entities/causal_models/evidence_assessment/sources/forecasts) given the research rows, preserving frontmatter/structure from spec.md §3, and to return a JSON object mapping changed filename → new full content. Call `{LLM_API_BASE}/chat/completions` (key from LLM_API_KEY env) via httpx; parse the JSON object out of the response (strip code fences; orjson.loads). Return only changed files. Type everything; no Any beyond the unavoidable JSON boundary (parse into typed dict).

Tests (python/tests/test_github.py and test_synthesis.py, discrete files): mock all httpx calls. For github.py assert correct request bodies (base64 content, branch, sha presence/absence on create vs update) and parsing. For synthesis.py mock the LLM to return a fenced JSON blob and assert it parses into the file map and ignores unchanged files. No real network.

Wiring: consumed by `run_research` in Prompt 17.

Verify: `uv run pytest -v` passes; mypy + ruff clean.

Commit: `feat(agent): GitHub REST write layer + LLM dossier synthesis with mocked tests`
```

### Prompt 17 — `run_research` orchestration + CLI + JSON contract

```text
Tie the agent together and freeze the Rust↔Python contract. spec.md §4.2 code pattern, §6.2 logging. CLAUDE.md (orjson stdout, logger.error, SystemExit(1) on failure, type hints).

python/src/workbench_agent/agent.py — implement the spec.md §4.2 `run_research(topic_id: str, query: str) -> dict`:
  1. `fetch_research(query)` (Prompt 15).
  2. read existing dossier files via GitHubRepo.get_file for the 7 paths under dossiers/{topic_id}/ from the default branch (missing -> "").
  3. read .workbench/profile.json (default "{}" if absent) for context.
  4. `synthesize(...)` -> changed files (Prompt 16).
  5. branch name `ai-update/{topic_id}`; `default_branch_sha()` -> `create_branch(branch, sha)`.
  6. for each changed file: `get_file(path, ref=branch)` to get its current sha on the branch, then `put_file(path, content, "research: update {topic_id}", branch, sha)`.
  7. `create_pull_request(title, branch, default_branch, body)`.
  8. return `{"topic_id", "branch", "pr_number", "pr_url", "changed_files": [...]}`.

__main__.py — replace the stub with argparse: required `--owner`, `--repo`, `--topic-id`, `--query`; optional `--dry-run` (run steps 1–4, skip 5–7, return changed file names + a `"dry_run": true` flag). PAT/LLM creds come from env (GITHUB_TOKEN, LLM_API_KEY, LLM_API_BASE, LLM_MODEL). On success, write the result dict to STDOUT as orjson bytes (single line) — this is the contract Rust parses. Wrap the whole run in try/except per spec.md §6.2: on any exception `logger.error(..., exc_info=True)` to STDERR and `raise SystemExit(1)`; STDOUT stays clean (no partial JSON).

Tests (python/tests/test_agent.py — extend the spec.md §7.2 skeleton, discrete file): mock fetch + github + synthesis; assert `run_research` performs create_branch → put_file(s) → create_pull_request in order and returns the contract dict; assert `--dry-run` skips writes; assert a raised provider error leads to SystemExit(1) and clean stdout. No real network.

Wiring: this STDOUT JSON contract is consumed by Rust in Prompt 18.

Verify: `uv run python -m workbench_agent --owner X --repo Y --topic-id t --query "q" --dry-run` prints a JSON line; `uv run pytest -v` passes; mypy + ruff clean.

Commit: `feat(agent): run_research orchestration, CLI, and JSON stdout contract`
```

---

## Phase 8 — Research Orchestration (Rust ↔ Python)

### Prompt 18 — Rust spawns the agent + SSE progress

```text
Have Axum spawn the Python agent and surface its PR result. spec.md §4.2 (uv run subprocess). CLAUDE.md (Axum spawn_blocking guidance — but use tokio::process for the subprocess; offload CPU-bound parsing only).

src/agent.rs:
- `async fn run_agent(state, owner, repo, topic_id, query, dry_run: bool) -> Result<AgentResult, AppError>`:
    build `tokio::process::Command` for `uv run python -m workbench_agent --owner .. --repo .. --topic-id .. --query .. [--dry-run]`, with `current_dir(python/)`. Pass the needed secrets via the child's ENV (GITHUB_TOKEN from config, LLM_* from config) — do NOT pass them as CLI args (avoids leaking in process listings). Capture stdout + stderr. On non-zero exit, log stderr via tracing::error! (but scrub: stderr should not contain secrets — the agent already avoids that) and return `AppError::AgentError(<short message>)`. On success, parse the single-line stdout JSON into `AgentResult { topic_id, branch, pr_number, pr_url, changed_files }` (serde). If JSON parse fails, AgentError.
- define `AgentResult` (serde Deserialize matching the Prompt-17 contract).

Endpoint `POST /api/dossiers/:id/research`:
- body `ResearchRequest { query: String }`.
- call `run_agent(..., dry_run=false)`; return `Json<AgentResult>` (incl. pr_url).

SSE progress (lightweight): add `GET /api/events` returning an SSE stream backed by a `tokio::sync::broadcast` channel held in AppState; when a research run completes, publish a `{ "type":"pr_ready", "topic_id":.., "pr_url":.. }` event so the frontend can toast it. (Extend AppState with the broadcast Sender.)

Tests: unit-test the stdout-JSON parsing into AgentResult (feed a canned JSON line). For the subprocess itself, gate a real-invocation integration test behind an env flag / `#[ignore]` so CI stays hermetic (CLAUDE.md: mock external deps; the subprocess is external).

Wiring: consumed by the New-Dossier flow (Prompt 19) and can also be triggered standalone. Add routes above the static fallback; subscribe the frontend to `/api/events` in Prompt 19.

Verify: with everything configured, `POST /api/dossiers/<id>/research {"query":"..."}` runs the agent and returns a real PR URL; the `/api/events` stream emits `pr_ready`. `cargo test` green.

Commit: `feat(backend): spawn python agent via uv, parse PR result, SSE progress channel`
```

### Prompt 19 — New-Dossier flow (skeleton + trigger research)

```text
Make "New Dossier" fully functional end-to-end. spec.md §5.1 + §3 (repo layout) + §4.2. Reuses Prompt 7 (create_or_update_file), Prompt 9 (read), Prompt 18 (research).

Backend `POST /api/dossiers`:
- body `NewDossierRequest { title: String, question: String }`.
- derive a slug `id` from the title (lowercase, hyphenate, strip non-alphanumerics; ensure uniqueness against existing dossier ids — append -2, -3 on collision).
- if `.workbench/profile.json` or `.workbench/analyst_style.md` are absent on main, create them with sensible defaults (profile.json default per spec.md §3 example shape; analyst_style.md a short default). Commit via create_or_update_file on the default branch.
- scaffold the 7 dossier files under `dossiers/{id}/` on the default branch via create_or_update_file: `overview.md` gets YAML frontmatter (status: Active, confidence: Low, category: Uncategorized, last-updated: now, topics: []) + `# {title}` + the question under "## Situation Summary"; the other 6 get a minimal `# <Title>`-style header. One commit per file is acceptable for v1 (note the rate-limit tradeoff in a comment).
- then trigger research: call `run_agent(dry_run=false)` with `topic_id=id`, `query=question` (so the first PR is generated). Return `Json` of `{ id, pr_url }`.

Frontend:
- wire the Prompt-10 New-Dossier modal submit (remove the stub): POST `/api/dossiers` with {title, question}; on success, close the modal, navigate to `#/dossier/{id}` (the skeleton renders immediately), and show a non-blocking toast "Research started — a PR will appear for review."
- subscribe to `/api/events` (EventSource) on app init; on a `pr_ready` event, show a toast linking to the PR reviewer (route added in Prompt 22) for that dossier.

Tests (backend, wiremock for GitHub; agent subprocess `#[ignore]`d or mocked at the run_agent boundary): assert slug generation/uniqueness and that the 7 files + .workbench defaults are PUT on the default branch with correct paths/content. 

Verify: click New Dossier → fill title+question → dossier appears in the workspace with seeded files → after the agent finishes, an event/toast announces the PR. `cargo test` green; no framework usage.

Commit: `feat: new-dossier creation seeds repo skeleton and triggers first research PR`
```

---

## Phase 9 — Visual PR Reviewer

### Prompt 20 — PR read endpoints (list / detail / diff / conflict)

```text
Expose PR data + diffs + conflict views. spec.md §4.3 + §5.3. Reuses Prompt 7. Design decision #4 (diffy 3-way merge, similar diff).

Add deps: similar, diffy.

Endpoints (Result<_, AppError>; GitHubClient from state):
- `GET /api/prs` -> `Json<Vec<PrSummary>>`: `list_pull_requests(state="open")`, keep only PRs whose head ref starts with `ai-update/`; PrSummary { number, title, html_url, head_ref, mergeable }.
- `GET /api/prs/:number` -> `Json<PrDetail>`:
    * fetch the PR (get_pull_request) and its files (get_pull_request_files).
    * for each changed Markdown file build a `FileDiff`:
        - `base_text` = get_file_content(path, ref = base.ref) (main),
        - `head_text` = get_file_content(path, ref = head.ref),
        - `unified_diff` = a unified diff string of base→head via `similar` (TextDiff::from_lines, unified_diff()),
        - 3-way conflict: fetch `ancestor_text` = get_file_content(path, ref = pr.merge_base) if available (else base_text), run `diffy::merge(ancestor, ours=base_text, theirs=head_text)`. If Ok(merged) -> `conflicted=false`, provide merged as the editor's initial text; if Err(conflicted_markers) -> `conflicted=true`, provide the conflict-marked text (standard <<<<<<< ======= >>>>>>>) as the editor's initial text.
    * PrDetail { number, title, head_ref, base_ref, mergeable, files: Vec<FileDiff> } where FileDiff { path, unified_diff, conflicted, editor_text, head_sha }.
    Offload `similar`/`diffy` computation via spawn_blocking for large files (CLAUDE.md).

Tests (wiremock + unit): mock a PR with two changed files (one cleanly mergeable, one conflicting) and assert `conflicted` flags and that conflict markers appear in the conflicting file's `editor_text`. Unit-test the similar/diffy helpers directly on small fixtures.

Wiring: consumed by the PR reviewer UI (Prompt 22) and the mutation endpoints (Prompt 21). Add routes above the static fallback.

Verify: with a real `ai-update/*` PR open, `/api/prs` lists it and `/api/prs/<n>` returns per-file diffs + conflict text. `cargo test` green.

Commit: `feat(backend): PR list + per-file diff and 3-way conflict view endpoints`
```

### Prompt 21 — PR mutation endpoints (resolve / merge / discard)

```text
Add the PR write actions backing the reviewer footer. spec.md §4.3 steps 4–5 + §5.3 footer. Reuses Prompt 7.

Endpoints (Result<_, AppError>):
- `POST /api/prs/:number/resolve` body `ResolveRequest { path: String, resolved_text: String }`:
    fetch the file's current blob sha on the HEAD branch (get_file_content(path, ref=head.ref).sha), then `create_or_update_file(path, resolved_text, "resolve conflict in {path}", branch=head.ref, sha=Some(head_sha))`. Return `Json({ "ok": true })`. (This commits the analyst's clean resolution onto the AI branch, per spec.md §4.3 step 5.)
- `POST /api/prs/:number/merge`: `merge_pull_request(number, commit_title=Some(...))`; on success return ok. (Optionally also delete the head ref after merge via delete_ref — gate behind a request flag, default true.)
- `POST /api/prs/:number/discard`: `close_pull_request(number)` then `delete_ref("heads/{head.ref}")`. Return ok. (Tolerate a missing ref on delete.)

All three must resolve the PR's head ref first via get_pull_request (don't trust the client to supply it).

Tests (wiremock): assert resolve PUTs the resolved text to the head branch with the correct sha; assert merge calls the merge endpoint; assert discard closes then deletes the ref; assert a 409/422 from GitHub (e.g. merge blocked) surfaces as the mapped AppError, not a panic.

Wiring: consumed by the reviewer footer in Prompt 22.

Verify: against a real branch, resolve → commit lands on the AI branch; merge → PR merges into main; discard → PR closed + branch deleted. `cargo test` green.

Commit: `feat(backend): PR resolve/merge/discard endpoints`
```

### Prompt 22 — Visual PR Reviewer UI

```text
Build the PR reviewer screen and close the GitOps loop in the UI. spec.md §5.3. Vanilla JS + custom Pico; consumes Prompts 20–21.

Router: add `#/prs` (list) and `#/prs/:number` (reviewer). Add a "Pending PRs" entry point in the header (and have the Prompt-19 `pr_ready` toast link to `#/prs/{number}`).

PR list view (`#/prs`): fetch `/api/prs`; render rows showing title, head_ref, and a mergeable/conflict badge; clicking opens the reviewer.

Reviewer view (`#/prs/:number`): fetch `/api/prs/{number}`. For each changed file render:
- a DIFF panel showing the `unified_diff` with line coloring (added = green, removed = red); offer a toggle between unified and a simple side-by-side (split base/head) view.
- a CONFLICT EDITOR: a <textarea> seeded with `editor_text`. When `conflicted`, syntax-highlight the standard markers in a synced overlay (or by styling): the `<<<<<<< HEAD … =======` region tinted RED (local/HEAD), the `======= … >>>>>>> ai-update/...` region tinted GREEN (incoming AI), per spec.md §5.3. The analyst edits the textarea to a clean resolution (removing markers).
  Implement marker highlighting with a regex over the textarea content rendered into a positioned backdrop <pre> — pure vanilla, no editor library.

Action footer (per file or per PR, your call — keep it obvious):
- "Save Manual Edits" -> POST `/api/prs/{number}/resolve` {path, resolved_text} for the edited file(s); on success refetch the PR (mergeable should flip).
- "Approve & Merge" -> POST `/api/prs/{number}/merge`; on success toast + navigate back to `#/prs`.
- "Discard PR" -> confirm dialog -> POST `/api/prs/{number}/discard`; on success back to `#/prs`.

Disable Merge while unresolved conflicts remain (mergeable === false). Handle/show backend errors inline. Keyboard-accessible buttons; confirm-before-discard.

Verify (full loop): trigger a research PR that conflicts with a manual main edit → open the reviewer → see red/green conflict markers → edit to a clean resolution → Save → mergeable flips → Approve & Merge → PR lands on main and the dossier reflects it; Discard path also works. No framework usage; no console errors. Run spec.md §7.3 checklist again on these screens.

Commit: `feat(frontend): visual PR reviewer with diff view, conflict highlighting, and merge/resolve/discard`
```

---

## Phase 10 — Config UI + Final QA

### Prompt 23 — Profile/style config endpoints + settings panel

```text
Let the analyst edit personalization (spec.md §3 .workbench/*), which the chat context (Prompt 13) already consumes. Reuses Prompt 6/7.

Backend:
- `GET /api/config/profile` -> Json: read `.workbench/profile.json` (return parsed JSON; if absent return the default shape). `PUT /api/config/profile` body = the profile JSON: validate it parses, then create_or_update_file on the default branch (fetch existing sha first). Return ok.
- `GET /api/config/style` -> { content }: read `.workbench/analyst_style.md` (default "" if absent). `PUT /api/config/style` body { content }: create_or_update_file on default branch.
Validate the profile JSON against the spec.md §3 fields (analyst_name, focus_sectors[], preferred_depth, prior_assumptions[]) leniently (extra keys allowed); reject malformed JSON with a 422 AppError.

Frontend:
- a "Settings" entry in the header opening a `#/settings` route (or modal): a form for the profile fields (name, focus sectors as a tag/CSV input, preferred_depth select, prior_assumptions as a list) and a textarea for analyst_style.md. Load via the GET endpoints, save via the PUT endpoints, show success/error inline.

Tests (wiremock): assert GET returns defaults when files absent; PUT writes valid JSON/markdown to the default branch with the right path; malformed profile JSON 422s.

Wiring/verify: edit the profile (e.g. add a focus sector) → save → open a dossier chat → confirm the new context is reflected in the system payload (the build_context from Prompt 13 reads the updated file). `cargo test` green; no framework usage.

Commit: `feat: profile.json and analyst_style.md editing endpoints + settings UI`
```

### Prompt 24 — Final integration, QA, and README

```text
Final wiring pass, full quality gate, and run docs. Enforce the CLAUDE.md "Before Committing Checklist" across the whole repo.

Tasks:
- README.md (real now): prerequisites (Rust toolchain, uv, Node not required), how to configure (.env vars: GITHUB_OWNER/REPO/TOKEN, LLM_API_BASE/KEY/MODEL, optional TAVILY_API_KEY/SEMANTIC_SCHOLAR_API_KEY, GITHUB_API_BASE for tests), how to run (`cargo run` in rust-backend serves UI + API on :8787; the agent is invoked automatically as a subprocess). Document the v1 security simplification (PAT in gitignored .env, no keychain) explicitly.
- Verify the router order: /health, /api/* (all of them), then the ServeDir static fallback — no route shadowing.
- End-to-end smoke (manual script or checklist in README): setup → new dossier → research PR appears → chat works → PR reviewer resolve/merge → dossier updated on main.
- Quality gate (must all pass):
    Rust:   `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`
    Python: `uv run ruff check`, `uv run ruff format --check`, `uv run mypy src`, `uv run pytest`
    Frontend: grep clean of `React|jQuery|\bsvelte\b`; spec.md §7.3 manual checklist (theme switch no jank; mobile stacks cleanly; legible both themes).
- Confirm no secrets are logged anywhere (grep the codebase for token/key logging); confirm .env, .venv/, /target, test-output/ are gitignored; confirm no committed debug prints (println!/dbg!/print) and no commented-out code.
- Fix anything the gate surfaces (formatting, clippy/mypy findings, dead code, missing doc comments on public items).

Verify: every command in the quality gate exits clean; the end-to-end smoke passes; README lets a fresh developer run the app.

Commit: `chore: final integration pass, quality gate, and run documentation`
```

---

## Notes for the implementer

- **Order is load-bearing.** Endpoint paths, type names, the tab→filename map, and the agent's JSON stdout contract are referenced by later prompts verbatim. If you rename something, propagate it.
- **Every external boundary is mocked in tests** (`wiremock` for GitHub/LLM in Rust; `unittest.mock` for httpx/LLM in Python). The only un-mocked external interaction is the Rust→Python subprocess, which is `#[ignore]`d in CI and exercised manually.
- **Secrets never leave env/`.env`.** PAT and LLM key are passed to the child process via environment, not argv; they are never logged and the `secrecy` wrapper guards accidental Debug printing.
- **DRY the two frontmatter-split sites** (Prompt 8 parsing and Prompt 9 render-stripping) into one helper the first time the second site appears.
- **YAGNI for v1:** no OS keychain, no local clone, no client-side Markdown lib, no extra providers beyond the four specified (two of them optional/keyless-first). Add them only when a later requirement demands it.
