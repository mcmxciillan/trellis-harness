# AI Coding Agents & Providers Ecosystem — Technical Architecture & Headless Specification

**Synthesized by Trellis (`exo-suit`) Multi-Agent Research Swarm:**
- **Frontier Vendor CLI Agents:** Anthropic Claude Code (`claude`), OpenAI Codex CLI (`codex`), Google Gemini CLI (`gemini`), Google Antigravity (`agy`), Meta Muse Code (`muse`), GitHub Copilot CLI (`copilot`), Cursor Agent CLI (`cursor-agent`)
- **Open-Source & Multi-Provider Terminal Agents:** OpenCode (`opencode`), Aider (`aider`), Block Goose (`goose`), Charm Crush (`crush`)
- **Commercial & Specialized Coding Agents:** Sourcegraph Amp (`amp`), Factory Droid (`droid`), Qwen Code (`qwen`), Continue CLI (`cn`), Cline CLI (`cline`)
- **Model API, System-1 & Local Inference Providers:** TypeSafe Jev (`jev`), DeepSeek (`deepseek`), Kimi / Moonshot (`kimi`), xAI Grok (`grok`), Mistral Codestral / Devstral & Vibe CLI (`mistral` / `vibe`), Local Ollama (`ollama`), OpenRouter (`openrouter`)

---

## 1. Master Ecosystem Matrix (22 Supported Agents & Providers)

