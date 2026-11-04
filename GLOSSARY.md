# Project Glossary & Shared Vocabulary (`exo-suit` / Trellis)

Canonical language for humans and AI agents working in the **Trellis (`exo-suit`)** repository. Use the bolded canonical terms in code, specifications, prompts, and documentation; retire the listed `_Avoid_` synonyms.

---

## 1. Domain Ubiquitous Language

**Workspace Project (`project_id`)**:
A sandboxed software repository directory located under `~/code/<project_id>` that serves as the execution target for agent turns, Git diff inspection, and polyglot test runs.
- _Canonical Type / Schema_: [`src/models.rs`](src/models.rs) (`ExecutionScope::project_id`, `WorkspaceDevOpsSnapshot::project_id`)
- _Avoid_: Repo slug, Tenant, Client folder, Target app, Workspace name

**Session (`session_id`)**:
An isolated, persistent conversation and telemetry timeline stored under `~/.gemini/antigravity-cli/brain/<session_id>` and bound deterministically to a single **Workspace Project** via `.session_project.txt`.
- _Canonical Type / Schema_: [`src/models.rs`](src/models.rs) (`SessionItem`, `SessionProvisionResult`)
- _Avoid_: Thread, Conversation folder, Chat room, Brain instance, Workspace session

**Execution Scope (`ExecutionScope`)**:
The resolved `(session_id, project_id, workspace_path)` triad computed before any CLI subprocess, HTTP provider request, or Council session runs, following a strict 4-tier priority chain (`Explicit Override` → `.session_project.txt` → `Transcript Path Inference` → `Active Project`).
- _Canonical Type / Schema_: [`src/models.rs`](src/models.rs) (`ExecutionScope`), [`src/brain.rs`](src/brain.rs) (`resolve_execution_scope`)
- _Avoid_: Run context, Target env, Session target, Active directory

**Agent Node (`AgentNode`)**:
A vertex in the live espalier Directed Acyclic Graph (DAG) representing a root coding agent (`agent`), spawned worker (`subagent`), background shell/test process (`task`), or System-1 evaluation (`gate`).
- _Canonical Type / Schema_: [`src/models.rs`](src/models.rs) (`AgentNode`)
- _Avoid_: Graph box, Capsule widget, Worker item, Process card, Subagent bubble

**Provider (`ProviderInfo`)**:
One of the 23 registered AI coding agents or model endpoints categorized as either `DECISION_SYSTEM_1` (`jev`) or `GENERATIVE_SYSTEM_2` (22 CLI and API coding agents) with runtime availability mode (`LIVE_CLI`, `LIVE_API`, `DETERMINISTIC_GATE`, or `SIMULATED`).
- _Canonical Type / Schema_: [`src/models.rs`](src/models.rs) (`ProviderInfo`), [`src/providers.rs`](src/providers.rs) (`build_provider_registry`)
- _Avoid_: LLM vendor, Backend model, Engine plugin, AI adapter

**Jev Decision Gate (`DecisionEvaluation`)**:
The sub-200ms TypeSafe AI System-1 evaluation that computes three typed **Decision Primitives**—`noul` (regression risk probability `[0.0, 1.0]`), `choice` (`ALLOW_EXECUTION | REQUIRE_APPROVAL | FAIL_CLOSED_BLOCK`), and `score` (rubric alignment)—with NaN-safe fail-closed clamping.
- _Canonical Type / Schema_: [`src/models.rs`](src/models.rs) (`DecisionEvaluation`, `DecisionPrimitive`), [`src/providers.rs`](src/providers.rs) (`evaluate_jev_decision_gate`)
- _Avoid_: Safety filter, Risk check, Pre-prompt hook, Guardrail score

**Council Session (`CouncilSession`)**:
A 3-stage multi-agent deliberation (`STAGE_1_PROPOSALS` → `STAGE_2_NEGOTIATION` → `STAGE_3_VOTING` → `COMPLETED`) that combines peer Ranked-Choice Borda Count ballots (`3.0` pts for 1st choice, `1.5` pts for 2nd choice, with strict zero self-voting) and a **Jev Decision Gate** bonus to elect and synthesize a winning solution.
- _Canonical Type / Schema_: [`src/models.rs`](src/models.rs) (`CouncilSession`, `CouncilProposal`, `RankedChoiceVote`, `CouncilScoreEntry`)
- _Avoid_: Debate mode, Swarm vote, Committee run, Multi-LLM tournament

