# Technical Specification: Intelligence Workbench (v1.0)

This document is a developer-ready technical specification for the **Intelligence Workbench**, a modern, lightweight, "GitOps"-style intelligence system for analysts. The application serves as a stateless orchestration engine that manages a local-first, portable knowledge repository backed directly by a user's GitHub repository.

---

## 1. System Overview & Architecture

The Intelligence Workbench stores all analytical data, dossiers, and personalization configurations directly inside a single GitHub repository owned by the user. The application consists of three primary components:

```
                  +-------------------------------------------------+
                  |                 Web Browser                     |
                  |  - Customized Pico CSS & Vanilla JavaScript    |
                  |  - Light/Dark theme toggle                      |
                  +--------+-------------------------------+---------+
                           |                               |
                           | HTTP / SSE                    | Git Diff / Merge
                           v                               v
+--------------------------+----+               +----------+-----------+
|          Rust Backend         |               |     GitHub API       |
|  - Axum Web Server & API      |               |  - User Repository   |
|  - Auth: GitHub PAT           +-------------->+  - Dossiers (folders)|
|  - Branch/PR Orchestrator     |  HTTPS (REST) |  - .workbench/       |
+--------------------------+----+               +----------------------+
                           |
                           | subprocess (uv run)
                           v
+--------------------------+----+
|      Python Agent Engine      |
|  - uv & Polars Data Processing|
|  - Free Research APIs         |
|  - LLM Integration & Commits  |
+-------------------------------+
```

1. **Frontend (UI)**: Built with **Pico CSS** (with custom CSS/SCSS overrides) and **vanilla JavaScript**. It features an adaptive light/dark theme, modern typography, and a responsive split-pane design. It uses zero client-side frameworks (no React, no jQuery).
2. **Backend Server (Rust)**: An **Axum + Tokio** asynchronous HTTP server. It handles local server execution, routes request payloads, coordinates subprocess calls to the Python agent, and interfaces with the GitHub REST API using the user’s **GitHub Personal Access Token (PAT)**.
3. **Research Agent (Python)**: A background execution engine managed via **`uv`**. It performs autonomous web and academic searches using free-tier APIs, processes structural data with **Polars**, and edits dossier files before pushing them to feature branches on the user's repository.

---

## 2. Technical Stack & Constraints

All code contributions must align with the strict guidelines defined in `CLAUDE.md`:

### Rust Backend
* **Web Framework**: Axum. Request handlers must be asynchronous and return `Result<impl IntoResponse, AppError>` to centralize error handling.
* **Async Runtime**: Tokio. Offload any heavy parsing or CPU-bound work to `tokio::task::spawn_blocking`.
* **Serialization**: `serde` and `serde_json`.
* **Error Handling**: Custom error types using `thiserror` and application-level context mapping using `anyhow`. `.unwrap()` is prohibited in production paths; use `.expect()` with descriptive invariant messages where appropriate.
* **Logging**: Use `tracing::error!` or `log::error!` for error reporting instead of standard stdout prints.

### Python Agent Engine
* **Environment Management**: Managed strictly through `uv`. `.venv` must be added to `.gitignore`.
* **Serialization**: `orjson` for high-performance JSON operations.
* **Data Processing**: `polars` is mandated for all structured/tabular data manipulations. Do not use pandas.
* **Linter/Formatter**: Ruff (PEP 8 standard, 88-character line limit).
* **Type Safety**: Enforced type hints on all public function signatures, validated using `mypy`.

### Frontend
* **UI Framework**: Pico CSS combined with customized stylesheet overrides (CSS/SCSS) to produce a modern single-page-app layout. Component frameworks (React, Svelte, Vue) and jQuery are strictly prohibited.
* **Interactions**: Vanilla modern JavaScript (ES6+), leveraging native Web APIs.
* **Responsiveness**: Standard Human Interface Device (HID) guidelines, featuring a persistent responsive sidebar and light/dark theme switching native to Pico CSS.

---

## 3. Storage Schema & "GitOps" Engine

The user's GitHub repository acts as the single source of truth. The application is entirely stateless regarding data persistence; it reads, displays, and updates data by cloning or querying the configured repository.

### Repository Layout

```
repository-root/
│
├── .workbench/                        # User Personalization & Config
│   ├── profile.json                   # User background, goals, focus areas
│   └── analyst_style.md               # Code of style, prompt parameters, tone
│
└── dossiers/                          # All tracking projects
    ├── [dossier-id-a]/                # Subdirectory representing a unique topic
    │   ├── overview.md                # Summary, context, metadata (YAML)
    │   ├── timeline.md                # Event logs (Markdown list)
    │   ├── entities.md                # Major players (Markdown)
    │   ├── causal_models.md           # Hypotheses & crux analysis (Markdown)
    │   ├── evidence_assessment.md     # Bias checks & open questions (Markdown)
    │   ├── sources.md                 # Citation appendix (Markdown)
    │   └── forecasts.md               # Calibration ledger & AI benchmarks (Markdown)
    │
    └── [dossier-id-b]/
        └── ...
```

