<p align="center">
  <img src="./trellis-espalier.jpg" alt="Trellis — AI Harness (Espaliered Pomegranate Logo)" width="680" />
</p>

<h1 align="center">TRELLIS :: AI HARNESS</h1>

<p align="center">
  <em>Structure for intelligence. Cultivated scaffolding for complex autonomous workflows.</em>
</p>

<p align="center">
  <a href="./BRAND_GUIDELINES.md"><strong>Brand Guidelines</strong></a> •
  <a href="#architecture--philosophy"><strong>Architecture</strong></a> •
  <a href="#multi-provider--3-stage-council-engine"><strong>Council Engine & Jev System-1</strong></a> •
  <a href="#quickstart"><strong>Quickstart</strong></a>
</p>

---

## Overview

**Trellis** is a single-binary Rust AI harness (`tokio` + `axum` + `notify` + `reqwest`) and zero-dependency HTML5 Canvas2D Progressive Web App engineered to structure, support, and accelerate multi-model workflows across desktop and mobile.

Where raw AI can be unruly or overbearing, Trellis provides an intentional architectural scaffold—guiding generative peers and quantitative decision gates to bear meaningful weight without collapsing into chaos.

```text
  ┌─ trellis :: ai-harness v1.0.0
  │
  ├── [lattice] loading scaffold topology... ok
  ├── [root] binding model context (~/.gemini/antigravity-cli/brain)... connected
  └── [yield] pipeline ready on port 9000 (http://127.0.0.1:9000)
```

---

## Architecture & Philosophy

### The Espaliered Pomegranate

- **The Espalier Lattice (`src/main.rs`, `src/brain.rs`):** A zero-GC Rust daemon that watches live agent session transcripts (`transcript.jsonl`) and workspace boundaries (`~/code`) in real time, streaming telemetry over WebSockets with a sub-15 MB RAM footprint.
- **The Pomegranate Yield (`src/council.rs`, `src/providers.rs`):** Dense multi-provider intelligence organized into structured tiers—combining **System-2 Generative Peers** with **System-1 Quantitative Decision Gates**.
- **Tactile Botanical Workbench (`web/`):** A collapsible 3-column workbench rendered on a **Linen / Warm Bone (`#F7F4EB`)** surface with `0.5px` **Weathered Trellis (`#7D796F`)** perpendicular hairline lattice guides, orthogonal espalier DAG stems, **Pomegranate Crimson (`#A8202A`)** active execution pulses, and **Deep Laurel Green (`#2D4A3E`)** verified yield indicators.

---

## Multi-Provider & 3-Stage Council Engine

Trellis orchestrates **7 AI providers** across two distinct cognitive classes:

| Provider | Model Class | Default Model | Specialty |
| :--- | :--- | :--- | :--- |
| **Jev (`jev-latest`)** | `DECISION_SYSTEM_1` | `jev-latest` | Sub-200ms TypeSafe AI System-1 decision primitives (`Noul` risk probability, `Choice` routing verdict, `Score` rubric compliance) with NaN-safe fail-closed fallback |
| **Antigravity (`agy`)** | `GENERATIVE_SYSTEM_2` | `gemini-4-argon` | Multi-agent graph orchestration & deep repository execution |
| **Claude Code** | `GENERATIVE_SYSTEM_2` | `claude-4-sonnet` | Deep module design, boundary invariants & adversarial review |
| **OpenAI Codex** | `GENERATIVE_SYSTEM_2` | `codex-mini-latest` | Low-latency code synthesis & surgical patching |
| **DeepSeek** | `GENERATIVE_SYSTEM_2` | `deepseek-reasoner` | First-principles algorithmic reasoning & math verification |
| **Kimi (Moonshot)** | `GENERATIVE_SYSTEM_2` | `kimi-k2-5` | Long-context cross-repository synthesis |
| **GitHub Copilot** | `GENERATIVE_SYSTEM_2` | `copilot-gpt-4o` | CLI workflow automation & PR/issue integration |

### Execution Modes

1. **⚡ Independent Mode:** Dispatch prompts directly to any individual provider (CLI or API) or run a sub-200ms **Jev System-1 Decision Gate** check (`{noul, choice, score}`).
2. **🏛️ 3-Stage Council Mode:**
   - **Stage 1 — Parallel Proposals:** Selected System-2 Generative AIs draft independent architectural proposals concurrently while the **Jev System-1 Gate** evaluates each candidate on `Noul` (regression risk), `Choice` (gate action), and `Score` (rubric alignment).
   - **Stage 2 — Cross-Critique & Negotiation:** Peers inspect neighboring proposals, surface trade-offs, and synthesize revised drafts.
   - **Stage 3 — Ranked-Choice Borda Ballot + Jev Gate Bonus:** Every peer casts a 1st-choice (`3.0 pts`) and 2nd-choice (`1.5 pts`) ballot with **strict zero self-voting**, combined with Jev's quantitative rubric bonus to elect and synthesize the winning solution.

---

## Quickstart

### Build & Run Release Binary

```bash
./build.sh
./target/release/exo-suit
```

Then open **`http://127.0.0.1:9000`** in any desktop or mobile browser.

### Optional Provider Environment Variables

Trellis auto-detects installed local CLIs (`agy`, `claude`, `codex`, `gh`) and also accepts API keys via environment variables or the in-app **🔑 API Keys** modal:

```bash
export JEV_API_KEY="..."        # TypeSafe AI Jev System-1 Gate (https://api.typesafe.ai/v1/evaluate)
export ANTHROPIC_API_KEY="..."  # Claude API
export OPENAI_API_KEY="..."     # OpenAI / Codex API
export DEEPSEEK_API_KEY="..."   # DeepSeek API
export MOONSHOT_API_KEY="..."   # Kimi / Moonshot API
```

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

See [`BRAND_GUIDELINES.md`](./BRAND_GUIDELINES.md) for full logo lockup, typography, and visual language rules.