**Interactive Prompt (`InteractivePrompt`)**:
A structured single- or multi-choice clarifying question emitted by an agent's `ask_question` tool call (`Pending` or `Answered`), reconciled across both live streaming events and persisted `transcript.jsonl` logs.
- _Canonical Type / Schema_: [`src/models.rs`](src/models.rs) (`InteractivePrompt`, `InteractiveQuestionItem`), [`src/brain.rs`](src/brain.rs) (`reconcile_interactive_prompt`)
- _Avoid_: Clarifying popup, User quiz, Tool question, Inline form

**Active Run Handle (`ActiveRunHandle`)**:
An in-memory cancellation token (`watch::Sender<bool>`) and child `AgentNode` tracker bound to an in-flight independent or Council execution, enabling instant mid-flight termination via `CANCEL_EXECUTION`.
- _Canonical Type / Schema_: [`src/council.rs`](src/council.rs) (`ActiveRunHandle`)
- _Avoid_: Job token, Process handle, Task killer, Running turn

**Workspace DevOps Snapshot (`WorkspaceDevOpsSnapshot`)**:
The composite read projection combining a **Workspace Project**'s live Git working tree (`GitStatusSnapshot`), polyglot Anchor Test Gate status (`TestRunSnapshot`), and CI/endpoint release readiness (`DeployHealthSnapshot`).
- _Canonical Type / Schema_: [`src/models.rs`](src/models.rs) (`WorkspaceDevOpsSnapshot`, `GitStatusSnapshot`, `TestRunSnapshot`, `DeployHealthSnapshot`)
- _Avoid_: Cockpit state, Git summary, Repo health object, DevOps payload

---

## 2. Deep Module, Subsystem & Seam Catalog (5-Tier Seam Taxonomy)

Every module in `exo-suit` maps to one of the **5-Tier Deep Seam Roles** defined in [`docs/specs/ARCHITECTURE-SPEC.md`](docs/specs/ARCHITECTURE-SPEC.md):

