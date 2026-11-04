# Meta Muse Code (`muse` CLI) & Meta Model API — Primary-Source Research

**Primary Sources:**
- [Meta Developer Platform Overview](https://dev.meta.ai/docs/overview)
- [Meta Quickstart](https://dev.meta.ai/docs/quickstart)
- [Meta Models Catalog](https://dev.meta.ai/docs/models)
- [Meta Authentication](https://dev.meta.ai/docs/authentication)
- [Meta Pricing & Rate Limits](https://dev.meta.ai/docs/pricing-rate-limits)
- [Muse Code (`muse` CLI) Overview](https://dev.meta.ai/docs/muse-code)
- [Muse Code — Authentication & Billing](https://dev.meta.ai/docs/muse-code/auth)
- [Muse Code — Extending (`AGENTS.md`, Skills, MCP, Hooks, Agents)](https://dev.meta.ai/docs/muse-code/extending)
- [Muse Code — Configuration & Settings](https://dev.meta.ai/docs/muse-code/configuration)
- [Muse Code — Session Messaging](https://dev.meta.ai/docs/muse-code/session-messaging)
- [Muse Spark Open](https://dev.meta.ai/docs/muse-spark-open) & [Muse Glimmer](https://dev.meta.ai/docs/muse-glimmer)
- Official Installer (`https://dev.meta.ai/install.sh`) & Local Binary Verification (`/Users/coreymcmillan/.local/bin/muse`, `Muse Code 1.4.14 (1.4.14-R7309.1)`)

---

## 1. Executive Summary

**Muse Code** (`muse` CLI) is Meta's terminal and CI coding agent built for **Muse Spark** (`muse-spark-1.3`) and **Muse Wonder** (`muse-wonder-1.0`), powered by the **Meta Model API** (`https://api.meta.ai/v1`). It combines an interactive terminal TUI (`muse`), a non-interactive headless runner (`muse exec`) with structured **Muse Session Protocol (MSP)** JSONL event streaming (`--json`), Git worktree isolation (`-w` / `--worktree`), subagent orchestration, and a 4-observer background reminder architecture (Memory recall, Skill recall, Goal tracking, and Verification).

---

## 2. Installation & Binary Paths

- **Official Product Name:** **Muse Code**
- **Exact CLI Binary Name:** `muse`
- **macOS / Linux Install:**
  ```bash
  curl -fsSL https://dev.meta.ai/install.sh | sh
  ```
- **Windows PowerShell Install:**
  ```powershell
  irm https://dev.meta.ai/install.ps1 | iex
  ```
- **Launcher & Binary Mechanics:**
  - Default install directory: `${MUSE_INSTALL_DIR:-$HOME/.local/bin}`
  - Entrypoint script at `~/.local/bin/muse` manages auto-updates from `https://api.meta.ai/muse-code/channels/muse-stable` (or `muse-canary` via `MUSE_CHANNEL`) and delegates to the versioned native binary `~/.local/bin/muse-bin-<version>` (tracked by `~/.local/bin/.muse-version`).
  - Candidate discovery paths on macOS/Linux:
    1. `~/.local/bin/muse`
    2. `~/.muse/bin/muse`
    3. `/opt/homebrew/bin/muse` / `/usr/local/bin/muse`
    4. `muse` on `$PATH`

---

## 3. Authentication & Credentials

Muse Code and the Meta Model API support three authentication paths (in priority order):

1. **Environment Variables (Headless / CI / Harness Priority):**
   - `META_API_KEY` — Primary environment variable for `muse` CLI headless runs (`META_API_KEY` always takes priority over account OAuth login).
   - `MODEL_API_KEY` — Primary environment variable documented for direct Meta Model API calls (`https://api.meta.ai/v1`).
   - **Key Format:** Keys begin with `LLM_` followed by a numeric ID and secret separated by underscores (e.g. `LLM_1607358788850350_...`).
2. **Stored API Key via Stdin (`muse auth set`):**
   ```bash
   printf '%s\n' "$META_API_KEY" | muse auth set --api-key-stdin
   ```
   Persists credentials to `~/.config/muse/auth.json` (overridable via `MUSE_AUTH_PATH`) without exposing secrets in shell history or process argv.
3. **Interactive Browser Device OAuth (`muse login` / `/login`):**
   Opens `https://auth.meta.com/oauth/device/` to link a Meta account subscription and writes `~/.config/muse/auth.json`.
   > **Harness Safety Invariant:** If neither `META_API_KEY` nor `~/.config/muse/auth.json` is present, invoking `muse exec` with `--provider meta` prompts interactively on stdin/stderr for device OAuth. A headless harness must check credential availability (`META_API_KEY` / `MODEL_API_KEY` or `~/.config/muse/auth.json`) and bind `Stdio::null()` on stdin so non-interactive runs never block waiting for a browser press.

---

## 4. Meta Model Catalog & Capabilities

Base URL: **`https://api.meta.ai/v1`** (supports three first-class wire protocols: OpenAI Chat Completions `POST /v1/chat/completions`, OpenAI Responses `POST /v1/responses`, and Anthropic Messages `POST /v1/messages`).

| Model ID | Tier / License | Context Window | Max Output | Pricing (Standard / Contributor per 1M) | Key Capabilities |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **`muse-spark-1.3`**<br>(`muse-spark-1.3-contributor`) | Frontier (Recommended default for new agentic work) | `1,048,576` (1M) | `131,072` (128K) | Std: `$1.25` in / `$4.25` out<br>Contrib: `$0.10` in / `$0.20` out | Multi-step tool use, browser/computer use, long-horizon coding, reasoning effort `minimal`..`max` (default `high`) |
| **`muse-wonder-1.0`**<br>(CLI alias `muse-wonder`) | Frontier Flagship | `1,048,576` (1M) | `131,072` (128K) | Std: `$4.00` in / `$20.00` out | Complex multi-file software engineering, sub-agent delegation, parallel tool calls, and computer use |
| **`muse-spark-1.2`**<br>(`muse-spark-1.2-contributor`) | Default CLI model | `1,048,576` (1M) | `131,072` (128K) | Std: `$1.25` in / `$4.25` out<br>Contrib: `$0.10` in / `$0.20` out | Out-of-the-box default model in `muse` CLI; full audio input support |
| **`muse-spark-1.1`** | Legacy Muse Spark | `1,048,576` (1M) | `131,072` (128K) | Std: `$1.25` in / `$4.25` out | First-generation Muse Spark release (API only) |
| **`muse-spark-open`**<br>(`meta-models/Muse-Spark-Open-816B-A42B`) | Open-Weight (Muse Model License) | `1,048,576` (1M) | `131,072` (128K) | Self-hosted / 3P provider | 816B total / 42B active sparse MoE multimodal transformer (BF16 & NVFP4 on vLLM / SGLang) |
| **`muse-glimmer`**<br>(`meta-models/Muse-Glimmer-30B`) | Open-Weight (Apache 2.0) | `128,000` (up to 256K) | `32,768` (32K) | Self-hosted / 3P provider | 30B dense multimodal model distilled from Muse Spark for local & edge coding workflows |

---

## 5. CLI Subcommands & Headless Execution (`muse exec`)

```text
Usage: muse [OPTIONS] [PROMPT]
       muse [OPTIONS] <COMMAND>
```

### Core Subcommands
- `muse` — Interactive terminal TUI.
- `muse exec [OPTIONS] [PROMPT]` — Run one prompt non-interactively to completion.
- `muse resume [--last | <session-ref>]` — Resume a previous session.
- `muse serve` / `muse schema` — Run the Muse Session Protocol (MSP) host over stdio or export its JSON Schema / TypeScript definitions (`@muse-code/sdk`).
- `muse skills [list|inspect|enable|disable|validate|install|import]` — Manage skills across `user`, `project`, `built-in`, and `plugin` scopes (`muse skills import --from claude|codex`).
- `muse model-profile show <model> [--effort <tier>]` — Inspect model resolution knobs and effort tiers.
- `muse session-message [list|send]` — Cross-session messaging on the same machine.
- `muse init [--dry-run] [--force]` — Scaffold `AGENTS.md` in the current workspace.

### Headless Execution Flags (`muse exec`)
- `--json` — Emit machine-readable **Muse Session Protocol (MSP)** JSONL events on stdout (`schema_version: 1`).
- `--model <ID>` — Select model ID (e.g., `muse-spark-1.3`, `muse-wonder-1.0`, `muse-spark-1.2`).
- `--reasoning-effort <EFFORT>` (alias `--effort`) — `none | minimal | low | medium | high | xhigh | max | ultra` (default: `high`).
- `--workspace <PATH>` — Root policy-gated workspace tools at `PATH`.
- `--trust-workspace` — Load workspace skills and rules for this run.
- `--approval-mode <MODE>` — `untrusted | on-request | never` (default: `on-request`).
- `--disable-approval` — Disable interactive tool approval prompts for non-interactive harness runs while preserving OS sandboxing.
- `--session-id <UUID>` — Resume or bind a specific session UUID non-interactively.
- `-w, --worktree [<MODE>]` — Session Git worktree (`off | create | existing`; bare `-w` means `create`).

### MSP JSONL Wire Events (`muse exec --json`)
Each stdout line with `--json` is a versioned JSON envelope (`schema_version: 1`):
- `payload_type: "session.run.linked"` — Links `stream.id` (session UUID) and `payload.run_stream.id` (run UUID).
- `payload_type: "task.lifecycle.side_effect_intent"` / `"task.lifecycle.started"` — Announces tool or subagent task execution (`payload.event.operation`).
- `payload_type: "run.output.delta"` — Incremental text delta in `payload.text`.
- `payload_type: "run.usage"` — Token usage summary in `payload.models[]` (`input_tokens`, `cached_tokens`, `output_tokens`, `reasoning_tokens`).
- `payload_type: "run.terminal.completed"` — Final completion payload with `payload.text` and `payload.terminal == "completed"`.

---

## 6. Workspace Instructions, Skills & Configuration Paths

- **Settings File:** `~/.config/muse/settings.json` (requires `"schema_version": 1`).
- **Instruction Hierarchy:** `AGENTS.md` (with `AGENTS.override.md` and fallback filenames `CLAUDE.md`, `.agents/AGENTS.md`, `.claude/CLAUDE.md`) discovered from repo root to current working directory.
- **Skill Discovery Directories:**
  - `~/.config/muse/skills/<skill-id>/SKILL.md` (`$XDG_CONFIG_HOME/muse/skills`)
  - `~/.agents/skills/<skill-id>/SKILL.md`
  - `<repo>/.agents/skills/<skill-id>/SKILL.md`
