# Intelligence Workbench — Build Checklist (`todo.md`)

Tracks `prompt_plan.md` 1:1. Work top-to-bottom; each prompt ends with its tests passing, wiring done, and a commit. The **Per-commit gate** (bottom) runs before every commit.

Legend: `[ ]` todo · `[~]` in progress · `[x]` done · `[-]` skipped/N-A

---

## Decisions to confirm before starting

- [ ] **Conflict view**: keep `diffy` 3-way merge marker reconstruction (P20), or simplify to side-by-side base/head only?
- [ ] **PR creation owner**: keep PR creation in the Python agent (P16–P17) with Rust holding only the write methods it calls (P7), or move PR creation into Rust?
- [ ] **Unpinned crates**: confirm current name/version for the YAML crate (`serde_yml`?), `diffy`, `similar`.
- [ ] **GitHub client**: confirm `reqwest` (per spec) vs. adopting `octocrab` for read-only calls.
- [ ] **LLM provider**: confirm OpenAI-compatible Chat Completions endpoint (or substitute provider in `llm.rs` / `synthesis.py`).

---

## Project-wide setup & conventions

- [ ] Rust toolchain installed (`rustup`, stable); `cargo fmt`, `cargo clippy` available
- [ ] `uv` installed (Python ≥ 3.11)
- [ ] Decide repo host & create empty target GitHub repo (the data store)
- [ ] Generate a GitHub PAT with repo scope for the target repo
- [ ] Have LLM provider creds ready
- [ ] `.env` keys documented and ready (do **not** commit):
  - [ ] `GITHUB_OWNER`, `GITHUB_REPO`, `GITHUB_TOKEN`
  - [ ] `LLM_API_BASE`, `LLM_API_KEY`, `LLM_MODEL`
  - [ ] optional: `TAVILY_API_KEY`, `SEMANTIC_SCHOLAR_API_KEY`
  - [ ] tests: `GITHUB_API_BASE` override points at the mock server
- [ ] Confirm gitignore covers: `/target`, `.env`, `.venv/`, `**/__pycache__/`, `web/pkg/`, `test-output/`
- [ ] Conventions internalized: no `.unwrap()` outside tests; errors via `tracing::error!` / `logger.error`; secrets only via env; PAT/LLM key never logged or passed as argv; doc comments on all public items; rustfmt 100-col / ruff 88-col; no emoji/unicode-emoji in code

---

## Phase 0 — Scaffolding & Toolchain

### P1 — Rust/Axum server skeleton
- [ ] Create git root: `.gitignore`, `README.md` (WIP stub)
- [ ] `cargo init --name workbench` in `rust-backend/`
- [ ] Add deps: axum, tokio(full), tower, tower-http(fs,trace,timeout,compression-full), serde(derive), serde_json, thiserror, anyhow, tracing, tracing-subscriber(env-filter), reqwest(json)
- [ ] `src/errors.rs`: `AppError` (GitHubError/IoError/AgentError) + `impl IntoResponse` (spec §6.1)
- [ ] `src/state.rs`: empty `AppState` (derive Clone) placeholder
- [ ] `src/main.rs`: tracing init (EnvFilter, default info)
- [ ] `src/main.rs`: router with `GET /health` → 200 `{"status":"ok"}`
- [ ] Apply `TraceLayer` + request timeout layer
- [ ] Bind `127.0.0.1:8787`, serve via tokio listener
- [ ] Test: `AgentError` → 422
- [ ] Verify: `cargo run` + `curl /health`; `cargo test`; `cargo clippy -- -D warnings` clean
- [ ] Commit: `feat(backend): axum skeleton with health route, tracing, and AppError`