### Core File Schemas

#### 1. `.workbench/profile.json` (JSON)
Defines user priorities and constraints. Used to inject user-specific context into the LLM system prompts.
```json
{
  "analyst_name": "Senior Analyst",
  "focus_sectors": ["geopolitics", "semiconductor-supply-chains", "east-asia"],
  "preferred_depth": "detailed",
  "prior_assumptions": [
    "Supply chain bottlenecks in Southeast Asia are highly correlated with shipping lane stability."
  ]
}
```

#### 2. `dossiers/[dossier-id]/overview.md` (Markdown + YAML Frontmatter)
The entry point of a dossier.
```markdown
---
last-updated: 2026-06-19T06:50:00Z
confidence: Medium-High
status: Active
category: Geopolitics
topics:
  - military-strategy
  - east-asia
  - cross-strait
---

# Situation Overview: Cross-Strait Maritime Security

## Executive Summary
[Synthesized summary of the current state of affairs...]

## Situation Summary
[Current events and escalations...]

## Historical Context
[Precedents and base rates...]
```

#### 3. `dossiers/[dossier-id]/forecasts.md` (Markdown)
Tracks the evolution of the analyst's predictions and logs alternative AI estimates.
```markdown
# Forecasts and Calibration Ledger

## Active Question: Will Country X conduct a military intervention before December 31, 2027?

### Analyst Estimation Ledger
| Date | Probability | Confidence | Rationale |
| :--- | :--- | :--- | :--- |
| 2026-06-19 | 45% | Medium | Recent diplomatic shifts indicate posturing rather than immediate operational prep. |
| 2026-03-10 | 60% | Low | Initial troop movements near the border suggested high-risk maneuvers. |

### Optional AI Benchmark
<details>
<summary>Click to view AI Probability Estimate</summary>

* **AI Estimated Probability**: 52% (As of 2026-06-19)
* **Causal Drivers**:
  * Logistic constraints suggest readiness won't peak until early 2027.
  * Deterrence thresholds have remained stable over the last quarter.
</details>
```

---

## 4. Key Workflows

### 4.1 Integration & Auth
1. On first launch, the application prompts the user for their **GitHub Username**, **Repository Name**, and **GitHub Personal Access Token (PAT)**.
2. The credentials are encrypted and saved locally in a secure file (e.g., `.env` or system keychain, adhering to local environment security).
3. The Axum backend uses the PAT to authenticate all requests using the `@octokit/rest` equivalent REST calls in Rust (or direct structured HTTP calls via `reqwest`).

### 4.2 Autonomous Research & Update Loop (The PR Flow)

When the user initiates a research cycle for a specific topic, the system triggers the Python agent:

```
[User triggers Research]
          │
          ▼
[Axum spawns Python Subprocess] (using `uv run`)
          │
          ▼
[Python Agent Queries Free-Tier APIs] (Tavily, Wikipedia, ArXiv, Semantic Scholar)
          │
          ▼
[Agent pulls Main Branch from GitHub]
          │
          ▼
[Agent performs LLM synthesis & edits Dossier Markdown Files]
          │
          ▼
[Agent pushes modifications to remote branch `ai-update/[topic]`]
          │
          ▼
[Agent creates GitHub Pull Request (PR)]
          │
          ▼
[Axum notifies User of pending PR for Review]
```

#### Python Agent Research Execution Code Pattern
```python
# python/src/agent.py
import os
import sys
import polars as pl
import orjson
from typing import Dict, Any

def run_research(topic_id: str, query: str) -> None:
    """Execute background intelligence gathering and format results.
    
    Args:
        topic_id: Unique identifier of the target dossier.
        query: Specific research query derived from dossier goals.
    """
    # 1. Fetch search data (Using Tavily, Semantic Scholar, etc.)
    # 2. Format with Polars
    # 3. Read existing files from local checkout
    # 4. Synthesize with LLM
    # 5. Push to git branch and open PR via GitHub API
    pass
```

### 4.3 Git-Native Conflict Resolution
When an AI-generated branch overlaps with manual changes made by the analyst on `main`:
1. The GitHub PR will indicate a merge conflict status.
2. The **Visual PR Reviewer** screen pulls the conflicting files from the remote branch.
3. The UI highlights standard Git conflict markers:
   ```markdown
   <<<<<<< HEAD
   The analyst believes the troop maneuvers are purely political.
   =======
   The system detected logistical supply shipments supporting troop build-up.
   >>>>>>> ai-update/maritime-security
   ```
4. The user edits the conflict block directly within the split-pane markdown viewer and submits the clean resolution.
5. The backend commits the resolved file back to the branch and merges the PR into `main`.

### 4.4 Chat Session Integration
To avoid structural retrieval failures, the chat window utilizes the entire dossier.
* **Context Construction**: On each chat interaction, the backend reads all 7 Markdown files within the dossier subdirectory, appends `.workbench/profile.json` and `.workbench/analyst_style.md`, and serializes them into a single prompt payload.
* **Context Payload Structure**:
  ```markdown
  System: You are an elite intelligence partner. The user is a senior analyst. 
  The following files represent the complete, current Dossier state:
  
  === FILE: overview.md ===
  [content]
  
  === FILE: causal_models.md ===
  [content]
  ...
  === ANALYST PROFILE ===
  [content of profile.json]
  
  Instructions: Do not repeat facts already explicit in the files. Analyze disagreements, expose hidden assumptions, and evaluate alternative hypotheses.
  ```

