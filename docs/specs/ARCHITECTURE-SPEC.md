# System Architecture Specification: Workspace & Session Provisioning Engine (US-05..US-07)

## 1. Executive Summary
This specification defines the architecture for the **Topbar "+ New Session / New Project" Provisioning Engine** in the Trellis AI Harness (`exo-suit`). It enables developers to spin up either:
1. A **fresh, isolated conversation session** bound to any existing repository in `~/code/<project_id>`, or
2. A **brand-new greenfield repository** in `~/code/<new-slug>` (bootstrapped with `git init -b main`, `.gitignore`, `README.md`, and an initial baseline commit) paired with a newly provisioned session and an optional 1-click `/greenfield-architecture` kickoff prompt.

---

## 2. 8-Pillar Architectural Blueprint

### Pillar 1: Entities & Cardinality
- **`SessionProvisionResult`**:
  - Fields: `session_id: String`, `project_id: String`, `title: String`, `workspace_path: String`, `created_new_project: bool`, `git_initialized: bool`.
- **Available Projects Catalog (`available_projects: Vec<String>`)**:
  - Broadcast in telemetry snapshots so the `+ New` modal always has an up-to-date list of all repositories in `~/code/*`.

### Pillar 2: Invariants & Lifecycle
1. **Immediate Durable Session Persistence**:
   - Persists `.session_project.txt`, `.session_title.txt`, and `.system_generated/logs/transcript.jsonl` immediately inside `~/.gemini/antigravity-cli/brain/<session_id>/` so the session survives reloads and appears at the top of the session selector before the first user message is sent.
2. **Idempotent Greenfield Git Bootstrap**:
   - Creates `~/code/<slug>`, writes `.gitignore` and `README.md`, runs `git init -b main`, and creates an initial commit so the DevOps Cockpit (`probe_git_status`) immediately reports a clean `main` branch.
3. **Zero Cross-Session Bleed on Switch**:
   - Atomically switches `SharedAppState.active_session_id` and `SharedAppState.active_project`, refreshes the File Browser, Agent Graph, Chat Thread, and DevOps Strip, and optionally dispatches `/greenfield-architecture`.
