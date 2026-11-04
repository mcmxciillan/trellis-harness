<p align="center">
  <img src="./trellis-espalier.jpg" alt="Trellis — AI Harness (Espaliered Pomegranate Logo)" width="680" />
</p>

<h1 align="center">TRELLIS :: AI HARNESS (<code>exo-suit</code>)</h1>

<p align="center">
  <em>Structure for intelligence. Cultivated scaffolding for complex autonomous workflows.</em>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-2021_edition-A8202A?style=flat-square&logo=rust" alt="Rust Edition" />
  <img src="https://img.shields.io/badge/binary-single_executable-2D4A3E?style=flat-square" alt="Single Binary" />
  <img src="https://img.shields.io/badge/tests-30_passed_(5_seams)-2D4A3E?style=flat-square" alt="30 Tests Passing" />
  <img src="https://img.shields.io/badge/frontend-zero_dependency_PWA-7D796F?style=flat-square" alt="Zero-Dependency PWA" />
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-262420?style=flat-square" alt="MIT License" /></a>
</p>

<p align="center">
  <a href="#overview"><strong>Overview</strong></a> •
  <a href="#key-capabilities"><strong>Capabilities</strong></a> •
  <a href="#system-architecture"><strong>Architecture</strong></a> •
  <a href="#multi-provider--3-stage-council-engine"><strong>Council &amp; Jev Gate</strong></a> •
  <a href="#developer-lifecycle-cockpit"><strong>DevOps Cockpit</strong></a> •
  <a href="#quickstart"><strong>Quickstart</strong></a> •
  <a href="#testing-suite"><strong>Testing</strong></a> •
  <a href="./BRAND_GUIDELINES.md"><strong>Brand Guidelines</strong></a>
</p>

---

## Overview

**Trellis (`exo-suit`)** is a single-binary Rust AI harness (`tokio` + `axum` + `notify` + `reqwest`) and zero-dependency HTML5 Canvas2D Progressive Web App engineered to structure, observe, and accelerate multi-model software engineering workflows across desktop and mobile.

Where raw autonomous agents can become opaque or unruly across multiple repositories, Trellis provides an intentional architectural scaffold—binding every session deterministically to its target workspace, visualizing live subagent and background task topologies on an orthogonal espalier DAG canvas, enforcing quantitative **System-1 Decision Gates**, and embedding a full **Developer Lifecycle Cockpit** (Git Diff, Anchor Test Gate, Slash-Command Pipeline, and Release Health Probes) directly in the workbench.

```text
  ┌─ trellis :: ai-harness v0.2.0 (exo-suit)
  │
  ├── [lattice] loading scaffold topology... ok
  ├── [root] binding model context (~/.gemini/antigravity-cli/brain)... connected
  ├── [cockpit] git + polyglot anchor test runner + health probes... armed
  └── [yield] pipeline ready on port 9000 (http://127.0.0.1:9000)
```

---

## Key Capabilities

### 1. Live Espalier DAG Canvas & Transcript Engine
- **Real-Time Telemetry (`notify` + WebSockets):** Watches live agent session transcripts (`transcript.jsonl`) and workspace directories (`~/code/<project>`) in real time with a sub-15 MB RAM footprint.
- **Active Subagent & Task Pruning:** Automatically prunes completed subagent branches into the parent agent's history while keeping actively executing subagents and running/daemon background tasks (`run_command`, dev servers, timers) visible as distinct capsule nodes on the DAG canvas.
- **Interactive Decision Cards (`ask_question`):** Renders structured single- and multi-select architectural questions directly in the chat stream and reconciles historical answers from transcript logs.

### 2. Bidirectional Workspace Isolation & Hard Cancellation
- **Deterministic `Session ↔ Project` Binding:** Every session persists its bound repository in `.session_project.txt` and resolves execution scope through a 4-tier priority chain (`Explicit Override` → `.session_project.txt` → `Transcript Path Inference` → `Active Project`).
- **Zero Cross-Session Bleed:** Concurrent background runs stream strictly into their originating session's message store (`session_custom_messages[session_id]`) and never bleed into other open chats.
- **Hard Execution Cancellation (`⏹ Stop`):** Every CLI subprocess and HTTP provider stream is wired to a `tokio::sync::watch` abort channel and `.kill_on_drop(true)` process guard, enabling instant mid-flight termination from the UI.

### 3. Topbar Session & Greenfield Project Provisioner (`+ New`)
- **Existing Workspace Sessions:** Spin up a clean, isolated session bound to any existing `~/code/<repo>` directory in one click.
- **Greenfield Project Scaffolding:** Provision a brand-new repository under `~/code/<slug>` with strict slug validation, automatic `git init -b main`, initial `README.md` scaffolding, and an optional Day-1 `/greenfield-architecture` kickoff prompt.