### P2 — Python/`uv` agent skeleton
- [ ] Create `python/` with `pyproject.toml`, `src/workbench_agent/{__init__,__main__}.py`, `tests/test_smoke.py`
- [ ] `uv` project + `.venv` (gitignored)
- [ ] `pyproject`: name `workbench-agent`, requires-python ≥3.11
- [ ] Runtime deps: polars, orjson, httpx
- [ ] Dev deps: pytest, mypy, ruff
- [ ] ruff config (line 88, PEP 8); mypy config (strict-ish, no avoidable Any)
- [ ] Install ipykernel + ipywidgets into venv only (not project deps)
- [ ] `__init__.py`: `__version__ = "0.1.0"`
- [ ] `__main__.py`: argparse stub → `--version` prints version, exit 0
- [ ] Module logger "workbench_agent" at INFO; use logger.* never print
- [ ] Test: `__version__` is non-empty str (discrete file)
- [ ] Verify: `uv run python -m workbench_agent --version`; `uv run pytest`; `uv run ruff check`; `uv run mypy src` clean
- [ ] Commit: `feat(agent): uv project skeleton with CLI stub, logging, and smoke test`

### P3 — Frontend static scaffold + theme toggle
- [ ] Create `web/{index.html,app.js,styles.css}` + `web/vendor/` (vendored `pico.min.css`, no runtime CDN)
- [ ] `index.html`: header (title left, theme-toggle right) + `<main id="app">`; link pico then styles.css; two self-hosted Google Fonts (sans headers / serif body), documented in CSS
- [ ] `styles.css`: palette CSS custom properties; `[data-theme="light"]`/`[data-theme="dark"]` overrides; fonts wired; distinct from Pico defaults
- [ ] `app.js`: theme module (localStorage `workbench-theme`, else `prefers-color-scheme`, toggle persists)
- [ ] `app.js`: `init()` on DOMContentLoaded renders "loading…" placeholder
- [ ] Axum: serve `web/` at `/` via `ServeDir` + fallback to `index.html`; keep `/health` and `/api/*` above the static fallback
- [ ] Verify: page loads, fonts applied, toggle persists across reload; no console errors; grep clean of `React|jQuery|\$\(`
- [ ] Commit: `feat(frontend): static scaffold with custom Pico theme and dark/light toggle`

---

## Phase 1 — Configuration & GitHub Auth

### P4 — Settings, AppState, setup status
- [ ] Add deps: dotenvy, secrecy(serde)
- [ ] `src/config.rs`: `WorkbenchConfig { owner, repo, github_token: SecretString }` (Optionals; private fields + accessors; redact token in any Debug)
- [ ] `WorkbenchConfig::load()`: `dotenvy::dotenv().ok()` + read `GITHUB_OWNER/REPO/TOKEN`
- [ ] `is_configured()` (all three present)
- [ ] `src/state.rs`: `AppState { config: Arc<RwLock<WorkbenchConfig>>, http: reqwest::Client }` (Clone); build client with User-Agent + timeout
- [ ] `main.rs`: construct AppState at startup, `.with_state(state)`
- [ ] `GET /api/setup/status` → `{configured: bool}` (never leak token)
- [ ] Test: `is_configured` false if any None, true if all set
- [ ] Verify: `curl /api/setup/status` → `{"configured":false}`; clippy clean; token absent from logs
- [ ] Commit: `feat(backend): config loading via dotenvy/secrecy, AppState, setup-status endpoint`

### P5 — PAT validation + first-launch setup
- [ ] `POST /api/setup` body `{owner, repo, token}`
- [ ] Validate via GitHub `GET /user` (Bearer, UA, Accept); parse `login`; non-2xx → mapped AppError; never log token
- [ ] On success: persist `.env` (replace the 3 keys, preserve others) **and** update in-memory `state.config` (write lock, wrap token)
- [ ] Response `{login}`
- [ ] `src/github/mod.rs` seed: `get_authenticated_login(http, token)`; handler calls it
- [ ] Add `GITHUB_API_BASE` env override (default `https://api.github.com`)
- [ ] Test (wiremock): 200→login parsed; 401→error (discrete module)
- [ ] Frontend: on init fetch status; if not configured render setup form (PAT field type=password)
- [ ] Frontend: submit → POST `/api/setup`; success → "Connected as {login}" → empty app-shell placeholder; failure → inline error
- [ ] Verify: fresh start shows form; valid submit writes `.env`, status flips, reload skips form; `.env` gitignored; token not logged
- [ ] Commit: `feat: first-launch GitHub PAT setup with validation and secure persistence`