---

## 5. Frontend UI Specifications

The UI utilizes a modern responsive grid built exclusively using customized Pico CSS and vanilla JS. It features a theme toggle (Light/Dark) in the top-right header.

### 5.1 Dashboard Screen
* **Overview**: Displayed as a responsive grid card deck. 
* **Details**: Lists active dossiers. Displays the category, confidence score, status, and last-updated date.
* **Actions**: A prominent "New Dossier" button prompts the user for a title and initial question, spinning up the first background research process.

### 5.2 Split-Pane Dossier Workspace
* **Layout**: `grid-template-columns: 2fr 1fr;` at desktop scale.
* **Left Pane**: Contains tabbed navigation mapping directly to the Markdown files:
  * `Overview` | `Causal Models` | `Timeline` | `Entities` | `Evidence` | `Sources` | `Forecasts`
  * Each tab renders the compiled Markdown in a highly legible layout using custom fonts (e.g., modern sans-serif headers and serif body text).
* **Right Pane**: A persistent vertical **Chat Sidebar** containing a message history window and a query input box.

### 5.3 Visual PR Reviewer Screen
* **Diff Layout**: Side-by-side or unified line-by-line diff.
* **Conflict Handlers**: Highlighting git conflict bounds with distinct colors (red for local `HEAD`, green for remote AI branch changes).
* **Action Footer**: Contains "Approve & Merge", "Save Manual Edits", and "Discard PR" buttons.

---

## 6. Error Handling Strategy

### 6.1 Rust Backend Error Propagation
All Rust endpoint actions must catch and cleanly represent upstream failures (such as GitHub API rate limiting or bad credentials) without crashing the runtime.

```rust
// src/errors.rs
use axum::{
    response::{IntoResponse, Response},
    http::StatusCode,
    Json,
};
use thiserror::Error;
use serde_json::json;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("GitHub API error: {0}")]
    GitHubError(#[from] reqwest::Error),
    
    #[error("File system I/O error: {0}")]
    IoError(#[from] std::io::Error),
    
    #[error("Internal Agent error: {0}")]
    AgentError(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            AppError::GitHubError(ref err) => (StatusCode::BAD_GATEWAY, err.to_string()),
            AppError::IoError(ref err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            AppError::AgentError(ref msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg.clone()),
        };
        
        let body = Json(json!({ "error": error_message }));
        (status, body).into_response()
    }
}
```

### 6.2 Python Exception Logging
Python background tasks must never silently crash. Implement exhaustive try-except clauses around all external API networking operations, utilizing Pythons standard `logging.error` interface:

```python
import logging

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("workbench_agent")

try:
    # Attempt research fetch
    pass
except Exception as e:
    logger.error("Failed executing research loop: %s", str(e), exc_info=True)
    raise SystemExit(1)
```

---

## 7. Testing & Quality Assurance Plan

Developers must adhere to the testing policies outline in `CLAUDE.md`.

### 7.1 Rust Unit Tests
* Implement unit tests in a nested `#[cfg(test)]` block.
* **Mocking**: External GitHub REST queries must be mocked using test utilities (e.g., standard payload mocks) instead of executing actual HTTP traffic during tests.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dossier_parsing() {
        // Arrange
        let mock_markdown = "---\nstatus: Active\nconfidence: Medium\n---\n# Title";
        
        // Act
        let parsed = parse_dossier_frontmatter(mock_markdown).unwrap();
        
        // Assert
        assert_eq!(parsed.status, "Active");
        assert_eq!(parsed.confidence, "Medium");
    }
}
```

### 7.2 Python Integration Tests
* **Testing Tool**: Use `pytest`.
* **Rules**: 
  * Never delete files generated during integration testing.
  * Ensure the test output directory is completely isolated and declared in `.gitignore`.
  * External network requests to LLM providers or research endpoints must be mocked using standard `unittest.mock` configurations to ensure fast, deterministic local builds.

```python
# python/tests/test_agent.py
import pytest
from unittest.mock import patch, MagicMock
from src.agent import run_research

@patch("src.agent.search_api_call")
def test_agent_research_execution(mock_search: MagicMock) -> None:
    # Arrange
    mock_search.return_value = {"results": [{"title": "Test Resource", "snippet": "Data content"}]}
    
    # Act
    # Assert
    # Verify file structures are generated under isolated temporary environments
    pass
```

### 7.3 Frontend Browser & Manual Testing Checklist
* Verify light/dark style sheet switching behaves responsively without structural rendering latency.
* Ensure code remains free of React/jQuery syntax.
* Confirm layout remains legible down to a mobile screen profile (e.g., responsive multi-column layout shifts cleanly into a stacked singular timeline).