### 4. Developer Lifecycle Cockpit
- **Live Git Working Tree & Unified Diff Inspector:** Displays active branch, `ahead`/`behind` counts, staged/unstaged/untracked file badges, `+additions`/`-deletions` numstats, recent commit history, and a syntax-highlighted unified diff viewer.
- **1-Click Polyglot Anchor Test Gate (`▶ Run Tests`):** Auto-detects `Cargo.toml` (`cargo test`), `package.json` (`npm test`), `pyproject.toml`/`pytest.ini` (`pytest`), or `go.mod` (`go test ./...`), spawns a visible `TASK` node on the DAG canvas, and parses pass/fail counts.
- **Lifecycle Pipeline Dock & `/` Slash Autocomplete:** 1-click stage pills above the prompt composer for **1. Plan** (`/greenfield-architecture`), **2. Build** (`/tdd`), **3. Debug** (`/bushog`), **4. Review** (`/survey`), **5. Release** (`/prod`), **6. Monitor** (`/health`), and **7. Tidy** (`/tidy`).
- **Deploy & Health Readiness Probe:** Read-only probe of GitHub PR status, recent Actions CI runs (`gh`), and local `/api/health` endpoint latency.

---

## System Architecture

```mermaid
flowchart LR
    subgraph Browser["Trellis Workbench PWA (web/)"]
        UI["3-Column Botanical UI\n(Sessions • DAG/Chat • Inspector)"]
        Cockpit["DevOps Lifecycle Cockpit\n(Git Diff • Anchor Tests • Health)"]
        Composer["Prompt Composer & Slash Dock\n(Independent • 3-Stage Council)"]
    end

    subgraph Daemon["Single-Binary Rust Daemon (src/)"]
        Router["Axum HTTP & WS Server\n(src/lib.rs, src/main.rs)"]
        Brain["Brain & Transcript Engine\n(src/brain.rs)"]
        Council["Council & Scope Orchestrator\n(src/council.rs)"]
        Providers["Provider & Jev Gate Adapter\n(src/providers.rs)"]
        DevOps["DevOps & Git Cockpit Engine\n(src/devops.rs)"]
    end

    subgraph External["Local Sandbox & Model Providers"]
        FS["~/code/<project> &\n~/.gemini/.../brain/<session>"]
        CLIs["Local CLIs & APIs\n(agy, claude, codex, deepseek, kimi, gh)"]
        Jev["TypeSafe AI System-1 Gate\n(jev-latest: Noul / Choice / Score)"]
    end

    UI <-->|"WebSocket JSON Frames\n+ /raw-file + /api/health"| Router
    Cockpit <--> Router
    Composer <--> Router
    Router --> Brain
    Router --> Council
    Router --> DevOps
    Brain <-->|"notify Watcher +\nSandbox Guards"| FS
    DevOps <-->|"git / cargo / npm /\npytest / go / gh"| FS
    Council --> Providers
    Providers <--> CLIs
    Providers <--> Jev
```

### Module Map

| File | Responsibility |
| :--- | :--- |
| [`src/lib.rs`](./src/lib.rs) | Library crate root exposing HTTP routes (`/`, `/api/health`, `/raw-file`, `/ws`), `generate_snapshot_json`, `dispatch_client_command`, and `spawn_brain_watcher`. |
| [`src/main.rs`](./src/main.rs) | Binary entrypoint initializing `SharedAppState`, binding `0.0.0.0:9000`, and launching the Axum server. |
| [`src/brain.rs`](./src/brain.rs) | JSONL transcript parser, active subagent/task pruning, `ask_question` reconciliation, sandbox path guards, and workspace/session provisioning. |
| [`src/council.rs`](./src/council.rs) | `SharedAppState`, zero-bleed session chat scoping, hard execution cancellation (`ActiveRunHandle`), independent prompt runner, and 3-Stage Borda Council engine. |
| [`src/providers.rs`](./src/providers.rs) | Multi-provider registry (`agy`, `muse`, `claude`, `codex`, `deepseek`, `kimi`, `copilot`, `jev`), CLI/API execution adapters, and NaN-safe Jev System-1 evaluation. |
| [`src/devops.rs`](./src/devops.rs) | Git status/porcelain/numstat parser, bounded unified diff generator, polyglot test runner auto-detector & executor, and CI/health readiness probes. |
| [`src/models.rs`](./src/models.rs) | Serde data contracts for telemetry snapshots, DAG nodes, Council ballots, DevOps snapshots, and WebSocket client commands. |
| [`web/`](./web/) | Embedded zero-dependency HTML5 Canvas2D UI (`index.html`, `styles.css`, `app.js`, `manifest.json`). |

