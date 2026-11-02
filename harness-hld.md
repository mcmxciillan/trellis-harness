# High-Level Design: EXO-SUIT v2.0 — Multi-Provider AI Harness & Council Engine

`exo-suit` is an ultra-lean, single-binary Rust daemon and embedded zero-dependency HTML5 Canvas PWA designed to orchestrate, visualize, and adjudicate across multiple AI providers (**Antigravity `agy`**, **Claude**, **OpenAI Codex**, **GitHub Copilot**, **DeepSeek**, and **Kimi/Moonshot**) both at home on desktop and on the go on mobile.

---

## 1. Architectural Philosophy & Constraints
* **Single Self-Contained Rust Binary (`axum` + `tokio` + `notify`)**: Eliminates Python/Node runtime overhead and bundles the entire Web UI in memory (`include_str!`).
* **Ultra-Low Client Footprint (`< 15 KB` Vanilla JS + Canvas2D PWA)**: Eliminates multi-megabyte WASM linear memory and VDOM overhead while supporting native mobile gestures, pinch-to-zoom, and installable PWA standalone mode.
* **0% Idle CPU**: The `<canvas>` renderer runs on demand (`requestAnimationFrame` only while nodes are actively executing/negotiating or being dragged) and pauses automatically via `contentvisibilityautostatechange` (`content-visibility: auto`).
* **Multi-Provider & 3-Stage Council Consensus**: Run providers independently or convene an **AI Council** that drafts parallel proposals, cross-critiques peers, and casts **Ranked-Choice Borda Count Ballots (with zero self-voting)** to synthesize the winning output.

---

## 2. System Topology

```
+-----------------------------------------------------------------------+
| HOME / HOST MACHINE (macOS / Linux)                                   |
|                                                                       |
|  `exo-suit` Rust Binary (0.0.0.0:9000)                                |
|  ├── `src/brain.rs`     : Real-time `notify` watcher on               |
|  │                        ~/.gemini/antigravity-cli/brain             |
|  │                        Recursive subagent DAG builder              |
|  ├── `src/providers.rs` : Multi-Provider Adapter                      |
|  │                        (Live CLIs: `agy`, `claude`, `codex`, `gh`  |
|  │                         + Direct REST APIs: Anthropic, OpenAI,     |
|  │                           DeepSeek, Kimi/Moonshot)                 |
|  ├── `src/council.rs`   : 3-Stage Council Negotiation & Borda Voting  |
|  └── `src/main.rs`      : `axum` WebSocket multiplex + Embedded PWA   |
+-----------------------------------------------------------------------+
                                   │
                    ws://<host>:9000/ws (Local or Tailscale)
                                   ▼
+----------------------------------+------------------------------------+
| DESKTOP BROWSER (At Home)        | MOBILE COMPANION PWA (On the Go)   |
| • Retina HTML5 Canvas2D DAG      | • Standalone Home-Screen App       |
| • Docked 5-Tab Inspector Drawer  | • Pinch-to-Zoom Touch Graph        |
| • Live Token & Tool Telemetry    | • Swipeable Bottom-Sheet Drawer    |
+----------------------------------+------------------------------------+
```

---

## 3. 3-Stage AI Council Protocol (`src/council.rs`)

1. **Stage 1 — Parallel Independent Proposals (`STAGE_1_PROPOSALS`)**:
   - The prompt is dispatched concurrently (`tokio::spawn`) to all selected Council providers.
   - Each provider generates its initial architectural/code proposal with latency and execution mode telemetry.
2. **Stage 2 — Cross-Critique & Negotiation (`STAGE_2_NEGOTIATION`)**:
   - Council members inspect peer proposals, issue adversarial critiques, and refine their own drafts (`revised_draft`).
   - Animated dashed peer-negotiation arcs render across the canvas graph.
3. **Stage 3 — Ranked-Choice Borda Ballot & Arbiter Synthesis (`STAGE_3_VOTING` ➔ `COMPLETED`)**:
   - Each AI casts a structured ballot ranking its **1st choice (3.0 Borda pts)** and **2nd choice (1.5 Borda pts)** weighted by confidence.
   - **Strict No-Self-Voting Invariant**: No provider can vote for its own proposal.
   - The **Consensus Arbiter** node crowns the highest-scoring provider and synthesizes the winning solution with peer critique highlights.