---

## Phase 2 — GitHub REST Client (Rust)

### P6 — GitHub read client + live health check
- [ ] Newtypes: `Owner/Repo/Branch/Sha` (`as_str()`; Debug/Clone/PartialEq)
- [ ] `GitHubClient { http, base, token }` from `&AppState`; private authorized-RequestBuilder helper (DRY)
- [ ] Models: `RepoTreeEntry`, `TreeResponse`, `ContentResponse`
- [ ] `get_authenticated_login()` (move P5 logic here)
- [ ] `get_tree(owner, repo, ref, recursive)`
- [ ] `get_file_content(owner, repo, path, ref?)` → `FileContent { text, sha }`; add `base64` dep; decode (spawn_blocking if large)
- [ ] Tests (wiremock): trees + contents fixtures incl. base64 decode (one per method)
- [ ] Wiring: `GET /api/health/github` builds client, returns `{login}`; replace inline P5 call with client method
- [ ] Verify: `cargo test`; configured `curl /api/health/github` → login; clippy clean
- [ ] Commit: `feat(backend): typed GitHub read client (trees, contents) with mocked tests`

### P7 — GitHub write client
- [ ] Models: `PullRequest`(+`PrRef`), `PullFile`, `ContentWriteResponse`(+meta)
- [ ] `create_or_update_file(owner, repo, path, content, message, branch, sha?)` → new blob `Sha`
- [ ] `list_pull_requests(owner, repo, state)` (open, per_page 100)
- [ ] `get_pull_request(owner, repo, number)`
- [ ] `get_pull_request_files(owner, repo, number)`
- [ ] `merge_pull_request(owner, repo, number, title?)`
- [ ] `close_pull_request(owner, repo, number)` (PATCH state closed)
- [ ] `delete_ref(owner, repo, ref_name)`
- [ ] Tests (wiremock): one per method incl. `mergeable:false`; verify PUT/PATCH/merge request bodies (base64, branch, sha)
- [ ] Wiring note: methods consumed by P18/P19/P20/P21 (live on the wired client — not orphaned)
- [ ] Verify: `cargo test`; clippy clean
- [ ] Commit: `feat(backend): GitHub write client (contents PUT, PR list/get/files/merge/close, ref delete)`

---

## Phase 3 — Dossier Domain Model

### P8 — Frontmatter parsing + listing/reading
- [ ] Add deps: YAML crate (e.g. `serde_yml`), chrono(serde)
- [ ] `DossierMeta` (serde, hyphen renames): last_updated, confidence, status, category, topics[]
- [ ] `parse_dossier_frontmatter(markdown)` — split leading `---…---`, parse YAML, default if absent (**keep this exact name** — spec §7.1)
- [ ] `DossierSummary { id, title, meta }` (Serialize)
- [ ] `DOSSIER_FILES` const (7 stems) + tab→filename helper
- [ ] `list_dossiers(client, owner, repo)` (recursive tree → per-id overview → frontmatter + `# ` title; concurrent via `try_join_all`; add `futures`)
- [ ] `read_dossier_files(client, owner, repo, id)` → BTreeMap stem→content (missing → empty)
- [ ] Test: spec §7.1 case → status "Active", confidence "Medium"
- [ ] Test: no-frontmatter → defaults
- [ ] Test (wiremock): `list_dossiers` returns expected summaries
- [ ] Wiring: consumed by P9
- [ ] Verify: `cargo test`; clippy clean
- [ ] Commit: `feat(backend): dossier model, frontmatter parsing, list/read from repo`

---

## Phase 4 — Read API + Dashboard