| ID | Agent / Provider Name | Class | Binary Candidates | Headless CLI Invocation | Streaming / JSON Flag | Primary Auth Env Var(s) | Default Model |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **`antigravity`** | Antigravity (`agy`) | `GENERATIVE_SYSTEM_2` | `agy` | `agy -p "<prompt>" --model <m>` | `--output-format stream-json` | `GEMINI_API_KEY`, `GOOGLE_API_KEY` | `gemini-4-argon` |
| **`jev`** | Jev (TypeSafe System-1) | `DECISION_SYSTEM_1` | *(HTTP / Quantized Gate)* | `POST https://api.typesafe.ai/v1/evaluate` | JSON (`noul`, `choice`, `score`) | `JEV_API_KEY`, `TYPESAFE_API_KEY` | `jev-latest` |
| **`gemini`** | Google Gemini CLI (`gemini`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/gemini`, `gemini` | `gemini -p "<prompt>" -m <m> --approval-mode yolo --skip-trust` | `-o stream-json` | `GEMINI_API_KEY`, `GOOGLE_API_KEY` | `gemini-2.5-pro` |
| **`muse`** | Meta Muse Code (`muse`) | `GENERATIVE_SYSTEM_2` | `~/.local/bin/muse`, `~/.muse/bin/muse`, `muse` | `muse exec --model <m> --workspace <dir> --trust-workspace --disable-approval "<prompt>"` | `--json` (MSP v1 JSONL) | `META_API_KEY`, `MODEL_API_KEY` | `muse-spark-1.3` |
| **`claude`** | Anthropic Claude Code (`claude`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/claude`, `claude` | `claude -p "<prompt>" --dangerously-skip-permissions --model <m>` | `--output-format stream-json --verbose` | `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN` | `claude-4-sonnet` |
| **`codex`** | OpenAI Codex CLI (`codex`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/codex`, `codex` | `codex exec --full-auto -m <m> -C <dir> "<prompt>"` | `--json` | `CODEX_API_KEY`, `OPENAI_API_KEY` | `codex-mini-latest` |
| **`copilot`** | GitHub Copilot CLI (`copilot`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/copilot`, `copilot`, `gh` | `copilot -p "<prompt>" --allow-all --trust-folder --no-ask-user --model <m> -C <dir>` | `--output-format json` (JSONL) | `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` | `copilot-gpt-4o` |
| **`cursor`** | Cursor Agent CLI (`cursor-agent`) | `GENERATIVE_SYSTEM_2` | `cursor-agent`, `agent` | `cursor-agent -p "<prompt>" --force --model <m> --workspace <dir>` | `--output-format stream-json --stream-partial-output` | `CURSOR_API_KEY` | `claude-4-sonnet` |
| **`opencode`** | OpenCode (`opencode`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/opencode`, `opencode` | `opencode run "<prompt>" --model <m> --auto` | `--format json` (JSONL) | `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` | `anthropic/claude-sonnet-4-5` |
| **`aider`** | Aider (`aider`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/aider`, `aider` | `aider --message "<prompt>" --yes-always --no-pretty --no-stream --model <m>` | Plain text / diff stdout | `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `DEEPSEEK_API_KEY` | `claude-3-7-sonnet-latest` |
| **`goose`** | Block Goose (`goose`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/goose`, `goose` | `GOOSE_MODE=auto goose run --text "<prompt>" --no-session --model <m>` | `--output-format stream-json` | `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` | `claude-sonnet-4-5` |
| **`amp`** | Sourcegraph Amp (`amp`) | `GENERATIVE_SYSTEM_2` | `amp` | `amp -x "<prompt>" --dangerously-allow-all` | `--stream-json` | `AMP_API_KEY` | `smart` |
| **`droid`** | Factory Droid (`droid`) | `GENERATIVE_SYSTEM_2` | `droid` | `droid exec --auto high -m <m> --cwd <dir> "<prompt>"` | `--output-format stream-json` | `FACTORY_API_KEY` | `claude-sonnet-4-20250514` |
| **`qwen`** | Qwen Code (`qwen`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/qwen`, `qwen` | `qwen -p "<prompt>" --yolo -m <m>` | `--output-format stream-json` | `DASHSCOPE_API_KEY`, `QWEN_CODE_API_KEY`, `OPENAI_API_KEY` | `qwen3-coder-plus` |
| **`continue`** | Continue CLI (`cn`) | `GENERATIVE_SYSTEM_2` | `cn`, `continue` | `cn -p "<prompt>" --allow "*" --model <m> --cwd <dir>` | `--format json` | `CONTINUE_API_KEY` | `claude-3-7-sonnet` |
| **`cline`** | Cline CLI (`cline`) | `GENERATIVE_SYSTEM_2` | `cline` | `cline --auto-approve true -m <m> -c <dir> "<prompt>"` | `--json` (NDJSON) | `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY` | `claude-sonnet-4-20250514` |
| **`crush`** | Charm Crush (`crush`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/crush`, `crush` | `crush run --cwd <dir> --yolo --quiet "<prompt>"` | `--quiet` (clean stdout) | `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` | `claude-sonnet-4-5` |
| **`deepseek`** | DeepSeek R1/V3 | `GENERATIVE_SYSTEM_2` | *(HTTP API)* | `POST https://api.deepseek.com/chat/completions` | OpenAI JSON | `DEEPSEEK_API_KEY` | `deepseek-reasoner` |
| **`kimi`** | Kimi (Moonshot) | `GENERATIVE_SYSTEM_2` | *(HTTP API)* | `POST https://api.moonshot.cn/v1/chat/completions` | OpenAI JSON | `MOONSHOT_API_KEY`, `KIMI_API_KEY` | `kimi-k2-5` |
| **`grok`** | xAI Grok (`grok`) | `GENERATIVE_SYSTEM_2` | *(HTTP API)* | `POST https://api.x.ai/v1/chat/completions` | OpenAI JSON | `XAI_API_KEY` | `grok-code-fast-1` |
| **`mistral`** | Mistral Codestral / Devstral (`vibe`) | `GENERATIVE_SYSTEM_2` | `vibe` | `vibe -p "<prompt>" --auto-approve --output json` or `POST https://api.mistral.ai/v1/chat/completions` | `--output json` / OpenAI JSON | `MISTRAL_API_KEY`, `CODESTRAL_API_KEY` | `codestral-latest` |
| **`ollama`** | Local Ollama (`ollama`) | `GENERATIVE_SYSTEM_2` | `/opt/homebrew/bin/ollama`, `ollama` | `ollama run <model> "<prompt>"` or `POST http://127.0.0.1:11434/v1/chat/completions` | Local CLI / OpenAI JSON | `OLLAMA_HOST` | `qwen3-coder` |
| **`openrouter`** | OpenRouter Gateway | `GENERATIVE_SYSTEM_2` | *(HTTP API)* | `POST https://openrouter.ai/api/v1/chat/completions` | OpenAI JSON | `OPENROUTER_API_KEY` | `anthropic/claude-sonnet-4` |

---

## 2. Unified JSONL Streaming Wire Protocols

Trellis normalizes **7 distinct JSONL streaming dialects** into the single [`AgyParsedEvent`](../../src/providers.rs) state machine (`Init`, `ToolActive`, `ToolDone`, `TextDelta`, `Result`):

1. **Antigravity (`agy --output-format stream-json`)**:
   - `{"event": "init", "conversation_id": "..."}`
   - `{"event": "step_update", "step_update": {"step_type": "tool"|"agent_response", ...}}`
   - `{"event": "result", "response": "...", "usage": {"input_tokens": ..., "output_tokens": ...}}`
2. **Meta Muse Code (`muse exec --json` — MSP v1)**:
   - `{"payload_type": "session.run.linked", "stream": {"id": "..."}}`
   - `{"payload_type": "task.lifecycle.side_effect_intent", "payload": {"event": {"operation": "..."}}}`
   - `{"payload_type": "run.output.delta", "payload": {"text": "..."}}`
   - `{"payload_type": "run.usage", "payload": {"models": [{"input_tokens": ..., "output_tokens": ..., "cached_tokens": ...}]}}`
   - `{"payload_type": "run.terminal.completed", "payload": {"text": "..."}}`
3. **Claude Code / Sourcegraph Amp / Cursor Agent (`--output-format stream-json` / `--stream-json`)**:
   - `{"type": "system", "subtype": "init", "session_id": "..."}`
   - `{"type": "assistant", "message": {"content": [{"type": "text", "text": "..."}, {"type": "tool_use", "name": "...", "input": {...}}]}}`
   - `{"type": "tool_call", "tool": "...", "arguments": {...}}`
   - `{"type": "result", "result": "...", "session_id": "...", "usage": {"input_tokens": ..., "output_tokens": ...}}`
4. **Google Gemini CLI / Qwen Code / Factory Droid (`-o stream-json`)**:
   - `{"type": "init", "session_id": "...", "model": "..."}`
   - `{"type": "message", "role": "assistant", "content": "...", "delta": true}`
   - `{"type": "tool_use", "tool_name": "...", "parameters": {...}}`
   - `{"type": "tool_result", "tool_id": "...", "status": "success"}`
   - `{"type": "result", "status": "success", "response": "...", "stats": {"input_tokens": ..., "output_tokens": ..., "cached": ...}}`
5. **OpenAI Codex CLI (`codex exec --json`)**:
   - `{"type": "thread.started", "thread_id": "..."}`
   - `{"type": "item.started", "item": {"type": "command_execution"|"mcp_tool_call"|"file_change", "command": "..."}}`
   - `{"type": "item.completed", "item": {"type": "agent_message", "text": "..."}}`
   - `{"type": "turn.completed", "usage": {"input_tokens": ..., "cached_input_tokens": ..., "output_tokens": ...}}`
6. **GitHub Copilot CLI (`copilot -p --output-format json`)**:
   - `{"type": "session.start", "data": {"sessionId": "..."}}`
   - `{"type": "tool.execution_start", "data": {"toolName": "...", "arguments": {...}}}`
   - `{"type": "tool.execution_complete", "data": {"toolName": "..."}}`
   - `{"type": "assistant.message", "data": {"content": "..."}}`
   - `{"type": "workflow.result", "data": {"content": "..."}}`
7. **OpenCode (`opencode run --format json`)**:
   - `{"type": "step_start", "sessionID": "..."}`
   - `{"type": "text", "part": {"text": "..."}}`
   - `{"type": "tool_use", "part": {"tool": "...", "state": {"status": "..."}}}`
   - `{"type": "step_finish", "part": {"tokens": {"input": ..., "output": ...}}}`

---

## 3. Cross-Agent Skill & Instruction Discovery Paths

Trellis scans all standard agent skill directories containing `<skill-name>/SKILL.md` so skills are shared across agents in the **Skills & Tools Lattice**:

- `~/.gemini/skills`, `~/.gemini/config/skills`, `~/.gemini/antigravity-cli/skills`, `~/.gemini/antigravity-cli/builtin/skills`
- `~/.agents/skills` (Universal Agent Skills standard used by Gemini CLI, Codex, Muse Code, Copilot, Amp, Goose, Cursor)
- `~/.config/muse/skills` (Meta Muse Code)
- `~/.claude/skills` (Anthropic Claude Code)
- `~/.codex/skills` (OpenAI Codex CLI)
- `~/.copilot/skills` (GitHub Copilot CLI)
- `~/.cursor/skills` (Cursor Agent CLI)
- `~/.config/opencode/skills` (OpenCode)
- `~/.config/goose/skills` (Block Goose)
- `~/.config/amp/skills` (Sourcegraph Amp)
- `~/.factory/skills` (Factory Droid)
- `~/.qwen/skills` (Qwen Code)
- `~/.continue/skills` (Continue CLI)
- `~/.cline/skills` (Cline CLI)
- `~/.config/crush/skills` (Charm Crush)