---

## Multi-Provider & 3-Stage Council Engine

Trellis orchestrates **8 AI providers** across two distinct cognitive classes:

| Provider | Model Class | Default Model | Specialty |
| :--- | :--- | :--- | :--- |
| **Jev (`jev-latest`)** | `DECISION_SYSTEM_1` | `jev-latest` | Sub-200ms TypeSafe AI System-1 decision primitives (`Noul` risk probability, `Choice` routing verdict, `Score` rubric compliance) with NaN-safe fail-closed fallback |
| **Antigravity (`agy`)** | `GENERATIVE_SYSTEM_2` | `gemini-4-argon` | Multi-agent graph orchestration & deep repository execution |
| **Meta Muse Code (`muse`)** | `GENERATIVE_SYSTEM_2` | `muse-spark-1.3` | Long-horizon agentic coding, worktree subagents & 1M-context reasoning (`muse-spark-1.3`, `muse-wonder-1.0`, `muse-spark-open`, `muse-glimmer`) |
| **Claude Code** | `GENERATIVE_SYSTEM_2` | `claude-4-sonnet` | Deep module design, boundary invariants & adversarial review |
| **OpenAI Codex** | `GENERATIVE_SYSTEM_2` | `codex-mini-latest` | Low-latency code synthesis & surgical patching |
| **DeepSeek** | `GENERATIVE_SYSTEM_2` | `deepseek-reasoner` | First-principles algorithmic reasoning & math verification |
| **Kimi (Moonshot)** | `GENERATIVE_SYSTEM_2` | `kimi-k2-5` | Long-context cross-repository synthesis |
| **GitHub Copilot** | `GENERATIVE_SYSTEM_2` | `copilot-gpt-4o` | CLI workflow automation & PR/issue integration |

### Execution Modes

1. **⚡ Independent Mode:** Dispatch prompts directly to any individual provider (local CLI or HTTPS API) or run a sub-200ms **Jev System-1 Decision Gate** evaluation (`{noul, choice, score}`).
2. **🏛️ 3-Stage Council Mode:**
   - **Stage 1 — Parallel Proposals:** Selected System-2 Generative AIs draft independent architectural proposals concurrently while the **Jev System-1 Gate** scores each candidate on `Noul` (regression risk), `Choice` (gate action), and `Score` (rubric alignment).
   - **Stage 2 — Cross-Critique & Negotiation:** Peers inspect neighboring proposals, surface trade-offs, and synthesize revised drafts.
   - **Stage 3 — Ranked-Choice Borda Ballot + Jev Gate Bonus:** Every peer casts a 1st-choice (`3.0 pts`) and 2nd-choice (`1.5 pts`) ballot with **strict zero self-voting**, combined with Jev's quantitative rubric bonus to elect and synthesize the winning solution.

---

## Developer Lifecycle Cockpit

The topbar DevOps strip (`#topbar-devops-strip`) and collapsible Cockpit drawer (`#main-devops-overlay`) give operators immediate visibility and control over the bound repository:

- **Git Diff & Working Tree Tab (`🌿 Git`):** Inspect modified, staged, renamed, and untracked files with per-file `+additions`/`-deletions` badges and click any file to view its unified diff (`git diff HEAD` or synthesized untracked file diff).
- **Anchor Test Gate Tab (`🧪 Tests`):** Click `▶ Run Tests` to execute the workspace's test suite as a live `TASK` node on the agent graph and inspect the terminal output excerpt and pass/fail tally.
- **Deploy & Health Tab (`🚀 Health`):** Monitor open pull requests, recent GitHub Actions workflow conclusions, release readiness (`git.is_clean && behind == 0 && !ci_failing`), and `/api/health` endpoint probes.

---

## Quickstart

### Prerequisites