### P9 — Read endpoints + server-side Markdown render
- [ ] Add deps: pulldown-cmark, ammonia
- [ ] `src/render.rs`: `render_markdown(md)` — strip frontmatter (refactor split into shared helper, DRY), convert (tables/footnotes/strikethrough), sanitize via ammonia (allow tables + `<details>`/`<summary>`)
- [ ] Test: simple doc renders; `<script>` stripped
- [ ] `GET /api/dossiers` → `Vec<DossierSummary>`
- [ ] `GET /api/dossiers/:id` → `{id, meta, files: stem→raw_md}`
- [ ] `GET /api/dossiers/:id/render/:tab` → `Html`; validate tab in set (unknown → 404)
- [ ] Wrap `render_markdown` in `spawn_blocking` (offload CPU)
- [ ] Tests (wiremock + unit): `/api/dossiers` shape; unknown tab 404s
- [ ] Wiring: routes above static fallback; consumed by P10/P11
- [ ] Verify: real repo → `curl /api/dossiers` + `/render/overview` HTML; `cargo test`
- [ ] Commit: `feat(backend): dossier read endpoints + sanitized server-side markdown rendering`

### P10 — Dashboard UI
- [ ] `app.js` dashboard module: `renderDashboard(root)` fetch `/api/dossiers`
- [ ] Card deck (CSS grid): title, category, status pill, confidence, last-updated (Intl.DateTimeFormat)
- [ ] Explicit states: loading skeleton, empty, error inline
- [ ] "New Dossier" button → native `<dialog>` (Title + Initial Question); submit stub logs+closes (TODO → P19)
- [ ] `init()`: when configured, `renderDashboard(#app)` (replace P5 placeholder)
- [ ] A11y/HID: cards keyboard-focusable; dialog focus-trap + Escape
- [ ] Verify: configured app → dashboard from live data; empty + error reachable; modal opens/closes; no console errors; no frameworks
- [ ] Commit: `feat(frontend): dashboard card deck with new-dossier modal (submit stubbed)`

---

## Phase 5 — Workspace UI

### P11 — Hash router + split-pane workspace + tabs
- [ ] Hash router: `#/`→dashboard, `#/dossier/:id`→workspace; `hashchange` + initial dispatch; card click navigates
- [ ] `renderWorkspace(root, id)`: grid `2fr 1fr` desktop (+ breakpoints in CSS)
- [ ] Left pane tab strip (ids: overview, causal_models, timeline, entities, evidence_assessment, sources, forecasts) → fetch `/render/{tab}` into content; in-memory cache per session; default Overview
- [ ] Right pane: `#chat-sidebar` placeholder
- [ ] Back-to-dashboard control
- [ ] innerHTML injection comment noting server-side ammonia sanitization invariant
- [ ] Verify: card → workspace Overview; tab switching loads each file; back works; no console errors
- [ ] Commit: `feat(frontend): hash router + split-pane workspace with 7 markdown tabs`

### P12 — Typography/responsive + chat shell
- [ ] CSS: typographic scale for rendered content (sans headers, serif body, max reading width, styled code/tables/`<details>`); legible both themes
- [ ] CSS: responsive collapse to single column below tablet (content first, chat below); long lists/timeline stack cleanly
- [ ] Chat markup into `#chat-sidebar`: scrollable `#chat-history` + `#chat-input` textarea + Send; DISABLED/inert (no network)
- [ ] Verify: desktop↔mobile reflow no overflow; legible both themes; sidebar inert; run spec §7.3 checklist
- [ ] Commit: `feat(frontend): workspace typography, responsive layout, and inert chat sidebar shell`

---

## Phase 6 — Chat

