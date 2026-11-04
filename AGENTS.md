# AGENTS.md — Trellis (`exo-suit`) Agent Context & Seam Rules

Authoritative, always-loaded instructions for AI coding agents working in this repository.

## 1. Canonical Vocabulary & Architecture Pointers
- **Shared Vocabulary (Mandatory)**: Read [`GLOSSARY.md`](GLOSSARY.md) before creating or renaming types, functions, WebSocket commands, or documentation. Always use the canonical domain terms (**Workspace Project**, **Session**, **Execution Scope**, **Agent Node**, **Provider**, **Jev Decision Gate**, **Council Session**, **Interactive Prompt**, **Active Run Handle**, **Workspace DevOps Snapshot**) and retire all listed `_Avoid_` synonyms.
- **System Architecture & Specs**:
  - 5-Tier Seam Blueprint & Spec Index: [`docs/specs/ARCHITECTURE-SPEC.md`](docs/specs/ARCHITECTURE-SPEC.md)
  - Deep Reference Diagrams & Wire Protocols: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
  - 23-Agent CLI/API Ecosystem Matrix: [`docs/research/coding-agents-ecosystem.md`](docs/research/coding-agents-ecosystem.md)
  - Meta Muse Code (`muse` MSP v1) Specification: [`docs/research/muse-cli.md`](docs/research/muse-cli.md)
  - Visual & Botanical Brand Identity: [`BRAND_GUIDELINES.md`](BRAND_GUIDELINES.md)

## 2. Non-Negotiable 5-Tier Architectural Invariants (The 8 Pillars)
1. **Tier 1 — Thin Delivery Adapters ([`src/lib.rs`](src/lib.rs))**:
   - HTTP handlers and `dispatch_client_command` arms must stay thin (<30 lines) and delegate to Tier 2–4 modules.
   - **Never hold `SharedAppState` `RwLock` guards across blocking disk I/O, `git`/`gh` subprocesses, or HTTP/CLI provider calls.** Snapshot minimal state inside a short block, execute I/O outside the lock, and re-acquire a write lock only to commit results.
2. **Tier 2 — Declarative Sandbox Guards ([`src/brain.rs`](src/brain.rs), [`src/devops.rs`](src/devops.rs))**:
   - Every file read (`read_bounded_file`, `/raw-file`), directory browse, git diff, and project provisioner must pass through `resolve_sandboxed_file_path`, `validate_workspace_sandbox`, or `validate_project_slug`.
   - Reject `..`, null bytes, and symlinks resolving outside canonicalized `$HOME/code/<project_id>` or `$HOME/.gemini/antigravity-cli/brain/<session_id>`.
3. **Tier 3 — Zero Cross-Session Bleed ([`src/council.rs`](src/council.rs))**:
   - Always resolve execution targets via `SharedAppState::resolve_and_bind_scope`.
   - Mutate live chat messages exclusively through `SharedAppState::update_scoped_chat_message` and broadcast via `broadcast_chat_stream`, `broadcast_agents_updated`, or `broadcast_council_updated` so background runs in one `Session` never bleed into another active `Session`.
   - Preserve the **Zero Self-Voting Invariant** in `run_council_session` Stage 3 Borda ballots (`voter_provider != first_choice && voter_provider != second_choice`).
4. **Tier 4 — Anti-Corruption Gateways ([`src/providers.rs`](src/providers.rs), [`src/devops.rs`](src/devops.rs))**:
   - All headless CLI spawns must set `.stdin(Stdio::null())` and `.kill_on_drop(true)` and wire into an `ActiveRunHandle` abort channel (`watch::Receiver<bool>`) so unauthenticated CLIs never hang the daemon on stdin prompts and `CANCEL_EXECUTION` terminates subprocesses immediately.
   - Clamp all `DecisionEvaluation` probabilities via `sanitize_prob(val, default)` to prevent `NaN`/`Infinity` from poisoning JSON serialization.
5. **Tier 5 — Single-Binary Embedded Web Assets ([`web/app.js`](web/app.js), [`web/index.html`](web/index.html), [`web/styles.css`](web/styles.css))**:
   - [`src/lib.rs`](src/lib.rs) embeds `web/*` at compile time via `include_str!`. Whenever you edit `web/app.js`, `web/styles.css`, or `web/index.html`, you **must** run `./build.sh` (or `cargo build --release`) and restart `./target/release/exo-suit` for changes to take effect on port `9000`.
   - Sanitize all dynamic markdown links via `sanitizeHrefUrl` and attribute values via `escapeAttr`.

## 3. Verification & Build Commands
```bash
# Run all 43 unit and 5-seam integration tests (Mandatory before any commit/release)
cargo test

# Build optimized single-binary release executable (./target/release/exo-suit)
./build.sh

# Validate embedded JavaScript syntax
node --check web/app.js
```