| Canonical Module Name | 5-Tier Seam Role | Public Interface / Barrel | Responsibility & Invariants | _Avoid_ Calling It |
| :--- | :--- | :--- | :--- | :--- |
| **`HttpAndWsDeliveryRouter`** | Thin Delivery Adapter (Tier 1) | [`src/lib.rs`](src/lib.rs) (`build_router`, `dispatch_client_command`, `generate_snapshot_json`) | Serves embedded PWA assets with `no-store` cache headers, streams WebSocket frames, and routes 16 typed `ClientCommand` variants without holding locks across disk or subprocess I/O. | "Web server", "Main controller", "Socket manager" |
| **`WorkspaceSandboxGuard`** | Declarative Policy Guard (Tier 2) | [`src/brain.rs`](src/brain.rs) (`resolve_sandboxed_file_path`, `validate_project_slug`), [`src/devops.rs`](src/devops.rs) (`validate_workspace_sandbox`) | Fail-closed path canonicalization and symlink containment ensuring file reads, diffs, and provisioning never escape `~/code/<project_id>` or `~/.gemini/antigravity-cli/brain/<session_id>`. | "Path helper", "File sanitizer", "Security util" |
| **`SessionTranscriptRepository`** | Deep Domain Module (Tier 3 / Pillar 2) | [`src/brain.rs`](src/brain.rs) (`get_session_chat_thread`, `build_live_session_graph`, `provision_workspace_and_session`, `append_turn_to_session_transcript`) | Single-pass JSONL transcript projection, active subagent/task pruning, `ask_question` answer reconciliation, atomic `.tmp` + `rename` metadata persistence, and 19-directory skill discovery. | "Brain service", "Log parser", "Session helper" |
| **`SharedAppStateRepository`** | Deep Domain Module (Tier 3 / Pillar 2) | [`src/council.rs`](src/council.rs) (`SharedAppState`, `resolve_and_bind_scope`, `update_scoped_chat_message`) | Synchronizes `chat_thread`, `active_session_live_messages`, and `session_custom_messages` with zero cross-session bleed; manages `ActiveRunHandle` lifecycles and session-scoped WebSocket broadcasts. | "Global state", "Memory cache", "Chat store" |
| **`CouncilOrchestrator`** | Deep Domain Module (Tier 3 / Pillar 2) | [`src/council.rs`](src/council.rs) (`run_independent_prompt`, `run_council_session`, `submit_interactive_answer`) | Executes independent agent turns and 3-Stage Borda Council deliberation (`Proposals` → `Negotiation` → `Ranked-Choice Voting` + `Jev Gate Bonus`) with strict zero self-voting. | "Council service", "Debate runner", "Prompt handler" |
| **`UniversalProviderGateway`** | Anti-Corruption Gateway (Tier 4 / Pillar 3) | [`src/providers.rs`](src/providers.rs) (`build_provider_registry`, `execute_provider_prompt`, `run_agy_streaming_turn`, `run_muse_streaming_turn`, `parse_agy_stream_json_line`, `evaluate_jev_decision_gate`) | Isolates 23 CLI/API coding agents and the TypeSafe System-1 Jev Gate behind a single streaming and HTTP adapter; normalizes 7 JSONL wire dialects into `CliStreamEvent` (`AgyParsedEvent`) and caches binary lookups via `OnceLock`. | "LLM helper", "API wrapper", "CLI runner" |
| **`WorkspaceDevOpsGateway`** | Anti-Corruption Gateway (Tier 4 / Pillar 3) | [`src/devops.rs`](src/devops.rs) (`probe_git_status`, `compute_workspace_diff`, `run_workspace_tests`, `probe_deploy_and_health`, `build_workspace_devops_snapshot`) | Wraps `git`, `gh`, and polyglot test runners (`cargo`, `npm`, `pytest`, `go`) behind deterministic porcelain/numstat parsers, 15s TTL deploy caching, and live `TASK` node telemetry. | "Git util", "Test helper", "CI checker" |
| **`WorkbenchClientController`** | Headless State Model & Presentation Guard (Tier 5 / Pillar 5) | [`web/app.js`](web/app.js) (`state`, `setActiveMainOverlay`, `dispatchUserPrompt`, `renderMarkdown`, `drawCanvasFrame`) | Manages client-side session state, outbound WebSocket command queuing during reconnects, mutual-exclusion main overlays (`skills`, `files`, `devops`), URL scheme sanitization, and on-demand HTML5 Canvas2D DAG rendering. | "Frontend script", "UI component", "Canvas helper" |

---

## 3. Semantic Rename Catalog (Completed)

The following semantic renames were executed across the codebase to eliminate vocabulary drift and align with **Section 1** and **Section 2**:

| Kind | Retired Name | Canonical Name (`file:line`) | Aligned Term / Seam | Status |
| :--- | :--- | :--- | :--- | :--- |
| **Type Alias** | `DeployHealthSummary` | [`DeployHealthSnapshot`](src/models.rs#L360) ([`src/devops.rs:26`](src/devops.rs#L26)) | `WorkspaceDevOpsSnapshot` (Pillar 1) | ✅ Completed |
| **Enum** | `AgyParsedEvent` | [`CliStreamEvent`](src/providers.rs#L326) *(with `AgyParsedEvent` alias)* | `UniversalProviderGateway` (Tier 4) | ✅ Completed |
| **Enum** | `AgyStreamOutcome` | [`CliStreamOutcome`](src/providers.rs#L104) *(with `AgyStreamOutcome` alias)* | `UniversalProviderGateway` (Tier 4) | ✅ Completed |
| **Function** | `sanitize_code_path` | [`resolve_workspace_browse_path`](src/brain.rs#L81) | `WorkspaceSandboxGuard` (Tier 2) | ✅ Completed |
| **Struct Field** | `SharedAppState::custom_chat_messages` | [`active_session_live_messages`](src/council.rs#L65) | `SharedAppStateRepository` (Tier 3) | ✅ Completed |
| **Doc File** | `harness-hld.md` *(stale root HLD)* | [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Core Documentation Hierarchy | ✅ Completed |