### P13 — Chat context builder + LLM client + SSE endpoint
- [ ] Add `futures` if needed; SSE-parse helper for upstream stream
- [ ] Extend `WorkbenchConfig`: `LLM_API_BASE/API_KEY(SecretString)/MODEL` (+ accessors; never log key)
- [ ] `src/chat.rs` `build_context(client, owner, repo, id)`: read 7 files + `.workbench/profile.json` + `analyst_style.md` (missing omitted); assemble exact spec §4.4 payload (`=== FILE: … ===`, profile, style, instructions)
- [ ] `src/llm.rs` `stream_chat(...)`: POST `{base}/chat/completions` `stream:true`; yield token deltas; map failures to AppError
- [ ] `POST /api/dossiers/:id/chat` body `{message, history[]}`: build system context + messages → relay `Sse` of `data:` token events + `done` sentinel
- [ ] Tests (wiremock): mock LLM SSE → concatenation; `build_context` payload contains delimiters + profile
- [ ] Wiring: route above static fallback; consumed by P14
- [ ] Verify: `cargo test`; real creds → `curl -N` streams tokens; PAT + LLM key not logged
- [ ] Commit: `feat(backend): grounded chat context builder + streaming LLM SSE endpoint`

### P14 — Chat UI wiring
- [ ] In-memory `history[]` per open dossier (reset on dossier change)
- [ ] Send: append user turn → POST `/chat` via fetch + ReadableStream reader; parse `data:` lines; live-update assistant bubble until `done`; push full turn to history
- [ ] Errors → inline error bubble; disable Send while streaming, re-enable after
- [ ] Enable the previously-inert input/Send
- [ ] Render assistant text minimally (preserve line breaks); **no client-side Markdown lib**
- [ ] Verify: grounded Q&A streams; multi-turn retained + sent each request; dossier switch clears history; no frameworks; no console errors
- [ ] Commit: `feat(frontend): streaming chat sidebar wired to the dossier SSE endpoint`

---

## Phase 7 — Python Research Agent

### P15 — Research fetch + Polars normalization
- [ ] `providers.py`: keyless `wikipedia_search`, `arxiv_search` (stdlib xml)
- [ ] `providers.py`: optional keyed `tavily_search` (TAVILY_API_KEY), `semantic_scholar_search` (read keys at call time)
- [ ] Common row shape `{title,url,snippet,source,published}`
- [ ] `research.py` `fetch_research(query, limit_per_source=5)` → `pl.DataFrame`: call enabled providers, normalize, drop dup URLs, sort; never print count+schema alongside frame; ≤10 rows per analysis helper
- [ ] httpx with timeouts + per-provider try/except (logger.error + continue on failure)
- [ ] Tests (`tests/test_research.py`, discrete): patch httpx with fixtures; assert normalized columns, dup removal, failing provider skipped; all network mocked
- [ ] Type hints everywhere; mypy + ruff clean
- [ ] Wiring: consumed by P17
- [ ] Verify: `uv run pytest tests/test_research.py -v`; mypy + ruff clean
- [ ] Commit: `feat(agent): multi-source research fetch with Polars normalization and mocked tests`

### P16 — Agent GitHub-REST + LLM synthesis
- [ ] `github.py` `GitHubRepo {owner, repo, token(env), base(env)}` + `GitHubError`
- [ ] `default_branch_sha()`
- [ ] `create_branch(new, from_sha)` (idempotency policy on exists)
- [ ] `get_file(path, ref)` → `(text, sha|None)`
- [ ] `put_file(path, content, message, branch, sha?)`
- [ ] `create_pull_request(title, head, base, body)` → `{number, html_url}`
- [ ] orjson (de)serialize; never log token/URL-with-token
- [ ] `synthesis.py` `synthesize(existing_files, research, profile, query, model)` → `{filename: new_content}`; preserve frontmatter/structure; call LLM via httpx; parse JSON (strip fences, orjson.loads); return only changed files
- [ ] Tests (`test_github.py`, `test_synthesis.py`, discrete): mock httpx; assert request bodies (base64/branch/sha create-vs-update) + parsing; synthesis parses fenced JSON, ignores unchanged; no real network
- [ ] mypy + ruff clean
- [ ] Wiring: consumed by P17
- [ ] Verify: `uv run pytest -v`; mypy + ruff clean
- [ ] Commit: `feat(agent): GitHub REST write layer + LLM dossier synthesis with mocked tests`