- **Rust Toolchain:** Rust `1.85+` (2024 Edition) via [`rustup`](https://rustup.rs/)
- **Git:** Standard `git` CLI for workspace status, diff inspection, and greenfield repository initialization

### Clone, Build & Run Release Binary

```bash
git clone https://github.com/mcmxciillan/trellis-harness.git
cd trellis-harness
./build.sh
./target/release/exo-suit
```

Open **`http://127.0.0.1:9000`** in any desktop or mobile browser. Because `web/` assets are compiled directly into the Rust binary via `include_str!` / `include_bytes!`, `./target/release/exo-suit` runs as a self-contained executable.

### Optional Provider Environment Variables

Trellis auto-detects installed local CLIs (`agy`, `muse`, `claude`, `codex`, `gh`) and also accepts API keys via environment variables or the in-app **🔑 Keys** modal (held strictly in process memory and never written to disk):

```bash
export JEV_API_KEY="..."        # TypeSafe AI Jev System-1 Gate (https://api.typesafe.ai/v1/evaluate)
export GEMINI_API_KEY="..."     # Google Gemini / Antigravity API
export META_API_KEY="..."       # Meta Muse Code / Meta Model API (https://api.meta.ai/v1)
export ANTHROPIC_API_KEY="..."  # Anthropic Claude API
export OPENAI_API_KEY="..."     # OpenAI / Codex API
export DEEPSEEK_API_KEY="..."   # DeepSeek API
export MOONSHOT_API_KEY="..."   # Kimi / Moonshot API
export GITHUB_TOKEN="..."       # GitHub Copilot API
```

---

## Testing Suite

Trellis includes a **30-test** Rust verification suite spanning in-module unit anchors and 5 black-box integration test binaries in [`tests/`](./tests/):

```bash
# Run the entire unit + integration test suite
cargo test
```

| Test Target | Command | Coverage |
| :--- | :--- | :--- |
| **Library Unit Anchors** | `cargo test --lib` | Core invariants across `brain`, `council`, and `devops` (`8 tests`) |
| **Seam 1 — Brain & Transcript** | `cargo test --test seam_1_brain_and_transcript` | ANSI stripping, `ask_question` parsing, sandbox file resolution, subagent pruning & task capsules (`5 tests`) |
| **Seam 2 — Provisioning & Scope** | `cargo test --test seam_2_provisioning_and_scope` | Slug validation, greenfield/existing repo provisioning, session renaming & scope resolution (`3 tests`) |
| **Seam 3 — Council & Providers** | `cargo test --test seam_3_council_and_providers` | NaN-safe probability clamping, Jev System-1 gate, 3-stage Borda no-self-voting & hard cancellation (`5 tests`) |
| **Seam 4 — DevOps Cockpit** | `cargo test --test seam_4_devops_cockpit` | Sandbox guards, git porcelain/rename/numstat parsing, polyglot test detection & diff synthesis (`5 tests`) |
| **Seam 5 — HTTP & WS Server** | `cargo test --test seam_5_http_and_ws_server` | Live Axum HTTP asset/health routes, `/raw-file` traversal blocking, telemetry snapshot & WS command dispatch (`4 tests`) |

---

## Security & Sandbox Guarantees

- **Strict Path Sandboxing:** All file reads (`OPEN_FILE`, `/raw-file`) and DevOps commands (`validate_workspace_sandbox`, `resolve_sandboxed_file_path`) canonicalize paths and verify they reside inside `$HOME/code/<project>` or `$HOME/.gemini/antigravity-cli/brain/<session>`, rejecting `..` traversal and arbitrary system paths with `404 NOT_FOUND`.
- **In-Memory API Keys:** API keys configured via environment variables or the `SET_API_KEYS` WebSocket command are stored only in `SharedAppState::api_keys` in RAM and are never serialized into telemetry snapshots or written to disk.
- **Subprocess Lifecycle Hygiene:** Every spawned CLI or test runner process uses `.kill_on_drop(true)` and bounded timeouts to prevent orphan background processes.

---

## Brand Palette

| Role | Color Name | Hex Code | Purpose |
| :--- | :--- | :--- | :--- |
| **Primary Base** | Linen / Warm Bone | `#F7F4EB` | Primary background canvas; editorial warm-light theme |
| **Primary Deep** | Espresso Trunk | `#262420` | Primary typography, structural borders, dark-mode trunk |
| **Brand Accent** | Pomegranate Crimson | `#A8202A` | Focal nodes, active execution status, primary CTAs |
| **Structural Green** | Deep Laurel Green | `#2D4A3E` | Canopy foliage, verified yield states, Jev System-1 badges |
| **Neutral Mid** | Weathered Trellis | `#7D796F` | Secondary copy, `0.5px` hairline lattice guides, passive scaffolds |
| **Surface Dark** | Basalt | `#181816` | IDE backgrounds, code blocks, dark-mode root layer |

See [`BRAND_GUIDELINES.md`](./BRAND_GUIDELINES.md) for full logo lockup, typography, and visual language rules, and [`docs/specs/ARCHITECTURE-SPEC.md`](./docs/specs/ARCHITECTURE-SPEC.md) for the 8-Pillar architectural specification.

---

## License

Released under the [MIT License](./LICENSE).