### P17 — `run_research` orchestration + CLI + JSON contract
- [ ] `agent.py` `run_research(topic_id, query)` (spec §4.2): fetch → read 7 files (default branch) → read profile.json (default `{}`) → synthesize → branch `ai-update/{id}` from default sha → per-file get sha on branch + put → create PR
- [ ] Return `{topic_id, branch, pr_number, pr_url, changed_files[]}`
- [ ] `__main__.py` argparse: required `--owner/--repo/--topic-id/--query`; optional `--dry-run` (steps 1–4, `dry_run:true`); creds from env
- [ ] On success: write result to STDOUT as single-line orjson (**the Rust contract**)
- [ ] try/except (spec §6.2): exception → logger.error(exc_info=True) to STDERR + `SystemExit(1)`; STDOUT stays clean
- [ ] Tests (`test_agent.py`, extend spec §7.2): mock all; assert create_branch→put→create_pull_request order + contract dict; `--dry-run` skips writes; provider error → SystemExit(1) + clean stdout
- [ ] mypy + ruff clean
- [ ] Wiring: contract consumed by P18
- [ ] Verify: `uv run python -m workbench_agent … --dry-run` prints JSON line; `uv run pytest -v`; mypy + ruff clean
- [ ] Commit: `feat(agent): run_research orchestration, CLI, and JSON stdout contract`

---

## Phase 8 — Research Orchestration (Rust ↔ Python)

### P18 — Rust spawns agent + SSE progress
- [ ] `src/agent.rs` `run_agent(state, owner, repo, topic_id, query, dry_run)`: `tokio::process::Command` `uv run python -m workbench_agent …` with `current_dir(python/)`; secrets via child **ENV** (not argv); capture stdout/stderr
- [ ] Non-zero exit → log stderr (tracing::error!) → `AppError::AgentError`; parse single-line stdout JSON → `AgentResult`; parse failure → AgentError
- [ ] `AgentResult` (Deserialize matching P17 contract)
- [ ] `POST /api/dossiers/:id/research` body `{query}` → `run_agent(dry_run=false)` → `Json<AgentResult>`
- [ ] SSE progress: `GET /api/events` backed by `tokio::sync::broadcast` in AppState; publish `{type:"pr_ready", topic_id, pr_url}` on completion
- [ ] Tests: stdout-JSON → AgentResult (canned line); real-subprocess test `#[ignore]`/env-gated
- [ ] Wiring: routes above static fallback; consumed by P19
- [ ] Verify: configured `POST …/research` returns real PR URL; `/api/events` emits `pr_ready`; `cargo test`
- [ ] Commit: `feat(backend): spawn python agent via uv, parse PR result, SSE progress channel`

### P19 — New-Dossier flow
- [ ] `POST /api/dossiers` body `{title, question}`
- [ ] Slug `id` from title (lowercase/hyphenate/strip; uniqueness vs existing → `-2/-3`)
- [ ] If `.workbench/profile.json`/`analyst_style.md` absent on main → create defaults (profile per spec §3 shape) via create_or_update_file (default branch)
- [ ] Scaffold 7 files under `dossiers/{id}/` on default branch: `overview.md` frontmatter (Active/Low/Uncategorized/now/[]) + `# {title}` + question under "## Situation Summary"; other 6 minimal headers (one commit/file OK for v1 — note rate-limit tradeoff)
- [ ] Trigger `run_agent(dry_run=false, topic_id=id, query=question)`; return `{id, pr_url}`
- [ ] Frontend: wire modal submit (remove stub) → POST `/api/dossiers` → close, navigate `#/dossier/{id}`, toast "Research started…"
- [ ] Frontend: subscribe `/api/events` (EventSource) on init; on `pr_ready` toast linking to PR reviewer
- [ ] Tests (wiremock; agent boundary mocked/`#[ignore]`): slug uniqueness; 7 files + `.workbench` defaults PUT to default branch with correct paths/content
- [ ] Verify: New Dossier → seeded workspace → agent finishes → event/toast for PR; `cargo test`; no frameworks
- [ ] Commit: `feat: new-dossier creation seeds repo skeleton and triggers first research PR`

---

## Phase 9 — Visual PR Reviewer

### P20 — PR read endpoints (list/detail/diff/conflict)
- [ ] Add deps: similar, diffy
- [ ] `GET /api/prs` → `Vec<PrSummary>`: open PRs filtered to head `ai-update/*`; `{number,title,html_url,head_ref,mergeable}`
- [ ] `GET /api/prs/:number` → `PrDetail`: get PR + files; per changed `.md`:
  - [ ] `base_text` (ref=base), `head_text` (ref=head)
  - [ ] `unified_diff` via `similar` (TextDiff::from_lines → unified_diff)
  - [ ] 3-way: `ancestor` (ref=merge_base else base) → `diffy::merge(ancestor, ours=base, theirs=head)` → Ok→`conflicted=false`+merged; Err→`conflicted=true`+marker text
  - [ ] `FileDiff { path, unified_diff, conflicted, editor_text, head_sha }`
- [ ] `PrDetail { number, title, head_ref, base_ref, mergeable, files[] }`
- [ ] Offload similar/diffy via spawn_blocking (large files)
- [ ] Tests (wiremock + unit): two files (mergeable + conflicting) → flags + markers present; unit-test similar/diffy helpers
- [ ] Wiring: routes above static fallback; consumed by P21/P22
- [ ] Verify: real `ai-update/*` PR → `/api/prs` lists + `/api/prs/:n` per-file diff + conflict text; `cargo test`
- [ ] Commit: `feat(backend): PR list + per-file diff and 3-way conflict view endpoints`

### P21 — PR mutation endpoints (resolve/merge/discard)
- [ ] All three: resolve head ref via get_pull_request first (don't trust client)
- [ ] `POST /api/prs/:number/resolve` body `{path, resolved_text}`: get head blob sha → create_or_update_file on head branch with sha → `{ok:true}`
- [ ] `POST /api/prs/:number/merge`: merge_pull_request(title); optional delete head ref (flag, default true)
- [ ] `POST /api/prs/:number/discard`: close_pull_request → delete_ref `heads/{head}` (tolerate missing ref)
- [ ] Tests (wiremock): resolve PUTs correct text+sha to head; merge calls merge; discard closes+deletes; GitHub 409/422 → mapped AppError (no panic)
- [ ] Wiring: consumed by P22
- [ ] Verify: resolve commits to AI branch; merge lands on main; discard closes+deletes; `cargo test`
- [ ] Commit: `feat(backend): PR resolve/merge/discard endpoints`

### P22 — Visual PR Reviewer UI
- [ ] Router: `#/prs` (list) + `#/prs/:number` (reviewer); header "Pending PRs" entry; P19 toast links to `#/prs/{number}`
- [ ] `#/prs`: fetch `/api/prs`; rows (title, head_ref, mergeable/conflict badge); click → reviewer
- [ ] `#/prs/:number`: fetch detail; per file:
  - [ ] Diff panel: `unified_diff` colored (added green / removed red); unified↔side-by-side toggle
  - [ ] Conflict editor: textarea seeded with `editor_text`; when conflicted, highlight markers via regex backdrop `<pre>` — HEAD region red, incoming AI region green (spec §5.3); pure vanilla, no editor lib
- [ ] Footer: "Save Manual Edits" → POST `/resolve` (refetch after, mergeable flips); "Approve & Merge" → POST `/merge` (toast + back to `#/prs`); "Discard PR" → confirm → POST `/discard` (back to `#/prs`)
- [ ] Disable Merge while `mergeable === false`; inline backend errors; keyboard-accessible; confirm-before-discard
- [ ] Verify (full loop): conflicting PR → red/green markers → edit clean → Save → mergeable flips → Merge lands on main → dossier reflects; Discard works; no frameworks; spec §7.3 checklist
- [ ] Commit: `feat(frontend): visual PR reviewer with diff view, conflict highlighting, and merge/resolve/discard`

---

## Phase 10 — Config UI + Final QA

### P23 — Profile/style config + settings panel
- [ ] `GET /api/config/profile` (parsed JSON; default shape if absent)
- [ ] `PUT /api/config/profile` (validate parses → create_or_update_file on default branch, fetch existing sha first)
- [ ] `GET /api/config/style` → `{content}` (default "" if absent)
- [ ] `PUT /api/config/style` body `{content}` → create_or_update_file default branch
- [ ] Lenient validation of profile fields (analyst_name, focus_sectors[], preferred_depth, prior_assumptions[]; extra keys OK); malformed JSON → 422
- [ ] Frontend: "Settings" → `#/settings` (or modal): profile form (name, sectors tag/CSV, depth select, assumptions list) + style textarea; load via GET, save via PUT, inline success/error
- [ ] Tests (wiremock): GET defaults when absent; PUT writes to default branch with right path; malformed profile → 422
- [ ] Verify: edit profile (add sector) → save → open chat → updated context reflected (P13 build_context reads new file); `cargo test`; no frameworks
- [ ] Commit: `feat: profile.json and analyst_style.md editing endpoints + settings UI`

### P24 — Final integration, QA, README
- [ ] README real: prerequisites; `.env` config (all vars incl. `GITHUB_API_BASE` for tests); run (`cargo run` serves UI+API :8787, agent auto-invoked); document v1 security simplification (PAT in gitignored `.env`, no keychain)
- [ ] Verify router order: `/health` → all `/api/*` → ServeDir fallback (no shadowing)
- [ ] End-to-end smoke (script/checklist in README): setup → new dossier → research PR appears → chat → reviewer resolve/merge → dossier updated on main
- [ ] Quality gate — Rust: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`
- [ ] Quality gate — Python: `uv run ruff check`, `uv run ruff format --check`, `uv run mypy src`, `uv run pytest`
- [ ] Quality gate — Frontend: grep clean `React|jQuery|\bsvelte\b`; spec §7.3 manual checklist
- [ ] Secrets: grep for token/key logging (none); `.env`/`.venv/`/`/target`/`test-output/` gitignored; no `println!`/`dbg!`/`print` debug; no commented-out code
- [ ] Fix anything the gate surfaces (fmt, clippy/mypy, dead code, missing doc comments)
- [ ] Verify: every gate command exits clean; smoke passes; fresh dev can run from README
- [ ] Commit: `chore: final integration pass, quality gate, and run documentation`

---

## Per-commit gate (run before every commit)

- [ ] Rust: `cargo fmt --check`
- [ ] Rust: `cargo clippy -- -D warnings` (no warnings)
- [ ] Rust: `cargo test` (all pass)
- [ ] Python: `uv run ruff check` + `uv run ruff format --check`
- [ ] Python: `uv run mypy src` (no errors)
- [ ] Python: `uv run pytest` (all pass; generated test files kept; test output dir gitignored)
- [ ] Public items have doc comments + type hints
- [ ] No commented-out code, no debug statements (`println!`/`dbg!`/`print`)
- [ ] No hardcoded credentials; secrets only in `.env`; no secret/URL-with-secret logged
- [ ] Frontend touched → grep clean of `React|jQuery`; theme + responsive sanity
- [ ] Commit message is clear and conventional

---

## Acceptance (definition of done for v1)

- [ ] Fresh clone + `.env` → `cargo run` serves the full app on `:8787`
- [ ] First-launch setup validates and persists GitHub creds
- [ ] Dashboard lists dossiers from the repo with metadata
- [ ] Workspace renders all 7 Markdown tabs (server-sanitized)
- [ ] Chat streams grounded responses using the full dossier + profile/style
- [ ] "New Dossier" seeds the repo skeleton and produces a research PR
- [ ] Agent runs autonomously: research → synthesis → `ai-update/*` branch → PR
- [ ] PR reviewer shows diffs, highlights conflicts (red HEAD / green AI), and resolves → merges → reflects on main
- [ ] Profile/style editable; changes feed chat context
- [ ] All quality gates green; no secrets in code/logs; spec §7.3 frontend checklist passes
