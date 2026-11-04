(() => {
  "use strict";

  const PROVIDER_COLORS = {
    antigravity: "#A8202A", // Pomegranate Crimson
    jev: "#2D4A3E",         // Deep Laurel Green
    gemini: "#1A73E8",      // Google Cobalt Blue
    muse: "#0064E0",        // Meta Royal Azure
    claude: "#9E472A",      // Burnt Umber
    codex: "#2D4A3E",       // Deep Laurel Green
    copilot: "#4A6B5D",     // Laurel Slate
    cursor: "#3B4252",      // Nordic Slate
    opencode: "#2E7D6B",    // Verdigris Pine
    aider: "#2A6F4E",       // Forest Emerald
    goose: "#6B4E2E",       // Warm Bronze
    amp: "#D64527",         // Sourcegraph Vermilion
    droid: "#3E4C59",       // Industrial Iron
    qwen: "#5B3FD3",        // Qwen Indigo
    continue: "#3A6073",    // Steel Blue
    cline: "#7A4988",       // Plum Violet
    crush: "#C23B6E",       // Charm Magenta
    deepseek: "#4A6B5D",    // Laurel Slate
    kimi: "#A8202A",        // Pomegranate Crimson
    grok: "#262420",        // Espresso Obsidian
    mistral: "#D96B27",     // Mistral Ochre
    ollama: "#52616B",      // Local Granite
    openrouter: "#4B5D67",  // Gateway Slate
  };

  const state = {
    connected: false,
    ws: null,
    pendingOutboundQueue: [],
    activeProject: "exo-suit",
    activeSessionId: "",
    executionMode: "independent", // "independent" | "council"
    selectedProvider: "antigravity",
    selectedModel: "gemini-4-argon",
    councilParticipants: new Set(["antigravity", "jev", "muse", "claude", "codex", "deepseek", "kimi"]),
    providers: [],
    agents: [],
    selectedAgentId: null,
    council: null,
    sessions: [],
    sessionAliases: (() => {
      try {
        return JSON.parse(localStorage.getItem("trellis.sessionAliases.v1") || "{}");
      } catch {
        return {};
      }
    })(),
    skills: [],
    tools: [],
    skillsToolsSegment: "all", // "all" | "skills" | "tools"
    skillsToolsSource: "all",
    skillsToolsQuery: "",
    artifacts: [],
    chatThread: [],
    fileTree: null,
    openedFile: null,
    activeTab: "chat",
    // Canvas viewport transform
    panX: 0,
    panY: 0,
    zoom: 1.0,
    isDraggingCanvas: false,
    draggingNode: null,
    dragStartX: 0,
    dragStartY: 0,
    pinchStartDist: null,
    pinchStartZoom: 1.0,
    canvasPaused: false,
    animFrameId: null,
    leftDrawerOpen: false,
    rightDrawerOpen: true,
    skillsToolsMainOpen: false,
    fileViewerMainOpen: false,
    fileViewerMode: "formatted",
    openedFileLineAnchor: null,
    devops: null,
    gitDiff: null,
    devopsMainOpen: false,
    devopsActiveTab: "diff",
    selectedDiffFile: null,
    availableProjects: [],
    newSessionMode: "existing_repo",
  };

  // DOM Elements
  const appShell = document.getElementById("app-shell");
  const canvas = document.getElementById("harness_canvas");
  const ctx = canvas.getContext("2d", { alpha: false });
  const graphViewport = document.getElementById("graph-viewport");
  const connBadge = document.getElementById("conn-badge");
  const activeProjectBadge = document.getElementById("active-project-badge");
  const sessionSelect = document.getElementById("session-select");
  const renameSessionBtn = document.getElementById("rename-session-btn");
  const sessionRenameInline = document.getElementById("session-rename-inline");
  const sessionRenameInput = document.getElementById("session-rename-input");
  const sessionRenameSaveBtn = document.getElementById("session-rename-save");
  const sessionRenameCancelBtn = document.getElementById("session-rename-cancel");
  const councilStageBanner = document.getElementById("council-stage-banner");
  const councilBannerText = document.getElementById("council-banner-text");
  const inspectorDrawer = document.getElementById("inspector-drawer");
  const drawerHandle = document.getElementById("drawer-handle");
  const atelierLeftRail = document.getElementById("atelier-left-rail");
  const reopenLeftBtn = document.getElementById("reopen-left-btn");
  const reopenRightBtn = document.getElementById("reopen-right-btn");
  const collapseRightBtn = document.getElementById("collapse-right-btn");

  const modeIndependentBtn = document.getElementById("mode-independent-btn");
  const modeCouncilBtn = document.getElementById("mode-council-btn");
  const councilPillsRow = document.getElementById("council-pills-row");
  const providerSelect = document.getElementById("provider-select");
  const modelSelect = document.getElementById("model-select");
  const promptForm = document.getElementById("prompt-form");
  const promptInput = document.getElementById("prompt-input");
  const submitPromptBtn = document.getElementById("submit-prompt-btn");
  const stopExecutionBtn = document.getElementById("stop-execution-btn");

  const keysDialog = document.getElementById("provider-keys-dialog");
  const openKeysBtn = document.getElementById("open-keys-btn");
  const closeKeysBtn = document.getElementById("close-keys-btn");
  const keysForm = document.getElementById("keys-form");

  function saveSessionAliases() {
    try {
      localStorage.setItem("trellis.sessionAliases.v1", JSON.stringify(state.sessionAliases));
    } catch {
      // ignore storage quota errors
    }
  }

  function bindDialogLightDismiss(dlg) {
    if (!dlg || "closedBy" in HTMLDialogElement.prototype) return;
    dlg.addEventListener("click", (event) => {
      if (event.target !== dlg) return;
      const rect = dlg.getBoundingClientRect();
      const inside =
        rect.top <= event.clientY &&
        event.clientY <= rect.top + rect.height &&
        rect.left <= event.clientX &&
        event.clientX <= rect.left + rect.width;
      if (!inside) dlg.close();
    });
  }

  // =========================================================================
  // 1. Modern Web Guidance: Light-Dismiss Dialog + Content-Visibility Pause
  // =========================================================================
  openKeysBtn.addEventListener("click", () => keysDialog.showModal());
  closeKeysBtn.addEventListener("click", () => keysDialog.close());
  bindDialogLightDismiss(keysDialog);

  keysForm.addEventListener("submit", () => {
    const api_keys = Object.fromEntries(
      Array.from(keysForm.querySelectorAll('input[id^="key-"]'), (el) => [
        el.id.slice(4),
        el.value.trim(),
      ])
    );
    sendCommand({ command: "SET_API_KEYS", api_keys });
  });

  graphViewport.addEventListener("contentvisibilityautostatechange", (event) => {
    if (event.skipped) {
      state.canvasPaused = true;
      if (state.animFrameId) {
        cancelAnimationFrame(state.animFrameId);
        state.animFrameId = null;
      }
    } else {
      state.canvasPaused = false;
      requestDraw();
    }
  });

  if (!("contentVisibility" in document.documentElement.style)) {
    const observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        state.canvasPaused = !entry.isIntersecting;
        if (!state.canvasPaused) requestDraw();
      }
    });
    observer.observe(graphViewport);
  }

  document.addEventListener("visibilitychange", () => {
    state.canvasPaused = document.hidden;
    if (!state.canvasPaused) requestDraw();
  });

  // =========================================================================
  // 2. WebSocket Connection & Telemetry Dispatch
  // =========================================================================
  function connectWebSocket() {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const wsUrl = `${proto}//${location.host}/ws`;
    const ws = new WebSocket(wsUrl);
    state.ws = ws;

    ws.addEventListener("open", () => {
      state.connected = true;
      if (connBadge) {
        connBadge.title = `ONLINE • ${location.host} • Scaffold: ${state.activeProject} • ${state.agents.length} agents`;
        connBadge.classList.remove("disconnected");
      }
      while (state.pendingOutboundQueue.length > 0 && ws.readyState === WebSocket.OPEN) {
        ws.send(state.pendingOutboundQueue.shift());
      }
    });

    ws.addEventListener("close", () => {
      state.connected = false;
      if (connBadge) {
        connBadge.title = "RECONNECTING...";
        connBadge.classList.add("disconnected");
      }
      setTimeout(connectWebSocket, 1500);
    });

    ws.addEventListener("message", (ev) => {
      try {
        const packet = JSON.parse(ev.data);
        handleTelemetryPacket(packet);
      } catch (e) {
        console.error("Failed to parse telemetry packet:", e);
      }
    });
  }

  function sendCommand(payload) {
    if (state.ws && state.ws.readyState === WebSocket.OPEN) {
      state.ws.send(JSON.stringify(payload));
    } else {
      if (state.pendingOutboundQueue.length >= 32) {
        state.pendingOutboundQueue.shift();
      }
      state.pendingOutboundQueue.push(JSON.stringify(payload));
      if (!state.ws || state.ws.readyState === WebSocket.CLOSED) {
        connectWebSocket();
      }
    }
  }

  function syncActiveProjectUI() {
    if (activeProjectBadge) {
      activeProjectBadge.textContent = `📂 ~/code/${state.activeProject}`;
      activeProjectBadge.title = `Active Workspace: ~/code/${state.activeProject} — Click to switch project directory in Files`;
    }
    updatePromptPlaceholder();
  }

  function isSessionRunning() {
    if (state.chatThread.some((m) => m.streaming)) {
      return true;
    }
    if (
      state.council &&
      state.council.stage &&
      state.council.stage !== "COMPLETED" &&
      state.council.stage !== "CANCELLED"
    ) {
      return true;
    }
    const rootId = state.agents[0]?.agent_id;
    if (
      state.agents.some(
        (a) =>
          a.agent_id !== rootId &&
          (a.node_kind === "task" || a.node_kind === "subagent") &&
          a.status === "Running"
      )
    ) {
      return true;
    }
    return false;
  }

  function syncStopButtonState() {
    if (!stopExecutionBtn) return;
    const running = isSessionRunning();
    stopExecutionBtn.classList.toggle("hidden", !running);
  }

  function cancelActiveExecution(runId = null) {
    // Optimistically stop any streaming messages in the local chat thread immediately
    let mutated = false;
    for (const msg of state.chatThread) {
      if (msg.streaming && (!runId || String(msg.id) === String(runId) || String(msg.id).includes(String(runId)))) {
        msg.streaming = false;
        if (Array.isArray(msg.tool_calls)) {
          for (const tc of msg.tool_calls) {
            if (tc.status === "Running") tc.status = "Cancelled";
          }
        }
        if (!msg.content || !msg.content.trim()) {
          msg.content = "*(⏹ Execution stopped by user before output was generated.)*";
        } else if (!msg.content.includes("Stopped by user")) {
          msg.content += "\n\n*(⏹ Stopped by user)*";
        }
        mutated = true;
      }
    }
    if (mutated) {
      renderChatTab(false);
    }
    syncStopButtonState();
    sendCommand({
      command: "CANCEL_EXECUTION",
      session_id: state.activeSessionId,
      run_id: runId || undefined,
    });
  }

  function handleTelemetryPacket(pkt) {
    if (pkt.type === "COMMAND_ERROR") {
      console.warn(
        `[Trellis] Command error (${pkt.command || "UNKNOWN"}):`,
        pkt.error || "Unknown error"
      );
      if (pkt.command === "OPEN_FILE") {
        state.openedFile = {
          name: "Unable to Open File",
          rel_path: pkt.path || "",
          abs_path: pkt.path || "",
          file_kind: "markdown",
          size_bytes: 0,
          content: `> [!WARNING] ${pkt.error || "Requested file could not be opened."}`,
        };
        if (state.fileViewerMainOpen) {
          renderMainFileViewer();
        }
      }
      return;
    }

    if (pkt.type === "CHAT_STREAM_UPDATE" || pkt.type === "AGENT_SPAWNED") {
      const pktSess = pkt.session_id || pkt.active_session_id;
      if (pktSess && state.activeSessionId && pktSess !== state.activeSessionId) {
        return;
      }
    }

    if (pkt.project_id) {
      state.activeProject = pkt.project_id;
      syncActiveProjectUI();
    }

    if (pkt.active_session_id) {
      state.activeSessionId = pkt.active_session_id;
      renderSessionsDropdown();
    }

    if (pkt.providers) {
      state.providers = pkt.providers;
      renderProviderControls();
      renderProvidersTab();
      renderAtelierLeftRail();
    }

    if (pkt.sessions) {
      state.sessions = pkt.sessions;
      renderSessionsDropdown();
    }

    if (pkt.available_projects) {
      state.availableProjects = pkt.available_projects;
      populateNewSessionRepoDropdown();
    }

    if (pkt.type === "SESSION_CREATED") {
      const dlg = document.getElementById("new-session-dialog");
      if (dlg && dlg.open) {
        dlg.close();
      }
      if (pkt.provision && pkt.provision.session_id && pkt.provision.title) {
        state.sessionAliases[pkt.provision.session_id] = pkt.provision.title;
        saveSessionAliases();
      }
      renderSessionsDropdown();
      switchTab("chat");
    }

    if (pkt.type === "SESSION_CREATE_ERROR") {
      const errBanner = document.getElementById("new-session-error");
      if (errBanner) {
        errBanner.textContent = `⚠️ ${pkt.error || "Failed to create session."}`;
        errBanner.classList.remove("hidden");
      }
    }

    if (pkt.skills || pkt.tools) {
      if (pkt.skills) state.skills = pkt.skills;
      if (pkt.tools) state.tools = pkt.tools;
      const totalCap = state.skills.length + state.tools.length;
      const topStBtn = document.getElementById("top-skills-tools-btn");
      if (topStBtn) {
        topStBtn.textContent = `🧰 Skills & Tools (${totalCap})`;
      }
      renderSkillsToolsTab();
      renderAtelierLeftRail();
    }

    if (pkt.artifacts) {
      state.artifacts = pkt.artifacts;
      document.getElementById("tab-btn-artifacts").textContent = `📑 ARTIFACTS (${state.artifacts.length})`;
      renderArtifactsTab();
      renderFilesTab();
    }

    if (pkt.file_browser) {
      state.fileTree = pkt.file_browser;
      renderFilesTab();
    }

    if (pkt.file) {
      state.openedFile = pkt.file;
      renderFilesTab();
      renderMainFileViewer();
      setFileViewerMainOpen(true);
    }

    if (pkt.agents) {
      state.agents = pkt.agents;
      if (connBadge && state.connected) {
        connBadge.title = `ONLINE • ${location.host} • Scaffold: ${state.activeProject} • ${state.agents.length} agents`;
      }
      if (
        !state.selectedAgentId ||
        !state.agents.some((a) => a.agent_id === state.selectedAgentId)
      ) {
        state.selectedAgentId = state.agents[0]?.agent_id || null;
      }
      renderAgentTab();
      renderAtelierLeftRail();
    }

    if (pkt.council !== undefined) {
      state.council = pkt.council;
      renderCouncilBanner();
      renderCouncilTab();
      renderAtelierLeftRail();
    }

    if (pkt.chat_thread) {
      const prevLen = state.chatThread.length;
      state.chatThread = pkt.chat_thread;
      const chatBtn = document.getElementById("tab-btn-chat");
      if (chatBtn) {
        chatBtn.textContent = `💬 CHAT (${state.chatThread.length})`;
      }
      const isStreaming =
        pkt.type === "CHAT_STREAM_UPDATE" ||
        pkt.type === "AGENT_SPAWNED" ||
        state.chatThread.some((m) => m.streaming);
      renderChatTab(state.chatThread.length > prevLen || isStreaming);
      renderInteractiveQuestionBanner();
    }

    if (pkt.devops) {
      state.devops = pkt.devops;
      renderTopbarDevOpsStrip();
      if (state.devopsMainOpen) {
        renderMainDevOpsWorkbench();
      }
    }

    if (pkt.git_diff) {
      state.gitDiff = pkt.git_diff;
      if (state.devopsMainOpen) {
        renderMainDevOpsWorkbench();
      }
    }

    syncStopButtonState();
    requestDraw();
  }

  // =========================================================================
  // 3. Topbar, Collapsible Side Drawers, Tabs & Mode Controls
  // =========================================================================
  if (typeof ResizeObserver !== "undefined" && graphViewport) {
    const viewportResizeObserver = new ResizeObserver(() => {
      resizeCanvas();
    });
    viewportResizeObserver.observe(graphViewport);
  }

  function setLeftDrawerOpen(open) {
    state.leftDrawerOpen = open;
    appShell?.classList.toggle("left-collapsed", !open);
    atelierLeftRail?.classList.toggle("collapsed", !open);
    reopenLeftBtn?.classList.toggle("hidden", open);
    requestAnimationFrame(resizeCanvas);
  }

  function setRightDrawerOpen(open) {
    state.rightDrawerOpen = open;
    appShell?.classList.toggle("right-collapsed", !open);
    inspectorDrawer?.classList.toggle("collapsed", !open);
    reopenRightBtn?.classList.toggle("hidden", open);
    requestAnimationFrame(resizeCanvas);
  }

  reopenLeftBtn?.addEventListener("click", () => setLeftDrawerOpen(true));
  reopenRightBtn?.addEventListener("click", () => setRightDrawerOpen(true));
  collapseRightBtn?.addEventListener("click", () => setRightDrawerOpen(false));

  function setActiveMainOverlay(overlay = null, devopsTab = null) {
    if (devopsTab) state.devopsActiveTab = devopsTab;
    state.skillsToolsMainOpen = overlay === "skills-tools";
    state.fileViewerMainOpen = overlay === "file-viewer";
    state.devopsMainOpen = overlay === "devops";

    document.getElementById("main-skills-tools-overlay")?.classList.toggle("hidden", !state.skillsToolsMainOpen);
    document.getElementById("top-skills-tools-btn")?.classList.toggle("drawer-toggle-active", state.skillsToolsMainOpen);
    document.getElementById("main-file-viewer-overlay")?.classList.toggle("hidden", !state.fileViewerMainOpen);
    document.getElementById("main-devops-overlay")?.classList.toggle("hidden", !state.devopsMainOpen);
    renderTopbarDevOpsStrip();

    if (state.skillsToolsMainOpen) {
      renderSkillsToolsTab();
    } else if (state.fileViewerMainOpen) {
      renderMainFileViewer();
    } else if (state.devopsMainOpen) {
      renderMainDevOpsWorkbench();
      sendCommand({
        command: "OPEN_GIT_DIFF",
        project_id: state.activeProject,
        path: state.selectedDiffFile || undefined,
      });
    } else {
      requestDraw();
    }
  }

  const setSkillsToolsMainOpen = (open) => setActiveMainOverlay(open ? "skills-tools" : null);
  const setFileViewerMainOpen = (open) => setActiveMainOverlay(open ? "file-viewer" : null);
  const setDevOpsMainOpen = (open, tab = null) => setActiveMainOverlay(open ? "devops" : null, tab);

  function openFileOnMainScreen(rawPath, explicitAnchor = "") {
    if (!rawPath) return;
    let cleanPath = String(rawPath).trim().replace(/^file:\/\//, "");
    let anchor = explicitAnchor || "";
    if (cleanPath.includes("#")) {
      const parts = cleanPath.split("#");
      cleanPath = parts[0];
      if (!anchor && parts[1]) {
        anchor = `#${parts[1]}`;
      }
    }
    try {
      cleanPath = decodeURIComponent(cleanPath);
    } catch {
      // keep raw path if not URI-encoded
    }

    state.openedFileLineAnchor = anchor || null;
    state.fileViewerMode = "formatted";

    // Instant optimistic preview if this matches a loaded session artifact
    const baseName = cleanPath.split("/").pop() || cleanPath;
    const baseNoExt = baseName.replace(/\.md$/i, "");
    const matchingArt = (state.artifacts || []).find(
      (a) =>
        a.path === cleanPath ||
        a.name.toLowerCase() === baseName.toLowerCase() ||
        a.name.toLowerCase() === baseNoExt.toLowerCase()
    );
    if (matchingArt && matchingArt.content_preview) {
      const artFileName = matchingArt.path.split("/").pop() || `${matchingArt.name}.md`;
      const isImg = /\.(jpg|jpeg|png|gif|webp|svg)$/i.test(artFileName);
      state.openedFile = {
        name: artFileName,
        rel_path: matchingArt.path,
        abs_path: matchingArt.path,
        file_kind: isImg ? "image" : "markdown",
        size_bytes: matchingArt.content_preview.length,
        content: isImg
          ? `/raw-file?path=${encodeURIComponent(matchingArt.path)}`
          : matchingArt.content_preview,
      };
      setFileViewerMainOpen(true);
      renderFilesTab();
    }

    sendCommand({ command: "OPEN_FILE", path: cleanPath });
  }

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      const slashPop = document.getElementById("slash-cmd-popover");
      if (slashPop && !slashPop.classList.contains("hidden")) {
        slashPop.classList.add("hidden");
        return;
      }
      if (state.devopsMainOpen || state.fileViewerMainOpen) {
        setActiveMainOverlay(null);
        return;
      }
      if (isSessionRunning()) {
        cancelActiveExecution();
      }
    }
  });

  for (const [id, tab] of [
    ["top-git-diff-btn", "diff"],
    ["top-test-gate-btn", "tests"],
    ["top-deploy-health-btn", "deploy"],
  ]) {
    document.getElementById(id)?.addEventListener("click", () => {
      setDevOpsMainOpen(!(state.devopsMainOpen && state.devopsActiveTab === tab), tab);
    });
  }

  activeProjectBadge?.addEventListener("click", () => {
    setRightDrawerOpen(true);
    switchTab("files");
    sendCommand({ command: "BROWSE_DIR", path: "" });
  });

  stopExecutionBtn?.addEventListener("click", () => {
    cancelActiveExecution();
  });

  document.getElementById("top-skills-tools-btn")?.addEventListener("click", () => {
    setSkillsToolsMainOpen(!state.skillsToolsMainOpen);
  });

  function switchTab(tabName) {
    state.activeTab = tabName;
    if (!state.rightDrawerOpen) {
      setRightDrawerOpen(true);
    }
    document.querySelectorAll(".tab-btn").forEach((btn) => {
      btn.classList.toggle("active", btn.dataset.tab === tabName);
    });
    document.querySelectorAll(".tab-pane").forEach((pane) => {
      pane.classList.toggle("active", pane.id === `pane-${tabName}`);
    });
    inspectorDrawer.classList.remove("minimized");
    if (tabName === "chat") {
      const scrollBody = document.getElementById("drawer-scroll-body");
      if (scrollBody) {
        requestAnimationFrame(() => {
          scrollBody.scrollTop = scrollBody.scrollHeight;
        });
      }
    }
  }

  document.querySelectorAll(".tab-btn").forEach((btn) => {
    btn.addEventListener("click", () => switchTab(btn.dataset.tab));
  });

  drawerHandle.addEventListener("click", () => {
    inspectorDrawer.classList.toggle("minimized");
  });

  sessionSelect.addEventListener("change", () => {
    if (sessionSelect.value) {
      sendCommand({ command: "SELECT_SESSION", session_id: sessionSelect.value });
    }
  });

  function resolveSessionTitle(s) {
    const localAlias = state.sessionAliases[s.session_id];
    if (localAlias && localAlias.trim()) return localAlias.trim();
    if (s.custom_title && s.custom_title.trim()) return s.custom_title.trim();
    return s.prompt_preview || "Interactive Agent Session";
  }

  function renderSessionsDropdown() {
    sessionSelect.innerHTML = "";
    for (const s of state.sessions) {
      const opt = document.createElement("option");
      opt.value = s.session_id;
      opt.selected = s.session_id === state.activeSessionId;
      const rawTitle = resolveSessionTitle(s);
      const preview = rawTitle.length > 34 ? rawTitle.slice(0, 34) + "…" : rawTitle;
      const projTag = s.project_id ? `[${s.project_id}] ` : "";
      opt.textContent = `${projTag}${preview} (${s.step_count} steps)`;
      sessionSelect.appendChild(opt);
    }
  }

  function openSessionRenameInline() {
    const targetId = sessionSelect.value || state.activeSessionId;
    if (!targetId) return;
    const sessionObj = state.sessions.find((s) => s.session_id === targetId);
    const currentLabel = sessionObj ? resolveSessionTitle(sessionObj) : "";
    sessionSelect.classList.add("hidden");
    renameSessionBtn?.classList.add("hidden");
    sessionRenameInline?.classList.remove("hidden");
    if (sessionRenameInput) {
      sessionRenameInput.value = currentLabel;
      sessionRenameInput.focus();
      sessionRenameInput.select();
    }
  }

  function closeSessionRenameInline() {
    sessionRenameInline?.classList.add("hidden");
    sessionSelect.classList.remove("hidden");
    renameSessionBtn?.classList.remove("hidden");
  }

  function commitSessionRename() {
    const targetId = sessionSelect.value || state.activeSessionId;
    if (!targetId || !sessionRenameInput) {
      closeSessionRenameInline();
      return;
    }
    const newTitle = sessionRenameInput.value.trim();
    if (newTitle) {
      state.sessionAliases[targetId] = newTitle;
    } else {
      delete state.sessionAliases[targetId];
    }
    saveSessionAliases();
    const sessionObj = state.sessions.find((s) => s.session_id === targetId);
    if (sessionObj) {
      sessionObj.custom_title = newTitle || null;
    }
    sendCommand({
      command: "RENAME_SESSION",
      session_id: targetId,
      title: newTitle,
    });
    renderSessionsDropdown();
    closeSessionRenameInline();
  }

  renameSessionBtn?.addEventListener("click", openSessionRenameInline);
  sessionSelect.addEventListener("dblclick", openSessionRenameInline);
  sessionRenameSaveBtn?.addEventListener("click", commitSessionRename);
  sessionRenameCancelBtn?.addEventListener("click", closeSessionRenameInline);
  sessionRenameInput?.addEventListener("keydown", (event) => {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === "Enter") {
      event.preventDefault();
      commitSessionRename();
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeSessionRenameInline();
    }
  });

  function setExecutionMode(mode) {
    state.executionMode = mode;
    const isCouncil = mode === "council";
    modeIndependentBtn.classList.toggle("active", !isCouncil);
    modeCouncilBtn.classList.toggle("council-active", isCouncil);
    providerSelect.style.display = isCouncil ? "none" : "";
    modelSelect.style.display = isCouncil ? "none" : "";
    submitPromptBtn.textContent = isCouncil ? "🏛️ CONVENE COUNCIL ➔" : "RUN AGENT ➔";
    submitPromptBtn.classList.toggle("council-send", isCouncil);
    updatePromptPlaceholder();
    renderProviderControls();
    renderAtelierLeftRail();
  }

  modeIndependentBtn.addEventListener("click", () => setExecutionMode("independent"));
  modeCouncilBtn.addEventListener("click", () => setExecutionMode("council"));

  function updatePromptPlaceholder() {
    if (state.executionMode === "council") {
      promptInput.placeholder = `Ask ${state.councilParticipants.size} AI Council members to propose, debate & vote for '${state.activeProject}'...`;
    } else {
      promptInput.placeholder = `Run ${state.selectedProvider} (${state.selectedModel}) on project '${state.activeProject}'...`;
    }
  }

  function renderProviderControls() {
    const prevProvider = state.selectedProvider;
    providerSelect.innerHTML = "";
    for (const p of state.providers) {
      const opt = document.createElement("option");
      opt.value = p.id;
      opt.selected = p.id === prevProvider;
      opt.textContent = `${p.icon} ${p.name} [${p.mode === "SIMULATED" ? "SIM" : "LIVE"}]`;
      providerSelect.appendChild(opt);
    }

    updateModelDropdown();

    councilPillsRow.innerHTML = "";
    const label = document.createElement("span");
    label.style.fontSize = "11px";
    label.style.color = "var(--text-muted)";
    label.textContent = state.executionMode === "council" ? "Council Seats:" : "Providers:";
    councilPillsRow.appendChild(label);

    for (const p of state.providers) {
      const chip = document.createElement("button");
      chip.type = "button";
      const isSelected = state.councilParticipants.has(p.id);
      chip.className = `provider-chip ${isSelected ? "selected" : ""}`;
      chip.style.borderColor = isSelected ? p.color : "";
      chip.textContent = `${p.icon} ${p.name.split(" ")[0]}`;
      chip.title = `${p.name} (${p.mode}) — Click to toggle`;
      chip.addEventListener("click", () => {
        if (state.executionMode !== "council") {
          state.selectedProvider = p.id;
          providerSelect.value = p.id;
          updateModelDropdown();
          updatePromptPlaceholder();
          return;
        }
        if (state.councilParticipants.has(p.id)) {
          if (state.councilParticipants.size > 2) {
            state.councilParticipants.delete(p.id);
          }
        } else {
          state.councilParticipants.add(p.id);
        }
        renderProviderControls();
        renderAtelierLeftRail();
        updatePromptPlaceholder();
      });
      councilPillsRow.appendChild(chip);
    }
  }

  function updateModelDropdown() {
    const p = state.providers.find((x) => x.id === state.selectedProvider) || state.providers[0];
    if (!p) return;
    modelSelect.innerHTML = "";
    for (const m of p.available_models) {
      const opt = document.createElement("option");
      opt.value = m;
      opt.textContent = m;
      modelSelect.appendChild(opt);
    }
    state.selectedModel = p.available_models[0] || p.default_model;
  }

  providerSelect.addEventListener("change", () => {
    state.selectedProvider = providerSelect.value;
    updateModelDropdown();
    updatePromptPlaceholder();
  });

  modelSelect.addEventListener("change", () => {
    state.selectedModel = modelSelect.value;
    updatePromptPlaceholder();
  });

  // Modern Web Guidance: form-fields-automatically-fit-contents fallback + ime-safe-enter-submit
  const growablePromptWrap = document.getElementById("growable-prompt-wrap");
  function setPromptValue(val, focus = false) {
    if (!promptInput) return;
    promptInput.value = val;
    if (growablePromptWrap) {
      growablePromptWrap.dataset.replicatedValue = val;
    }
    if (focus) promptInput.focus();
  }

  if (typeof CSS !== "undefined" && !CSS.supports("field-sizing", "content") && growablePromptWrap) {
    promptInput.addEventListener("input", () => {
      growablePromptWrap.dataset.replicatedValue = promptInput.value;
    });
  }

  promptInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      // Block premature submission during IME composition (including Safari keyCode 229)
      if (event.isComposing || event.keyCode === 229) {
        return;
      }
      promptForm.requestSubmit();
    }
  });

  function pushOptimisticChatTurn(promptText, isCouncil) {
    const pInfo =
      state.providers.find((p) => p.id === state.selectedProvider) || {
        id: state.selectedProvider,
        name: state.selectedProvider.toUpperCase(),
        icon: "⚡",
        model_class: "GENERATIVE_SYSTEM_2",
      };
    const nowStr = new Date().toISOString().slice(0, 16).replace("T", " ");

    state.chatThread.push({
      id: `opt-user-${Date.now()}`,
      step_index: state.chatThread.length + 1,
      role: "user",
      sender: isCouncil ? "You (Council Request)" : "You",
      provider: "user",
      model: "",
      timestamp: nowStr,
      content: promptText,
      thinking: null,
      tool_calls: [],
      streaming: false,
    });

    state.chatThread.push({
      id: `opt-reply-${Date.now()}`,
      step_index: state.chatThread.length + 1,
      role: isCouncil
        ? "council"
        : pInfo.model_class === "DECISION_SYSTEM_1"
        ? "decision"
        : "assistant",
      sender: isCouncil ? "🏛️ AI Council" : `${pInfo.icon} ${pInfo.name}`,
      provider: isCouncil ? "antigravity" : state.selectedProvider,
      model: isCouncil ? "borda-jev-consensus-v2" : state.selectedModel,
      timestamp: nowStr,
      content: "",
      thinking: isCouncil
        ? "Convening AI Council peers & Jev System-1 Decision Gate..."
        : `Dispatching prompt to ${pInfo.name} (${state.selectedModel})...`,
      tool_calls: [
        {
          name: isCouncil ? "invoke_subagent" : "read_file",
          summary: isCouncil
            ? "Fanning out prompt to Council peers"
            : `${state.activeProject}/src (workspace context)`,
          status: "Running",
        },
      ],
      streaming: true,
    });

    const chatBtn = document.getElementById("tab-btn-chat");
    if (chatBtn) {
      chatBtn.textContent = `💬 CHAT (${state.chatThread.length})`;
    }
    if (state.devopsMainOpen || state.fileViewerMainOpen || state.skillsToolsMainOpen) {
      setActiveMainOverlay(null);
    }
    switchTab("chat");
    renderChatTab(true);
    syncStopButtonState();
    requestDraw();
  }

  function dispatchUserPrompt({ promptText, isCouncil = false, provider = null, model = null }) {
    const text = String(promptText || "").trim();
    if (!text) return;

    if (provider && !isCouncil) {
      state.selectedProvider = provider;
      if (providerSelect) providerSelect.value = provider;
      updateModelDropdown();
    }
    if (model && !isCouncil) {
      state.selectedModel = model;
      if (modelSelect) modelSelect.value = model;
    }

    setExecutionMode(isCouncil ? "council" : "independent");
    pushOptimisticChatTurn(text, isCouncil);

    if (isCouncil) {
      sendCommand({
        command: "START_COUNCIL",
        prompt: text,
        providers: Array.from(state.councilParticipants),
        session_id: state.activeSessionId,
        project_id: state.activeProject,
      });
    } else {
      sendCommand({
        command: "SUBMIT_PROMPT",
        prompt: text,
        provider: state.selectedProvider,
        model: state.selectedModel,
        session_id: state.activeSessionId,
        project_id: state.activeProject,
      });
    }
  }

  promptForm.addEventListener("submit", (e) => {
    e.preventDefault();
    const text = promptInput.value.trim();
    if (!text) return;

    dispatchUserPrompt({
      promptText: text,
      isCouncil: state.executionMode === "council",
    });

    setPromptValue("");
  });

  document.getElementById("reset-view-btn")?.addEventListener("click", () => {
    state.panX = 0;
    state.panY = 0;
    state.zoom = 1.0;
    requestDraw();
  });

  document.getElementById("auto-layout-btn")?.addEventListener("click", () => {
    autoLayoutNodes();
    requestDraw();
  });

  function autoLayoutNodes() {
    if (!state.agents.length) return;
    const roots = state.agents.filter((a) => !a.parent_id);
    const childrenMap = new Map();
    for (const a of state.agents) {
      if (a.parent_id) {
        if (!childrenMap.has(a.parent_id)) childrenMap.set(a.parent_id, []);
        childrenMap.get(a.parent_id).push(a);
      }
    }

    const centerX = canvas.clientWidth / 2 || 460;
    roots.forEach((r, idx) => {
      r.x = centerX + (idx - (roots.length - 1) / 2) * 280;
      r.y = 115;
      const kids = childrenMap.get(r.agent_id) || [];
      kids.forEach((k, kIdx) => {
        k.x = r.x + (kIdx - (kids.length - 1) / 2) * 235;
        k.y = 305;
        const grandKids = childrenMap.get(k.agent_id) || [];
        grandKids.forEach((gk, gIdx) => {
          gk.x = k.x + (gIdx - (grandKids.length - 1) / 2) * 220;
          gk.y = 490;
        });
      });
    });
  }

  // =========================================================================
  // 4. Variant C Exclusive Left Rail + Inspector Tabs Rendering
  // =========================================================================
  function escapeHtml(str) {
    return String(str || "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  function escapeAttr(str) {
    return escapeHtml(str).replace(/"/g, "&quot;").replace(/'/g, "&#39;");
  }

  function sanitizeHrefUrl(rawUrl) {
    const clean = String(rawUrl || "").trim();
    if (
      !clean ||
      /^(javascript|vbscript|data):/i.test(clean.replace(/[\u0000-\u001F\s]+/g, ""))
    ) {
      return "#blocked-unsafe-url";
    }
    return clean;
  }

  function isLikelyOpenableFileToken(str) {
    const clean = String(str || "").trim().split("#")[0];
    if (!clean || clean.startsWith("http://") || clean.startsWith("https://")) return false;
    if (
      /^[a-zA-Z0-9_./~-]+\.(md|markdown|mdx|rs|js|mjs|cjs|ts|tsx|jsx|css|html|json|toml|yaml|yml|sh|py|go|sql|txt|jpg|jpeg|png|gif|webp|svg)$/i.test(
        clean
      )
    ) {
      return true;
    }
    return (state.artifacts || []).some(
      (a) => a.name.toLowerCase() === clean.toLowerCase()
    );
  }

  function renderInlineMarkdown(raw) {
    const tokens = [];
    const stash = (html) => {
      const key = `\x00MDTOK_${tokens.length}_\x00`;
      tokens.push(html);
      return key;
    };

    let text = String(raw || "");

    // 0. Markdown Images: ![alt](url)
    text = text.replace(/!\[([^\]]*)\]\(([^)\s]+)\)/g, (_, rawAlt, rawUrl) => {
      const url = sanitizeHrefUrl(rawUrl);
      let imgSrc = url;
      if (url.startsWith("file:///")) {
        imgSrc = `/raw-file?path=${encodeURIComponent(url.replace(/^file:\/\//, ""))}`;
      } else if (url.startsWith("/Users/")) {
        imgSrc = `/raw-file?path=${encodeURIComponent(url)}`;
      }
      const altText = rawAlt.trim();
      return stash(
        `<figure class="md-img-wrap"><img src="${escapeAttr(imgSrc)}" alt="${escapeAttr(
          altText
        )}" class="md-img" loading="lazy" />${
          altText ? `<figcaption class="md-img-caption">${escapeHtml(altText)}</figcaption>` : ""
        }</figure>`
      );
    });

    // 1. Markdown Links: [`code`](url) or [label](url)
    text = text.replace(/\[(`[^`]+`|[^\]]+)\]\(([^)\s]+)\)/g, (_, rawLabel, rawUrl) => {
      const labelHtml =
        rawLabel.startsWith("`") && rawLabel.endsWith("`") && rawLabel.length > 2
          ? `<code class="md-inline-code">${escapeHtml(rawLabel.slice(1, -1))}</code>`
          : escapeHtml(rawLabel);

      const url = sanitizeHrefUrl(rawUrl);
      if (url === "#blocked-unsafe-url") {
        return stash(
          `<a href="#blocked-unsafe-url" class="md-link" rel="noopener noreferrer">${labelHtml}</a>`
        );
      }
      if (url.startsWith("file://") || isLikelyOpenableFileToken(url)) {
        const withoutScheme = url.replace(/^file:\/\//, "");
        const [cleanPath, frag] = withoutScheme.split("#");
        const anchor = frag ? `#${frag}` : "";
        return stash(
          `<a href="${escapeAttr(url)}" class="md-link md-file-link" data-open-file-path="${escapeAttr(
            cleanPath
          )}" data-line-anchor="${escapeAttr(
            anchor
          )}" title="View ${escapeAttr(cleanPath)}${escapeAttr(anchor)} on Main Screen">📄 ${labelHtml}${
            anchor
              ? `<span style="opacity:0.75;font-size:9.5px;margin-left:2px;">${escapeHtml(
                  anchor
                )}</span>`
              : ""
          }</a>`
        );
      }

      if (url.startsWith("conversation://")) {
        return stash(
          `<a href="${escapeAttr(url)}" class="md-link md-file-link" title="${escapeAttr(
            url
          )}">🔗 ${labelHtml}</a>`
        );
      }

      return stash(
        `<a href="${escapeAttr(url)}" class="md-link" target="_blank" rel="noopener noreferrer">${labelHtml} ↗</a>`
      );
    });

    // 2. Inline code spans: `code` (with hex color swatches & clickable file chips)
    text = text.replace(/`([^`\n]+)`/g, (_, code) => {
      const trimmedCode = code.trim();
      if (/^#[0-9A-Fa-f]{6}$/.test(trimmedCode)) {
        return stash(
          `<code class="md-inline-code"><span class="md-color-swatch" style="background-color:${escapeAttr(
            trimmedCode
          )};"></span>${escapeHtml(code)}</code>`
        );
      }
      if (isLikelyOpenableFileToken(trimmedCode)) {
        const [cleanPath, frag] = trimmedCode.split("#");
        const anchor = frag ? `#${frag}` : "";
        return stash(
          `<a href="#" class="md-link md-file-link" data-open-file-path="${escapeAttr(
            cleanPath
          )}" data-line-anchor="${escapeAttr(
            anchor
          )}" title="View ${escapeAttr(cleanPath)} on Main Screen">📄 <code class="md-inline-code">${escapeHtml(
            code
          )}</code></a>`
        );
      }
      return stash(`<code class="md-inline-code">${escapeHtml(code)}</code>`);
    });

    // 3. Bare http/https URLs
    text = text.replace(/\b(https?:\/\/[^\s<)"']+)/g, (_, url) =>
      stash(
        `<a href="${escapeAttr(url)}" class="md-link" target="_blank" rel="noopener noreferrer">${escapeHtml(
          url
        )} ↗</a>`
      )
    );

    // 3b. Bare openable filenames in text (e.g. ui_branding_redesign_plan.md, trellis-espalier.jpg)
    text = text.replace(
      /\b([a-zA-Z0-9_-]+(?:\/[a-zA-Z0-9_.-]+)*\.(?:md|markdown|rs|js|ts|tsx|css|html|json|toml|sh|jpg|jpeg|png|svg))\b/g,
      (match) => {
        if (!isLikelyOpenableFileToken(match)) return match;
        return stash(
          `<a href="#" class="md-link md-file-link" data-open-file-path="${escapeAttr(
            match
          )}" title="View ${escapeAttr(match)} on Main Screen">📄 ${escapeHtml(match)}</a>`
        );
      }
    );

    // 4. Escape remaining HTML
    let safe = escapeHtml(text);

    // 5. Bold & Italic markup
    safe = safe
      .replace(/\*\*\*([^*]+)\*\*\*/g, "<strong><em>$1</em></strong>")
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/\*([^*\n]+)\*/g, "<em>$1</em>");

    // 6. Restore stashed HTML tokens
    return safe.replace(/\x00MDTOK_(\d+)_\x00/g, (_, idx) => tokens[Number(idx)] || "");
  }

  function renderMarkdown(rawText) {
    const src = String(rawText || "").replace(/\r\n/g, "\n").trim();
    if (!src) return "";

    const lines = src.split("\n");
    const blocks = [];
    let i = 0;

    while (i < lines.length) {
      const line = lines[i];
      const trimmed = line.trim();

      if (!trimmed) {
        i++;
        continue;
      }

      // Fenced Code Block: ```lang
      if (trimmed.startsWith("```")) {
        const lang = trimmed.slice(3).trim();
        const codeLines = [];
        i++;
        while (i < lines.length && !lines[i].trim().startsWith("```")) {
          codeLines.push(lines[i]);
          i++;
        }
        if (i < lines.length) i++; // skip closing ```
        blocks.push(`
          <div class="md-code-wrap">
            ${lang ? `<div class="md-code-lang">${escapeHtml(lang)}</div>` : ""}
            <pre class="md-code-block"><code>${escapeHtml(codeLines.join("\n"))}</code></pre>
          </div>
        `);
        continue;
      }

      // Horizontal rule
      if (/^(-{3,}|\*{3,}|═{4,})$/.test(trimmed)) {
        blocks.push(`<hr class="md-hr" />`);
        i++;
        continue;
      }

      // Headings: # .. ####
      const hMatch = trimmed.match(/^(#{1,4})\s+(.+)$/);
      if (hMatch) {
        const level = hMatch[1].length;
        blocks.push(`<div class="md-h${level}">${renderInlineMarkdown(hMatch[2])}</div>`);
        i++;
        continue;
      }

      // Blockquote or GitHub-style Alert: > [!IMPORTANT] / > ...
      if (trimmed.startsWith(">")) {
        const quoteLines = [];
        while (i < lines.length && lines[i].trim().startsWith(">")) {
          quoteLines.push(lines[i].trim().replace(/^>\s?/, ""));
          i++;
        }
        const firstQ = quoteLines[0] || "";
        const alertMatch = firstQ.match(/^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*(.*)$/i);
        if (alertMatch) {
          const kind = alertMatch[1].toLowerCase();
          const remainderFirst = alertMatch[2].trim();
          const bodyLines = remainderFirst
            ? [remainderFirst, ...quoteLines.slice(1)]
            : quoteLines.slice(1);
          const iconMap = {
            note: "ℹ️",
            tip: "💡",
            important: "📌",
            warning: "⚠️",
            caution: "🛑",
          };
          blocks.push(`
            <div class="md-alert md-alert-${escapeAttr(kind)}">
              <div class="md-alert-header">${iconMap[kind] || "📌"} ${escapeHtml(
            kind.toUpperCase()
          )}</div>
              <div class="md-alert-body">${bodyLines
                .filter((ql) => ql.trim().length > 0)
                .map((ql) => renderInlineMarkdown(ql))
                .join("<br/>")}</div>
            </div>
          `);
        } else {
          blocks.push(
            `<blockquote class="md-quote">${quoteLines
              .map((ql) => renderInlineMarkdown(ql))
              .join("<br/>")}</blockquote>`
          );
        }
        continue;
      }

      // Markdown Table: | col | col |
      if (
        trimmed.startsWith("|") &&
        i + 1 < lines.length &&
        /^\s*\|[\s\-:|]+\|\s*$/.test(lines[i + 1])
      ) {
        const parseCells = (rowStr) =>
          rowStr
            .trim()
            .replace(/^\|/, "")
            .replace(/\|$/, "")
            .split("|")
            .map((c) => renderInlineMarkdown(c.trim()));

        const headers = parseCells(lines[i]);
        i += 2; // skip header and separator row
        const bodyRows = [];
        while (i < lines.length && lines[i].trim().startsWith("|")) {
          bodyRows.push(parseCells(lines[i]));
          i++;
        }
        blocks.push(`
          <div class="md-table-wrap">
            <table class="leaderboard-table md-table">
              <thead>
                <tr>${headers.map((h) => `<th>${h}</th>`).join("")}</tr>
              </thead>
              <tbody>
                ${bodyRows
                  .map((r) => `<tr>${r.map((c) => `<td>${c}</td>`).join("")}</tr>`)
                  .join("")}
              </tbody>
            </table>
          </div>
        `);
        continue;
      }

      // Unordered (- / * / •) or Ordered (1. / 2.) List
      const isUl = /^[-*•]\s+/.test(trimmed);
      if (isUl || /^\d+\.\s+/.test(trimmed)) {
        const tag = isUl ? "ul" : "ol";
        const itemRe = isUl ? /^[-*•]\s+(.+)$/ : /^\d+\.\s+(.+)$/;
        const items = [];
        while (i < lines.length) {
          const l = lines[i].trim();
          const m = l.match(itemRe);
          if (m) {
            items.push(renderInlineMarkdown(m[1]));
            i++;
          } else if (lines[i].startsWith("  ") && items.length > 0 && l) {
            const subClean = l.replace(/^[-*•]\s+/, "• ");
            items[items.length - 1] += `<br/><span style="padding-inline-start:14px;display:inline-block;">${renderInlineMarkdown(subClean)}</span>`;
            i++;
          } else {
            break;
          }
        }
        blocks.push(`<${tag} class="md-list">${items.map((it) => `<li>${it}</li>`).join("")}</${tag}>`);
        continue;
      }

      // Standard Paragraph (collect consecutive non-special lines)
      const paraLines = [];
      while (
        i < lines.length &&
        lines[i].trim() &&
        !lines[i].trim().startsWith("```") &&
        !lines[i].trim().startsWith("#") &&
        !lines[i].trim().startsWith(">") &&
        !/^([-*•]|\d+\.)\s+/.test(lines[i].trim()) &&
        !/^(-{3,}|\*{3,}|═{4,})$/.test(lines[i].trim())
      ) {
        paraLines.push(renderInlineMarkdown(lines[i].trim()));
        i++;
      }
      blocks.push(`<p class="md-p">${paraLines.join("<br/>")}</p>`);
    }

    return blocks.join("");
  }

  // Intercept clicks on any file or artifact link across the entire application
  document.addEventListener("click", (e) => {
    const fileLink = e.target.closest("a[data-open-file-path], a[data-open-rel]");
    if (!fileLink) return;
    e.preventDefault();
    const targetPath = fileLink.dataset.openFilePath || fileLink.dataset.openRel;
    const lineAnchor = fileLink.dataset.lineAnchor || "";
    if (targetPath) {
      openFileOnMainScreen(targetPath, lineAnchor);
    }
  });

  const STATUS_BADGE_CLASSES = {
    Running: "running",
    Completed: "completed",
    DecisionGate: "completed",
    AwaitingToolApproval: "awaiting",
    AwaitingUserInput: "awaiting",
    Negotiating: "negotiating",
    Voting: "negotiating",
    Winner: "winner",
  };
  const statusBadgeClass = (st) => STATUS_BADGE_CLASSES[st] || "";

  function renderAtelierLeftRail() {
    if (!atelierLeftRail) return;
    const sys1 = state.providers.filter((p) => p.model_class === "DECISION_SYSTEM_1");
    const sys2 = state.providers.filter((p) => p.model_class !== "DECISION_SYSTEM_1");

    const renderGroup = (title, items) => `
      <div style="display:flex;flex-direction:column;gap:6px;">
        <div style="font-size:10.5px;font-weight:800;letter-spacing:0.6px;color:var(--text-muted);text-transform:uppercase;">
          ${escapeHtml(title)}
        </div>
        ${items
          .map(
            (p) => `
            <div class="file-row" style="border-left:3px solid ${escapeAttr(p.color)};padding:7px 9px;cursor:pointer;" data-rail-provider="${escapeAttr(p.id)}">
              <div>
                <div style="font-size:12px;font-weight:700;color:var(--text-main);">${p.icon} ${escapeHtml(p.name)}</div>
                <div style="font-size:10.5px;color:var(--text-muted);">${escapeHtml(p.default_model)}</div>
              </div>
              <span class="badge ${p.mode.startsWith("LIVE") ? "completed" : "awaiting"}">${escapeHtml(p.mode.replace("_", " "))}</span>
            </div>`
          )
          .join("")}
      </div>
    `;

    atelierLeftRail.innerHTML = `
      <div class="drawer-header-bar">
        <div>
          <div class="drawer-header-title">Trellis Roster</div>
          <div style="font-size:10px;color:var(--text-muted);font-style:italic;">Structure for intelligence.</div>
        </div>
        <button id="collapse-left-btn" class="collapse-drawer-btn" type="button" title="Collapse Roster Drawer">
          « Hide
        </button>
      </div>
      <div style="border-radius:10px;overflow:hidden;border:1px solid var(--border);background:#F7F4EB;flex-shrink:0;">
        <img src="/trellis-espalier.jpg" alt="Trellis — AI Harness" style="display:block;inline-size:100%;height:auto;" />
      </div>
      ${renderGroup("System-1 Decision Gate", sys1)}
      ${renderGroup("System-2 Generative Tiers", sys2)}
      <div class="lattice-quick-card">
        <div style="display:flex;align-items:center;justify-content:space-between;">
          <span style="font-size:10.5px;font-weight:800;letter-spacing:0.6px;color:var(--text-muted);text-transform:uppercase;">
            🧰 Capabilities Lattice
          </span>
          <span style="font-family:var(--font-mono);font-size:10px;color:var(--moss-strong);font-weight:700;">
            ${state.skills.length + state.tools.length} ready
          </span>
        </div>
        <div class="lattice-counter-row">
          <button type="button" class="lattice-counter-btn" data-open-cap-seg="skills">
            <span style="font-size:10px;color:var(--moss-strong);font-weight:700;">📘 SKILLS</span>
            <span style="font-family:var(--font-mono);font-size:13px;font-weight:800;color:var(--text-main);">${
              state.skills.length
            }</span>
          </button>
          <button type="button" class="lattice-counter-btn" data-open-cap-seg="tools">
            <span style="font-size:10px;color:var(--terracotta-strong);font-weight:700;">🔌 TOOLS &amp; MCP</span>
            <span style="font-family:var(--font-mono);font-size:13px;font-weight:800;color:var(--text-main);">${
              state.tools.length
            }</span>
          </button>
        </div>
        <div class="skills-source-pills">
          <button type="button" class="source-filter-pill" data-open-cap-source="native-harness">native</button>
          <button type="button" class="source-filter-pill" data-open-cap-source="chrome-devtools">devtools</button>
          <button type="button" class="source-filter-pill" data-open-cap-source="github">github</button>
          <button type="button" class="source-filter-pill" data-open-cap-source="user-skills">skills</button>
        </div>
      </div>
      <div class="card" style="margin-top:auto;">
        <div style="font-size:11px;font-weight:700;color:var(--moss-strong);">Espalier Topology</div>
        <div style="font-size:11.5px;color:var(--text-muted);">
          Agents: <strong>${state.agents.filter((a) => a.node_kind !== "task").length}</strong> • Active Tasks: <strong>${state.agents.filter((a) => a.node_kind === "task").length}</strong> • Mode: <strong>${state.executionMode.toUpperCase()}</strong>
        </div>
      </div>
    `;

    document.getElementById("collapse-left-btn")?.addEventListener("click", () => {
      setLeftDrawerOpen(false);
    });

    atelierLeftRail.querySelectorAll("[data-open-cap-seg]").forEach((btn) => {
      btn.addEventListener("click", () => {
        state.skillsToolsSegment = btn.dataset.openCapSeg || "all";
        state.skillsToolsSource = "all";
        setSkillsToolsMainOpen(true);
      });
    });

    atelierLeftRail.querySelectorAll("[data-open-cap-source]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const src = btn.dataset.openCapSource || "all";
        state.skillsToolsSource = src;
        state.skillsToolsSegment = src === "user-skills" ? "skills" : "tools";
        setSkillsToolsMainOpen(true);
      });
    });

    atelierLeftRail.querySelectorAll("[data-rail-provider]").forEach((el) => {
      el.addEventListener("click", () => {
        const pid = el.dataset.railProvider;
        if (!pid) return;
        dispatchUserPrompt({
          promptText: `Execute ${pid} check on ${state.activeProject}`,
          isCouncil: false,
          provider: pid,
        });
      });
    });
  }

  function renderSkillsToolsTab() {
    const pane = document.getElementById("pane-skills-tools");
    if (!pane) return;

    const q = (state.skillsToolsQuery || "").trim().toLowerCase();
    const seg = state.skillsToolsSegment || "all";
    const srcFilter = state.skillsToolsSource || "all";

    // Collect distinct sources from skills + tools
    const allSources = [
      "all",
      ...new Set(
        [...state.skills.map((s) => s.source), ...state.tools.map((t) => t.server)].filter(Boolean)
      ),
    ];

    const filteredSkills =
      seg === "tools"
        ? []
        : state.skills.filter((s) => {
            if (srcFilter !== "all" && s.source !== srcFilter) return false;
            if (!q) return true;
            return (
              s.name.toLowerCase().includes(q) ||
              s.description.toLowerCase().includes(q) ||
              s.source.toLowerCase().includes(q)
            );
          });

    const filteredTools =
      seg === "skills"
        ? []
        : state.tools.filter((t) => {
            if (srcFilter !== "all" && t.server !== srcFilter) return false;
            if (!q) return true;
            return (
              t.name.toLowerCase().includes(q) ||
              t.description.toLowerCase().includes(q) ||
              t.server.toLowerCase().includes(q) ||
              t.parameters_summary.toLowerCase().includes(q)
            );
          });

    const totalCount = state.skills.length + state.tools.length;

    let toolbarEl = pane.querySelector(".skills-tools-toolbar");
    let listEl = pane.querySelector(".skills-tools-results");

    if (!toolbarEl || !listEl) {
      pane.innerHTML = `
        <div class="skills-tools-toolbar">
          <div class="skills-toolbar-top-row">
            <div>
              <div style="font-family:var(--font-display);font-size:14px;font-weight:700;letter-spacing:0.06em;text-transform:uppercase;color:var(--text-main);">
                🧰 Capabilities Lattice — Skills &amp; Tools
              </div>
              <div style="font-size:11.5px;color:var(--text-muted);">
                Browse, filter, and inspect installed <code>SKILL.md</code> guides and MCP tool schemas
              </div>
            </div>
            <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
              <div class="skills-segmented" role="group" aria-label="Filter Skills or Tools">
                <button type="button" class="skills-seg-btn" data-seg="all"></button>
                <button type="button" class="skills-seg-btn" data-seg="skills"></button>
                <button type="button" class="skills-seg-btn" data-seg="tools"></button>
              </div>
              <button type="button" id="close-skills-tools-main-btn" class="icon-btn" style="border-color:var(--moss);color:var(--moss-strong);">
                🌿 Back to Agent Graph
              </button>
            </div>
          </div>
          <div class="skills-toolbar-controls-row">
            <input
              type="search"
              id="skills-tools-search"
              class="skills-search-input"
              placeholder="Search skills (SKILL.md), MCP servers, or tool schemas…"
              aria-label="Search skills and tools"
            />
          </div>
          <div class="skills-source-pills" id="skills-source-pills-bar"></div>
        </div>
        <div class="skills-tools-results"></div>
      `;
      toolbarEl = pane.querySelector(".skills-tools-toolbar");
      listEl = pane.querySelector(".skills-tools-results");

      pane.querySelector("#close-skills-tools-main-btn")?.addEventListener("click", () => {
        setSkillsToolsMainOpen(false);
      });

      const searchInput = pane.querySelector("#skills-tools-search");
      if (searchInput) {
        searchInput.value = state.skillsToolsQuery;
        searchInput.addEventListener("input", () => {
          state.skillsToolsQuery = searchInput.value;
          renderSkillsToolsTab();
        });
      }

      toolbarEl.querySelectorAll(".skills-seg-btn").forEach((btn) => {
        btn.addEventListener("click", () => {
          state.skillsToolsSegment = btn.dataset.seg || "all";
          renderSkillsToolsTab();
        });
      });
    }

    // Update segmented button labels & active states
    toolbarEl.querySelectorAll(".skills-seg-btn").forEach((btn) => {
      const bSeg = btn.dataset.seg;
      btn.classList.toggle("active", bSeg === seg);
      if (bSeg === "all") btn.textContent = `All (${totalCount})`;
      else if (bSeg === "skills") btn.textContent = `📘 Skills (${state.skills.length})`;
      else if (bSeg === "tools") btn.textContent = `🔌 Tools & MCP (${state.tools.length})`;
    });

    // Update source filter pills
    const pillsBar = pane.querySelector("#skills-source-pills-bar");
    if (pillsBar) {
      pillsBar.innerHTML = allSources
        .map(
          (src) => `
          <button
            type="button"
            class="source-filter-pill ${src === srcFilter ? "active" : ""}"
            data-src-pill="${escapeAttr(src)}"
          >
            ${src === "all" ? "All Sources" : escapeHtml(src)}
          </button>`
        )
        .join("");

      pillsBar.querySelectorAll("[data-src-pill]").forEach((btn) => {
        btn.addEventListener("click", () => {
          state.skillsToolsSource = btn.dataset.srcPill || "all";
          renderSkillsToolsTab();
        });
      });
    }

    const skillCardsHtml = filteredSkills
      .map(
        (s) => `
        <article class="capability-card skill-card">
          <div class="card-header">
            <span class="capability-title">📘 ${escapeHtml(s.name)}</span>
            <span class="badge completed">${escapeHtml(s.source)}</span>
          </div>
          <div class="capability-desc">${escapeHtml(s.description)}</div>
          <details class="capability-inspect-details">
            <summary>
              <span>Inspect SKILL.md &amp; Path</span>
              <span style="font-family:var(--font-mono);font-size:10px;color:var(--text-muted);">▾</span>
            </summary>
            <div style="margin-top:7px;display:flex;flex-direction:column;gap:6px;">
              <div style="font-family:var(--font-mono);font-size:10px;color:var(--text-muted);word-break:break-all;">
                Path: <code>${escapeHtml(s.path)}</code>
              </div>
              <div class="mono-box md-rendered" style="max-block-size:220px;">${renderMarkdown(
                s.content_preview
              )}</div>
            </div>
          </details>
          <div style="display:flex;justify-content:flex-end;">
            <button type="button" class="mini-btn primary" data-inject-prompt="Use skill ${escapeAttr(
              s.name
            )} (${escapeAttr(s.path)}): ">
              + Use Skill in Prompt
            </button>
          </div>
        </article>`
      )
      .join("");

    const toolCardsHtml = filteredTools
      .map(
        (t) => `
        <article class="capability-card tool-card">
          <div class="card-header">
            <span class="capability-title">🔌 ${escapeHtml(t.name)}</span>
            <div style="display:flex;gap:4px;align-items:center;">
              <span class="badge ${t.mode === "EAGER" ? "running" : "negotiating"}">${escapeHtml(
          t.mode
        )}</span>
              <span class="badge awaiting">${escapeHtml(t.server)}</span>
            </div>
          </div>
          <div class="capability-desc">${escapeHtml(t.description)}</div>
          <div class="capability-params" title="Tool Parameters">
            <strong>Params:</strong> ${escapeHtml(t.parameters_summary)}
          </div>
          <details class="capability-inspect-details">
            <summary>
              <span>Inspect JSON Schema</span>
              <span style="font-family:var(--font-mono);font-size:10px;color:var(--text-muted);">▾</span>
            </summary>
            <pre class="mono-box" style="margin-top:6px;max-block-size:190px;">${escapeHtml(
              t.schema_preview
            )}</pre>
          </details>
          <div style="display:flex;justify-content:flex-end;">
            <button type="button" class="mini-btn" data-inject-prompt="Run tool ${escapeAttr(
              t.name
            )} (${escapeAttr(t.server)}): ">
              + Use Tool in Prompt
            </button>
          </div>
        </article>`
      )
      .join("");

    if (!skillCardsHtml && !toolCardsHtml) {
      listEl.innerHTML = `
        <div class="card">
          <div style="font-size:12px;color:var(--text-muted);">
            No matching skills or tools found for filter <strong>${escapeHtml(
              q || srcFilter
            )}</strong>.
          </div>
        </div>
      `;
    } else {
      listEl.innerHTML = skillCardsHtml + toolCardsHtml;
    }

    listEl.querySelectorAll("[data-inject-prompt]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const prefix = btn.dataset.injectPrompt || "";
        if (promptInput) setPromptValue(prefix + promptInput.value, true);
      });
    });
  }

  const TOOL_VERB_MAP = {
    read_file: "📖 Reading",
    edit_file: "✏️ Editing",
    write_file: "✏️ Editing",
    run_command: "⚡ Running",
    manage_task: "⚡ Running",
    search_web: "🌐 Searching",
    invoke_subagent: "🌿 Spawning",
  };

  function describeActiveTask(msg) {
    if (msg.content && msg.content.length > 0) {
      return {
        key: "streaming-output",
        verb: "✍️ Streaming",
        toolName: "output",
        target: `Weaving response from ${msg.sender}...`,
      };
    }
    if (msg.tool_calls && msg.tool_calls.length > 0) {
      const tc = msg.tool_calls[msg.tool_calls.length - 1];
      const name = tc.name || "tool";
      const verb =
        TOOL_VERB_MAP[name] ||
        (name.includes("jev") || name.includes("verify") ? "🎯 Verifying" : "⚙ Executing");
      const target = tc.summary || "workspace operation";
      return { key: `${name}:${target}:${msg.tool_calls.length}`, verb, toolName: name, target };
    }
    const firstThinkLine = msg.thinking
      ? msg.thinking.split("\n").find((l) => l.trim().length > 0) || ""
      : "";
    const target = firstThinkLine
      ? firstThinkLine.slice(0, 68)
      : `Analyzing context with ${msg.sender}...`;
    return {
      key: `planning:${target}`,
      verb: "🧠 Planning",
      toolName: msg.provider || "agent",
      target,
    };
  }

  function buildTraceabilityDrawerParts(msg) {
    const tools = msg.tool_calls || [];
    const hasThinking = Boolean(msg.thinking && msg.thinking.trim());
    if (!tools.length && !hasThinking) return null;

    const countsMap = new Map();
    for (const tc of tools) {
      const n = tc.name || "tool";
      countsMap.set(n, (countsMap.get(n) || 0) + 1);
    }
    const countsLabel = Array.from(countsMap.entries())
      .map(([n, c]) => (c > 1 ? `${n} ×${c}` : n))
      .join(", ");

    const stepWord = tools.length === 1 ? "step" : "steps";
    const metaLabel =
      tools.length > 0
        ? `• ${tools.length} ${stepWord}${hasThinking ? " + reasoning" : ""}`
        : "• reasoning";

    const summaryHtml = `
      <span class="chat-trace-summary-left">
        <span class="chat-trace-chevron">▶</span>
        <span>🗂️ Traceability</span>
        <span style="font-weight:400;color:var(--text-muted);white-space:nowrap;">${escapeHtml(
          metaLabel
        )}</span>
      </span>
      <span class="chat-trace-counts" title="${escapeAttr(countsLabel)}">${escapeHtml(
      countsLabel
    )}</span>
    `;

    const stepsHtml = tools.length
      ? `<div class="chat-trace-steps">
          ${tools
            .map((tc, idx) => {
              const isRun = tc.status === "Running" && msg.streaming;
              return `
                <div class="trace-step-row" title="${escapeAttr(tc.summary || "")}">
                  <span class="trace-step-idx">#${idx + 1}</span>
                  <span class="trace-step-name ${isRun ? "running" : ""}">${escapeHtml(
                tc.name
              )}</span>
                  <span class="trace-step-summary">${escapeHtml(tc.summary || "")}</span>
                  <span style="color:${
                    isRun ? "var(--terracotta)" : "var(--moss-strong)"
                  };font-weight:700;">${isRun ? "●" : "✓"}</span>
                </div>
              `;
            })
            .join("")}
        </div>`
      : "";

    const thinkingHtml = hasThinking
      ? `<details class="chat-thinking">
          <summary>🧠 Reasoning / Thinking</summary>
          <div class="chat-thinking-body md-rendered">${renderMarkdown(msg.thinking)}</div>
        </details>`
      : "";

    const lastTool = tools[tools.length - 1];
    const sig = `${tools.length}:${lastTool ? lastTool.name + ":" + lastTool.summary + ":" + lastTool.status : ""}:${
      msg.thinking ? msg.thinking.length : 0
    }:${Boolean(msg.streaming)}`;

    return {
      sig,
      summaryHtml,
      stepsHtml,
      thinkingHtml,
    };
  }

  function renderInteractiveQuestionBanner() {
    const banner = document.getElementById("interactive-question-banner");
    const bannerText = document.getElementById("interactive-banner-text");
    if (!banner || !bannerText) return;

    const pendingMsg = state.chatThread.find(
      (m) => m.interactive_prompt && m.interactive_prompt.status === "Pending"
    );
    const pendingAgent = state.agents.find(
      (a) => a.interactive_prompt && a.interactive_prompt.status === "Pending"
    );
    const pendingIp = pendingMsg?.interactive_prompt || pendingAgent?.interactive_prompt;

    if (!pendingIp) {
      banner.classList.add("hidden");
      banner.hidden = true;
      return;
    }

    const qCount = (pendingIp.questions || []).length || 1;
    bannerText.textContent = `❓ Agent is waiting for your answer (${qCount} question${
      qCount === 1 ? "" : "s"
    }) — Click to respond in Chat →`;
    banner.hidden = false;
    banner.classList.remove("hidden");
    banner.dataset.targetPromptId = pendingIp.prompt_id || "";
  }

  document.getElementById("interactive-question-banner")?.addEventListener("click", () => {
    setRightDrawerOpen(true);
    switchTab("chat");
    const targetId =
      document.getElementById("interactive-question-banner")?.dataset.targetPromptId || "";
    requestAnimationFrame(() => {
      const card = targetId
        ? document.querySelector(`.interactive-question-card[data-prompt-id="${CSS.escape(targetId)}"]`)
        : document.querySelector(".interactive-question-card");
      if (card) {
        card.scrollIntoView({ behavior: "smooth", block: "center" });
      }
    });
  });

  function renderInteractiveQuestionSlot(questionSlot, msg) {
    if (!questionSlot) return;
    const ip = msg.interactive_prompt;
    if (!ip || !Array.isArray(ip.questions) || ip.questions.length === 0) {
      if (questionSlot.innerHTML !== "") {
        questionSlot.innerHTML = "";
        delete questionSlot.dataset.ipSig;
      }
      return;
    }

    const forceEdit = questionSlot.dataset.forceEdit === "true";
    const isAnswered = ip.status === "Answered" && !forceEdit;
    const sig = `${ip.prompt_id}:${ip.status}:${forceEdit ? "edit" : "view"}:${(
      ip.answers || []
    ).join("|")}`;

    if (questionSlot.dataset.ipSig === sig) {
      return;
    }
    questionSlot.dataset.ipSig = sig;

    const qCount = ip.questions.length;
    const qWord = qCount === 1 ? "Question" : "Questions";

    if (isAnswered) {
      const answeredItemsHtml = ip.questions
        .map((q, idx) => {
          const prefix = `A${q.index}:`;
          const matchingAns =
            (ip.answers || []).find((a) => a.trim().startsWith(prefix)) ||
            (ip.answers || [])[idx] ||
            "Answered";
          const cleanAns = matchingAns.replace(/^A\d+:\s*/, "");
          return `
            <div class="iq-answered-item">
              <div class="iq-answered-q">Q${q.index}. ${escapeHtml(q.question)}</div>
              <div class="iq-answered-a">✓ ${escapeHtml(cleanAns)}</div>
            </div>
          `;
        })
        .join("");

      questionSlot.innerHTML = `
        <div class="interactive-question-card answered" data-prompt-id="${escapeAttr(ip.prompt_id)}">
          <div class="iq-card-header">
            <span class="iq-card-title">✓ Interactive Clarification (${qCount} ${qWord})</span>
            <div style="display:flex;align-items:center;gap:6px;">
              <span class="badge completed">ANSWERED</span>
              <button type="button" class="mini-btn iq-edit-btn">✎ Revise Answers</button>
            </div>
          </div>
          <div class="iq-answered-list">
            ${answeredItemsHtml}
          </div>
        </div>
      `;

      questionSlot.querySelector(".iq-edit-btn")?.addEventListener("click", () => {
        questionSlot.dataset.forceEdit = "true";
        delete questionSlot.dataset.ipSig;
        renderInteractiveQuestionSlot(questionSlot, msg);
      });
      return;
    }

    // Render interactive form for Pending (or Revise) mode
    const fieldsetsHtml = ip.questions
      .map((q) => {
        const inputType = q.is_multi_select ? "checkbox" : "radio";
        const groupName = `iq_${ip.prompt_id}_q${q.index}`;
        const existingAnsLine =
          (ip.answers || []).find((a) => a.trim().startsWith(`A${q.index}:`)) || "";
        const existingClean = existingAnsLine.replace(/^A\d+:\s*/, "");
        const hasRecOption = (q.options || []).some((o) => /^\(recommended\)/i.test(o.trim()));

        const optionsHtml = (q.options || [])
          .map((rawOpt, optIdx) => {
            const isRec = /^\(recommended\)/i.test(rawOpt.trim());
            const displayOpt = rawOpt.trim().replace(/^\(recommended\)\s*/i, "");
            const isPreChecked = existingClean
              ? existingClean.includes(rawOpt) || existingClean.includes(displayOpt)
              : isRec || (!q.is_multi_select && !hasRecOption && optIdx === 0);

            return `
              <label class="iq-option-card">
                <input
                  type="${inputType}"
                  class="iq-option-input"
                  name="${escapeAttr(groupName)}"
                  value="${escapeAttr(rawOpt)}"
                  ${isPreChecked ? "checked" : ""}
                />
                <span class="iq-option-text">
                  ${isRec ? `<span class="iq-rec-badge">★ Recommended</span>` : ""}
                  ${escapeHtml(displayOpt)}
                </span>
              </label>
            `;
          })
          .join("");

        return `
          <fieldset class="iq-fieldset" data-q-index="${q.index}" data-multi="${
          q.is_multi_select ? "true" : "false"
        }">
            <legend class="iq-legend">Q${q.index}. ${escapeHtml(q.question)}</legend>
            <div class="iq-mode-hint">${
              q.is_multi_select ? "Select all that apply:" : "Choose one option:"
            }</div>
            <div class="iq-options-list">
              ${optionsHtml}
              <label class="iq-option-card">
                <input
                  type="${inputType}"
                  class="iq-option-input iq-other-toggle"
                  name="${escapeAttr(groupName)}"
                  value="__OTHER__"
                />
                <span class="iq-option-text">
                  Other (specify custom answer)…
                  <input
                    type="text"
                    class="iq-other-input"
                    placeholder="Type custom answer for Q${q.index}…"
                    aria-label="Custom answer for question ${q.index}"
                  />
                </span>
              </label>
            </div>
          </fieldset>
        `;
      })
      .join("");

    questionSlot.innerHTML = `
      <div class="interactive-question-card" data-prompt-id="${escapeAttr(ip.prompt_id)}">
        <div class="iq-card-header">
          <span class="iq-card-title">❓ Interactive Clarification (${qCount} ${qWord})</span>
          <span class="badge running">● AWAITING YOUR INPUT</span>
        </div>
        <form
          class="interactive-question-form"
          data-prompt-id="${escapeAttr(ip.prompt_id)}"
          data-provider="${escapeAttr(msg.provider || state.selectedProvider)}"
          data-model="${escapeAttr(msg.model || state.selectedModel)}"
        >
          ${fieldsetsHtml}
          <div class="iq-submit-row">
            <span style="font-size:11px;color:var(--text-muted);">
              Submit your selection to continue the session directly inside Trellis.
            </span>
            <div style="display:flex;gap:6px;align-items:center;">
              ${
                forceEdit
                  ? `<button type="button" class="mini-btn iq-cancel-edit-btn">Cancel</button>`
                  : ""
              }
              <button type="submit" class="iq-submit-btn">✓ Submit Answers &amp; Continue</button>
            </div>
          </div>
        </form>
      </div>
    `;

    const formEl = questionSlot.querySelector("form.interactive-question-form");
    if (!formEl) return;

    // Automatically check the "Other" radio/checkbox when the user focuses or types in its text box
    formEl.querySelectorAll(".iq-other-input").forEach((textInput) => {
      const label = textInput.closest("label.iq-option-card");
      const toggle = label?.querySelector(".iq-other-toggle");
      textInput.addEventListener("focus", () => {
        if (toggle && !toggle.checked) {
          toggle.checked = true;
        }
      });
      textInput.addEventListener("input", () => {
        if (toggle && textInput.value.trim().length > 0) {
          toggle.checked = true;
        }
      });
    });

    formEl.querySelector(".iq-cancel-edit-btn")?.addEventListener("click", () => {
      delete questionSlot.dataset.forceEdit;
      delete questionSlot.dataset.ipSig;
      renderInteractiveQuestionSlot(questionSlot, msg);
    });

    formEl.addEventListener("submit", (e) => {
      e.preventDefault();
      const submitBtn = formEl.querySelector(".iq-submit-btn");
      if (submitBtn) {
        submitBtn.disabled = true;
        submitBtn.textContent = "Submitting…";
      }

      const collectedAnswers = [];
      formEl.querySelectorAll("fieldset.iq-fieldset").forEach((fs) => {
        const qIndex = fs.dataset.qIndex || "1";
        const chosen = [];
        fs.querySelectorAll("input.iq-option-input:checked").forEach((inp) => {
          if (inp.value === "__OTHER__") {
            const customVal = fs.querySelector(".iq-other-input")?.value.trim();
            if (customVal) {
              chosen.push(customVal);
            }
          } else {
            chosen.push(inp.value);
          }
        });
        if (chosen.length === 0) {
          const customFallback = fs.querySelector(".iq-other-input")?.value.trim();
          if (customFallback) {
            chosen.push(customFallback);
          } else {
            const firstOpt = fs.querySelector("input.iq-option-input")?.value;
            if (firstOpt && firstOpt !== "__OTHER__") {
              chosen.push(firstOpt);
            }
          }
        }
        collectedAnswers.push(`A${qIndex}: ${chosen.join(", ")}`);
      });

      delete questionSlot.dataset.forceEdit;
      ip.status = "Answered";
      ip.answers = collectedAnswers;
      delete questionSlot.dataset.ipSig;
      renderInteractiveQuestionSlot(questionSlot, msg);
      renderInteractiveQuestionBanner();

      sendCommand({
        command: "ANSWER_QUESTION",
        session_id: state.activeSessionId,
        prompt_id: ip.prompt_id,
        answers: collectedAnswers,
        provider: formEl.dataset.provider || state.selectedProvider,
        model: formEl.dataset.model || state.selectedModel,
      });
    });
  }

  const CHAT_ROLE_CLASSES = { user: "user", decision: "decision", council: "council" };

  function renderChatTab(scrollToBottom = false) {
    const pane = document.getElementById("pane-chat");
    if (!pane) return;

    if (!state.chatThread.length) {
      pane.innerHTML = `
        <div class="card" data-empty-chat="true">
          <div class="card-title" style="color:var(--terracotta-strong);">💬 Session Conversation Thread</div>
          <p style="font-size:12px;color:var(--text-muted);line-height:1.5;">
            No conversation messages recorded yet for this session. Submit a prompt in the bottom bar to start the conversation thread.
          </p>
        </div>
      `;
      return;
    }

    if (pane.querySelector("[data-empty-chat]")) {
      pane.innerHTML = "";
    }

    // Remove any optimistic or superseded message nodes no longer in state.chatThread
    const validIds = new Set(state.chatThread.map((m) => String(m.id)));
    pane.querySelectorAll("article[data-msg-id]").forEach((el) => {
      if (!validIds.has(el.dataset.msgId)) {
        el.remove();
      }
    });

    for (const msg of state.chatThread) {
      const msgId = String(msg.id);
      const roleClass = CHAT_ROLE_CLASSES[msg.role] || "assistant";

      let article = Array.from(pane.querySelectorAll("article[data-msg-id]")).find(
        (el) => el.dataset.msgId === msgId
      );

      if (!article) {
        article = document.createElement("article");
        article.className = `chat-bubble ${roleClass}`;
        article.dataset.msgId = msgId;
        if (roleClass === "assistant") {
          const pColor = PROVIDER_COLORS[msg.provider] || "var(--terracotta)";
          article.style.borderLeftColor = pColor;
        }
        const modelTag = msg.model
          ? `<span style="font-family:var(--font-mono);font-size:10px;color:var(--text-muted);">• ${escapeHtml(
              msg.model
            )}</span>`
          : "";

        article.innerHTML = `
          <div class="chat-meta">
            <div>
              <span class="chat-sender">${escapeHtml(msg.sender)}</span>
              ${modelTag}
            </div>
            <span class="chat-meta-status"></span>
          </div>
          <div class="chat-trace-slot"></div>
          <div class="chat-task-slot"></div>
          <div class="chat-body-slot"></div>
          <div class="chat-question-slot"></div>
        `;
        pane.appendChild(article);
      }

      // 1. Update status badge / timestamp in place
      const statusSlot = article.querySelector(".chat-meta-status");
      const desiredStatusHtml = msg.streaming
        ? `<span class="badge running" style="font-size:9.5px;padding:2px 7px;">● ACTIVE</span>`
        : `<span>${escapeHtml(msg.timestamp || "")}</span>`;
      if (statusSlot && statusSlot.innerHTML !== desiredStatusHtml) {
        statusSlot.innerHTML = desiredStatusHtml;
      }

      // 2. Update collapsed Traceability drawer in place (preserving open/closed state)
      const traceSlot = article.querySelector(".chat-trace-slot");
      const traceParts = buildTraceabilityDrawerParts(msg);
      if (traceSlot) {
        if (!traceParts) {
          if (traceSlot.innerHTML !== "") traceSlot.innerHTML = "";
        } else {
          let detailsEl = traceSlot.querySelector("details.chat-trace-drawer");
          if (!detailsEl) {
            traceSlot.innerHTML = `
              <details class="chat-trace-drawer" data-trace-id="${escapeAttr(
                msgId
              )}" data-trace-sig="${escapeAttr(traceParts.sig)}">
                <summary class="chat-trace-summary">${traceParts.summaryHtml}</summary>
                <div class="chat-trace-body">
                  <div class="chat-trace-steps-slot">${traceParts.stepsHtml}</div>
                  <div class="chat-trace-thinking-slot">${traceParts.thinkingHtml}</div>
                </div>
              </details>
            `;
          } else if (detailsEl.dataset.traceSig !== traceParts.sig) {
            detailsEl.dataset.traceSig = traceParts.sig;
            const sumEl = detailsEl.querySelector(".chat-trace-summary");
            if (sumEl) sumEl.innerHTML = traceParts.summaryHtml;
            const stepsSlot = detailsEl.querySelector(".chat-trace-steps-slot");
            if (stepsSlot) stepsSlot.innerHTML = traceParts.stepsHtml;
            const thinkSlot = detailsEl.querySelector(".chat-trace-thinking-slot");
            if (thinkSlot) {
              const existingThinkDetails = thinkSlot.querySelector("details.chat-thinking");
              const wasThinkOpen = existingThinkDetails ? existingThinkDetails.open : false;
              thinkSlot.innerHTML = traceParts.thinkingHtml;
              if (wasThinkOpen) {
                const newThinkDetails = thinkSlot.querySelector("details.chat-thinking");
                if (newThinkDetails) newThinkDetails.open = true;
              }
            }
          }
        }
      }

      // 3. Update In-Place Transient Active Task Indicator (appears while working, disappears when done)
      const taskSlot = article.querySelector(".chat-task-slot");
      if (taskSlot) {
        if (msg.streaming) {
          const task = describeActiveTask(msg);
          let liveBar = taskSlot.querySelector(".chat-active-task-live");
          if (!liveBar) {
            // Create the 3-dot wave once so its animation runs continuously without resetting
            taskSlot.innerHTML = `
              <div class="chat-active-task-live" role="status" aria-live="polite">
                <span class="streaming-dots" aria-hidden="true"><span></span><span></span><span></span></span>
                <div class="active-task-content" data-task-key="${escapeAttr(task.key)}">
                  <span class="active-task-verb">${escapeHtml(task.verb)}</span>
                  <code class="active-task-tool-code">${escapeHtml(task.toolName)}</code>
                  <span class="active-task-target" title="${escapeAttr(task.target)}">${escapeHtml(
              task.target
            )}</span>
                </div>
                <button type="button" class="inline-stop-task-btn" data-stop-msg="${escapeAttr(
                  msgId
                )}" title="Stop this execution (Esc)">⏹ Stop</button>
              </div>
            `;
            taskSlot.querySelector(".inline-stop-task-btn")?.addEventListener("click", (ev) => {
              ev.stopPropagation();
              cancelActiveExecution(msgId);
            });
          } else {
            const contentEl = liveBar.querySelector(".active-task-content");
            if (contentEl && contentEl.dataset.taskKey !== task.key) {
              const nextContent = document.createElement("div");
              nextContent.className = "active-task-content";
              nextContent.dataset.taskKey = task.key;
              nextContent.innerHTML = `
                <span class="active-task-verb">${escapeHtml(task.verb)}</span>
                <code class="active-task-tool-code">${escapeHtml(task.toolName)}</code>
                <span class="active-task-target" title="${escapeAttr(task.target)}">${escapeHtml(
                task.target
              )}</span>
              `;
              contentEl.replaceWith(nextContent);
            }
          }
        } else if (taskSlot.innerHTML !== "") {
          // Task completed: disappear in place!
          taskSlot.innerHTML = "";
        }
      }

      // 4. Update message output body in place
      const bodySlot = article.querySelector(".chat-body-slot");
      if (bodySlot) {
        const rawContent = msg.content || "";
        const streamFlag = String(Boolean(msg.streaming));
        if (!rawContent) {
          if (bodySlot.innerHTML !== "") {
            bodySlot.innerHTML = "";
            bodySlot.dataset.rawContent = "";
            bodySlot.dataset.streaming = streamFlag;
          }
        } else if (
          bodySlot.dataset.rawContent !== rawContent ||
          bodySlot.dataset.streaming !== streamFlag
        ) {
          bodySlot.dataset.rawContent = rawContent;
          bodySlot.dataset.streaming = streamFlag;
          bodySlot.innerHTML = `<div class="chat-content">${renderMarkdown(rawContent)}${
            msg.streaming ? `<span class="streaming-cursor" aria-hidden="true"></span>` : ""
          }</div>`;
        }
      }

      // 5. Update Interactive Question Card (`ask_question`) in place
      const questionSlot = article.querySelector(".chat-question-slot");
      renderInteractiveQuestionSlot(questionSlot, msg);
    }

    if (scrollToBottom && state.activeTab === "chat") {
      const scrollBody = document.getElementById("drawer-scroll-body");
      if (scrollBody) {
        requestAnimationFrame(() => {
          scrollBody.scrollTop = scrollBody.scrollHeight;
        });
      }
    }
  }

  function renderAgentTab() {
    const pane = document.getElementById("pane-agent");
    const agent = state.agents.find((a) => a.agent_id === state.selectedAgentId);
    if (!agent) {
      pane.innerHTML = `<div class="card"><p style="color:var(--text-muted);font-size:12px;">Click any agent node on the canvas to inspect its live telemetry.</p></div>`;
      return;
    }

    const pColor = PROVIDER_COLORS[agent.provider] || "#C86D51";
    const approveBtnHtml =
      agent.status === "AwaitingToolApproval"
        ? `<button class="action-btn amber" id="approve-tool-btn" type="button">⚡ APPROVE TOOL EXECUTION</button>`
        : agent.status === "AwaitingUserInput"
        ? `<button class="action-btn amber" id="jump-to-question-btn" type="button">❓ ANSWER INTERACTIVE QUESTION IN CHAT ➔</button>`
        : "";

    const toolsHtml = (agent.tool_calls || [])
      .map(
        (tc) => `
        <div class="file-row">
          <div>
            <div style="font-size:12px;font-weight:700;color:var(--text-main);">${escapeHtml(tc.name)}</div>
            <div style="font-size:11px;color:var(--text-muted);">${escapeHtml(tc.summary)}</div>
          </div>
          <span class="badge ${statusBadgeClass(tc.status)}">${escapeHtml(tc.status)}</span>
        </div>`
      )
      .join("");

    const outputsHtml = (agent.outputs || [])
      .map((out) => `<div class="mono-box md-rendered" style="white-space:normal;">${renderMarkdown(out)}</div>`)
      .join("");

    const de = agent.decision_eval;
    const decisionEvalHtml = de
      ? `<div class="card" style="border-color:var(--moss);background:var(--moss-tint);">
          <div class="card-header">
            <span class="card-title" style="color:var(--moss-strong);">🎯 JEV SYSTEM-1 TYPED PRIMITIVES</span>
            <span class="badge ${de.gate_status === "ALLOW_EXECUTION" ? "completed" : "awaiting"}">${escapeHtml(de.gate_status)} • ${de.latency_ms}ms</span>
          </div>
          <div style="display:grid;grid-template-columns:repeat(3,1fr);gap:6px;margin-top:4px;">
            <div style="background:var(--bg-panel);padding:8px;border-radius:8px;border:1px solid var(--border);">
              <div style="font-size:10px;color:var(--text-muted);">NOUL (Risk)</div>
              <div style="font-size:14px;font-weight:800;color:var(--moss-strong);">${de.noul_risk_prob.toFixed(2)}</div>
            </div>
            <div style="background:var(--bg-panel);padding:8px;border-radius:8px;border:1px solid var(--border);">
              <div style="font-size:10px;color:var(--text-muted);">CHOICE (Route)</div>
              <div style="font-size:11.5px;font-weight:800;color:var(--terracotta-strong);">${escapeHtml(de.choice_action)}</div>
            </div>
            <div style="background:var(--bg-panel);padding:8px;border-radius:8px;border:1px solid var(--border);">
              <div style="font-size:10px;color:var(--text-muted);">CONFIDENCE</div>
              <div style="font-size:14px;font-weight:800;color:#8F641E;">${(de.confidence * 100).toFixed(0)}%</div>
            </div>
          </div>
          <div style="font-size:11px;color:var(--text-muted);margin-top:4px;">
            Rubric Score: <strong style="color:var(--text-main);">${escapeHtml(de.score_rubric)}</strong>
          </div>
        </div>`
      : "";

    const isTaskNode = agent.node_kind === "task";
    const isSubagentNode = agent.node_kind === "subagent";
    const kindBadgeLabel = isTaskNode
      ? "⚡ RUNNING TASK"
      : isSubagentNode
      ? "🌿 ACTIVE SUBAGENT"
      : agent.node_kind === "gate"
      ? "🎯 DECISION GATE"
      : "🌳 LEAD AGENT";

    pane.innerHTML = `
      <div class="card ${agent.is_council_winner ? "winner-card" : ""}" style="border-left: 3.5px solid ${
      isTaskNode ? "#181816" : pColor
    };">
        <div class="card-header">
          <div>
            <div style="display:flex;align-items:center;gap:6px;margin-bottom:2px;">
              <span class="badge ${isTaskNode ? "running" : isSubagentNode ? "completed" : "awaiting"}" style="font-size:9.5px;padding:1px 6px;">${kindBadgeLabel}</span>
            </div>
            <div class="card-title">${escapeHtml(agent.role)}</div>
            <div style="font-size:11px;color:var(--text-muted);font-family:var(--font-mono);">
              ${escapeHtml(agent.agent_id)} • ${escapeHtml(agent.provider.toUpperCase())} (${escapeHtml(agent.model)})
            </div>
          </div>
          <span class="badge ${statusBadgeClass(agent.status)}">${escapeHtml(agent.status)}</span>
        </div>
        ${approveBtnHtml}
        <div style="display:flex;gap:8px;flex-wrap:wrap;font-size:11px;color:var(--text-muted);">
          <span>Step: <strong style="color:var(--text-main)">${agent.execution_step}</strong></span>
          ${
            isTaskNode
              ? `<span>•</span><span>Lifecycle: <strong style="color:var(--terracotta-strong)">Auto-clears on completion</strong></span>`
              : `<span>•</span>
                 <span>In: <strong style="color:var(--terracotta-strong)">${agent.tokens?.input_tokens || 0}</strong></span>
                 <span>Cached: <strong style="color:var(--moss-strong)">${agent.tokens?.cache_read_tokens || 0}</strong></span>
                 <span>Out: <strong style="color:#8F641E">${agent.tokens?.output_tokens || 0}</strong></span>`
          }
        </div>
      </div>

      ${decisionEvalHtml}

      <div class="card">
        <div class="card-title" style="color:var(--terracotta-strong);">${
          isTaskNode ? "TASK COMMAND / INVOCATION" : "PROMPT"
        }</div>
        <div class="mono-box">${escapeHtml(agent.prompt)}</div>
      </div>

      ${
        agent.thinking
          ? `<div class="card">
              <div class="card-title" style="color:var(--plum);">REASONING / THINKING</div>
              <div class="mono-box">${escapeHtml(agent.thinking)}</div>
            </div>`
          : ""
      }

      ${
        outputsHtml
          ? `<div class="card">
              <div class="card-title" style="color:var(--moss-strong);">AGENT OUTPUT</div>
              ${outputsHtml}
            </div>`
          : ""
      }

      <div class="card">
        <div class="card-title" style="color:#8F641E;">TOOL CALLS (${(agent.tool_calls || []).length})</div>
        ${toolsHtml || `<div style="font-size:11.5px;color:var(--text-muted);">No tool calls recorded.</div>`}
      </div>

      <div class="card">
        <div class="card-title">CONTEXT WINDOW</div>
        <div class="mono-box">${escapeHtml(agent.context)}</div>
      </div>
    `;

    const approveBtn = document.getElementById("approve-tool-btn");
    if (approveBtn) {
      approveBtn.addEventListener("click", () => {
        sendCommand({ command: "APPROVE_TOOL_EXECUTION", agent_id: agent.agent_id });
      });
    }

    const jumpQBtn = document.getElementById("jump-to-question-btn");
    if (jumpQBtn) {
      jumpQBtn.addEventListener("click", () => {
        switchTab("chat");
        requestAnimationFrame(() => {
          const card = document.querySelector(".interactive-question-card");
          if (card) card.scrollIntoView({ behavior: "smooth", block: "center" });
        });
      });
    }
  }

  function renderCouncilBanner() {
    if (!state.council) {
      councilStageBanner.classList.add("hidden");
      return;
    }
    councilStageBanner.classList.remove("hidden");
    const c = state.council;
    const stageLabels = {
      STAGE_1_PROPOSALS: `🏛️ Stage 1/3: Parallel Proposals across ${c.participants.length} AIs...`,
      STAGE_2_NEGOTIATION: "⚔️ Stage 2/3: Cross-Critique & Peer Negotiation...",
      STAGE_3_VOTING: "🗳️ Stage 3/3: Tallying Ranked-Choice Borda Ballots...",
    };
    councilBannerText.textContent =
      stageLabels[c.stage] || `🏆 Council Winner: ${c.winning_provider || "Consensus Reached"}`;
  }

  function renderCouncilTab() {
    const pane = document.getElementById("pane-council");
    const c = state.council;
    if (!c) {
      pane.innerHTML = `
        <div class="card">
          <div class="card-title" style="color:var(--terracotta-strong);">🏛️ Multi-AI Council Mode</div>
          <p style="font-size:12px;color:var(--text-muted);line-height:1.5;">
            Convene multiple AI providers (Antigravity, Claude, Codex, DeepSeek, Kimi, Copilot) plus the Jev System-1 Decision Gate.
            They draft independent proposals in parallel, critique each other in Stage 2, and cast Ranked-Choice Borda ballots (with zero self-voting) in Stage 3.
          </p>
          <button class="action-btn amber" id="start-demo-council-drawer" type="button">
            🏛️ LAUNCH COUNCIL SESSION NOW
          </button>
        </div>
      `;
      document
        .getElementById("start-demo-council-drawer")
        ?.addEventListener("click", () => {
          dispatchUserPrompt({
            promptText: `Design the highest-throughput zero-copy architecture and mobile companion workflow for ${state.activeProject}`,
            isCouncil: true,
          });
        });
      return;
    }

    const leaderboardRows = (c.leaderboard || [])
      .map(
        (row, idx) => `
        <tr style="${row.is_winner ? "color:#8F641E;font-weight:700;" : ""}">
          <td>${row.is_winner ? "🏆 #1" : `#${idx + 1}`}</td>
          <td>${escapeHtml(row.provider_name)}</td>
          <td>${row.borda_points.toFixed(1)}</td>
          <td style="color:var(--moss-strong);">+${(row.jev_gate_bonus || 0).toFixed(2)}</td>
          <td>${row.weighted_score.toFixed(2)}</td>
        </tr>`
      )
      .join("");

    const ballotsHtml = (c.votes || [])
      .map(
        (v) => `
        <div class="file-row" style="flex-direction:column;align-items:flex-start;">
          <div style="display:flex;justify-content:space-between;width:100%;font-size:12px;font-weight:700;">
            <span style="color:var(--terracotta-strong);">${escapeHtml(v.voter_provider)}</span>
            <span style="color:var(--moss-strong);">1st: ${escapeHtml(v.first_choice)} • 2nd: ${escapeHtml(v.second_choice)}</span>
          </div>
          <div style="font-size:11px;color:var(--text-muted);">${escapeHtml(v.rationale)}</div>
        </div>`
      )
      .join("");

    const proposalsHtml = (c.proposals || [])
      .map(
        (p) => `
        <div class="card">
          <div class="card-header">
            <span class="card-title">${escapeHtml(p.provider_name)} (${escapeHtml(p.model)})</span>
            <span class="badge completed">${p.latency_ms}ms • ${escapeHtml(p.execution_mode)}</span>
          </div>
          <div class="mono-box md-rendered" style="white-space:normal;">${renderMarkdown(
            p.revised_draft || p.initial_draft
          )}</div>
          ${
            p.critique_given
              ? `<div style="font-size:11px;color:var(--plum);"><strong>Peer Critique:</strong> ${renderInlineMarkdown(
                  p.critique_given
                )}</div>`
              : ""
          }
        </div>`
      )
      .join("");

    pane.innerHTML = `
      <div class="card winner-card">
        <div class="card-header">
          <span class="card-title" style="color:#8F641E;">🏛️ ${escapeHtml(c.council_id.toUpperCase())}</span>
          <span class="badge winner">${escapeHtml(c.stage)}</span>
        </div>
        <div style="font-size:12px;color:var(--text-main);"><strong>Prompt:</strong> ${renderInlineMarkdown(c.prompt)}</div>
      </div>

      ${
        c.synthesized_output
          ? `<div class="card winner-card">
              <div class="card-title" style="color:#8F641E;">🏆 WINNING CONSENSUS SYNTHESIS</div>
              <div class="mono-box md-rendered" style="white-space:normal;">${renderMarkdown(
                c.synthesized_output
              )}</div>
            </div>`
          : ""
      }

      ${
        leaderboardRows
          ? `<div class="card">
              <div class="card-title" style="color:var(--terracotta-strong);">🗳️ RANKED-CHOICE BORDA + JEV LEADERBOARD</div>
              <table class="leaderboard-table">
                <thead>
                  <tr>
                    <th>Rank</th>
                    <th>Provider</th>
                    <th>Borda</th>
                    <th>Jev Gate</th>
                    <th>Total</th>
                  </tr>
                </thead>
                <tbody>${leaderboardRows}</tbody>
              </table>
            </div>`
          : ""
      }

      ${
        ballotsHtml
          ? `<div class="card">
              <div class="card-title" style="color:var(--moss-strong);">📜 PEER BALLOTS (NO SELF-VOTING)</div>
              ${ballotsHtml}
            </div>`
          : ""
      }

      ${proposalsHtml}
    `;
  }

  function parseLineRangeAnchor(anchorStr) {
    if (!anchorStr) return null;
    const m = String(anchorStr).match(/^#?L(\d+)(?:-L?(\d+))?$/i);
    if (!m) return null;
    const start = Number(m[1]);
    const end = m[2] ? Number(m[2]) : start;
    return { start: Math.min(start, end), end: Math.max(start, end) };
  }

  function renderMainFileViewer() {
    const container = document.getElementById("main-file-viewer-container");
    if (!container) return;

    const f = state.openedFile;
    if (!f) {
      container.innerHTML = `
        <div class="file-viewer-toolbar">
          <div class="file-viewer-title-group">
            <span class="file-viewer-icon">📄</span>
            <div>
              <div class="file-viewer-name">No File Selected</div>
              <div class="file-viewer-path">Select any file from the Files tab or click a file link in Chat</div>
            </div>
          </div>
          <div class="file-viewer-actions">
            <button type="button" class="mini-btn" id="close-main-file-btn">✕ Back to Graph</button>
          </div>
        </div>
      `;
      container.querySelector("#close-main-file-btn")?.addEventListener("click", () => {
        setFileViewerMainOpen(false);
      });
      return;
    }

    const nameLower = (f.name || "").toLowerCase();
    const isMarkdown =
      f.file_kind === "markdown" ||
      nameLower.endsWith(".md") ||
      nameLower.endsWith(".markdown") ||
      nameLower.endsWith(".mdx");
    const isImage =
      f.file_kind === "image" ||
      /\.(png|jpg|jpeg|gif|webp|svg|ico)$/i.test(nameLower);

    const icon = isMarkdown ? "📘" : isImage ? "🖼️" : "📜";
    const sizeKb = f.size_bytes
      ? `${(f.size_bytes / 1024).toFixed(1)} KB`
      : `${((f.content || "").length / 1024).toFixed(1)} KB`;
    const anchorBadge = state.openedFileLineAnchor
      ? `<span class="badge running" style="font-family:var(--font-mono);">${escapeHtml(
          state.openedFileLineAnchor
        )}</span>`
      : "";

    const modeToggleHtml = isMarkdown
      ? `<div class="file-viewer-mode-toggle" role="tablist" aria-label="Document View Mode">
          <button type="button" class="file-viewer-mode-btn ${
            state.fileViewerMode !== "raw" ? "active" : ""
          }" data-fv-mode="formatted">📖 Formatted</button>
          <button type="button" class="file-viewer-mode-btn ${
            state.fileViewerMode === "raw" ? "active" : ""
          }" data-fv-mode="raw">📝 Raw Markdown</button>
        </div>`
      : "";

    let bodyHtml = "";
    if (isImage) {
      const imgUrl = f.content && f.content.startsWith("/raw-file")
        ? f.content
        : `/raw-file?path=${encodeURIComponent(f.abs_path || f.rel_path)}`;
      bodyHtml = `
        <div class="main-doc-sheet" style="text-align:center;">
          <figure class="md-img-wrap" style="margin:0;">
            <img src="${escapeAttr(imgUrl)}" alt="${escapeAttr(f.name)}" class="md-img" style="max-block-size:68dvh;" />
            <figcaption class="md-img-caption">${escapeHtml(f.name)} • ${escapeHtml(f.rel_path)}</figcaption>
          </figure>
        </div>
      `;
    } else if (isMarkdown && state.fileViewerMode !== "raw") {
      bodyHtml = `
        <article class="main-doc-sheet md-rendered">
          ${renderMarkdown(f.content || "")}
        </article>
      `;
    } else {
      const range = parseLineRangeAnchor(state.openedFileLineAnchor);
      const rawLines = String(f.content || "").replace(/\r\n/g, "\n").split("\n");
      const rows = rawLines
        .map((lineText, idx) => {
          const lineNo = idx + 1;
          const isHl = range && lineNo >= range.start && lineNo <= range.end;
          return `<tr class="code-line-row ${isHl ? "highlighted-line" : ""}" data-line-no="${lineNo}">
            <td class="code-line-num">${lineNo}</td>
            <td class="code-line-text">${escapeHtml(lineText) || " "}</td>
          </tr>`;
        })
        .join("");
      bodyHtml = `
        <div class="code-viewer-wrap">
          <table class="code-viewer-table">
            <tbody>${rows}</tbody>
          </table>
        </div>
      `;
    }

    container.innerHTML = `
      <div class="file-viewer-toolbar">
        <div class="file-viewer-title-group">
          <span class="file-viewer-icon">${icon}</span>
          <div style="min-inline-size:0;">
            <div style="display:flex;align-items:center;gap:8px;">
              <span class="file-viewer-name">${escapeHtml(f.name)}</span>
              ${anchorBadge}
              <span class="badge completed" style="font-family:var(--font-mono);">${escapeHtml(
                sizeKb
              )}</span>
            </div>
            <div class="file-viewer-path" title="${escapeAttr(f.abs_path || f.rel_path)}">${escapeHtml(
      f.rel_path
    )}</div>
          </div>
        </div>
        <div class="file-viewer-actions">
          ${modeToggleHtml}
          <button type="button" class="mini-btn" id="fv-inject-prompt-btn" title="Reference this file in prompt composer">+ Reference in Prompt</button>
          <button type="button" class="mini-btn" id="fv-copy-btn" title="Copy file contents">📋 Copy</button>
          <button type="button" class="mini-btn primary" id="close-main-file-btn" title="Close file viewer and return to graph (Esc)">✕ Back to Graph</button>
        </div>
      </div>
      <div class="file-viewer-scroll" id="file-viewer-scroll">
        ${bodyHtml}
      </div>
    `;

    container.querySelector("#close-main-file-btn")?.addEventListener("click", () => {
      setFileViewerMainOpen(false);
    });

    container.querySelectorAll("[data-fv-mode]").forEach((btn) => {
      btn.addEventListener("click", () => {
        state.fileViewerMode = btn.dataset.fvMode || "formatted";
        renderMainFileViewer();
      });
    });

    container.querySelector("#fv-inject-prompt-btn")?.addEventListener("click", () => {
      if (promptInput) setPromptValue(`@${f.rel_path} ` + promptInput.value, true);
    });

    container.querySelector("#fv-copy-btn")?.addEventListener("click", (e) => {
      const btn = e.currentTarget;
      navigator.clipboard?.writeText(f.content || "").then(() => {
        btn.textContent = "✓ Copied";
        setTimeout(() => {
          btn.textContent = "📋 Copy";
        }, 1400);
      });
    });

    // If opened with a #L... line anchor, scroll the highlighted line into view
    if (state.openedFileLineAnchor) {
      requestAnimationFrame(() => {
        const hlRow = container.querySelector(".code-line-row.highlighted-line");
        if (hlRow) {
          hlRow.scrollIntoView({ behavior: "smooth", block: "center" });
        }
      });
    }
  }

  function renderFilesTab() {
    const pane = document.getElementById("pane-files");
    const tree = state.fileTree;
    if (!tree) {
      pane.innerHTML = `<div class="card">Loading ~/code file tree...</div>`;
      return;
    }

    const activeFilePath = state.openedFile?.rel_path || state.openedFile?.abs_path || "";
    const activeFileName = state.openedFile?.name || "";

    // 1. Session Artifacts & Documents Quick-Select Card (so users can open ui_branding_redesign_plan.md etc. from Files tab)
    const sessionDocsHtml =
      state.artifacts && state.artifacts.length > 0
        ? `
        <div class="card">
          <div class="card-header">
            <div>
              <div class="card-title" style="color:var(--terracotta-strong);">📑 Session Artifacts &amp; Documents</div>
              <div style="font-size:11px;color:var(--text-muted);">Click any artifact to view formatted on Main Screen</div>
            </div>
            <span class="badge completed">${state.artifacts.length}</span>
          </div>
          <div style="display:flex;flex-direction:column;gap:5px;">
            ${state.artifacts
              .map((art) => {
                const fileName = art.path.split("/").pop() || `${art.name}.md`;
                const isCurrent =
                  activeFileName === fileName || activeFilePath === art.path;
                const isImg = /\.(jpg|jpeg|png|gif|webp|svg)$/i.test(fileName);
                return `
                  <div class="file-row clickable-file-row ${
                    isCurrent ? "active-file-row" : ""
                  }" data-open-file="${escapeAttr(art.path)}">
                    <div class="file-name">
                      <span>${isImg ? "🖼️" : "📘"}</span>
                      <span>${escapeHtml(fileName)}</span>
                    </div>
                    <div class="file-actions">
                      <button type="button" class="mini-btn ${
                        isCurrent ? "primary" : ""
                      }" data-open-file="${escapeAttr(art.path)}">📖 Main Screen</button>
                    </div>
                  </div>
                `;
              })
              .join("")}
          </div>
        </div>`
        : "";

    const pathLabel = tree.current_rel_path ? `~/code/${tree.current_rel_path}` : "~/code (Root)";

    const rowsHtml = (tree.entries || [])
      .map((entry) => {
        if (entry.is_dir) {
          const icon = entry.is_project ? "🌿" : "📁";
          return `
            <div class="file-row">
              <div class="file-name">
                <span>${icon}</span>
                <span>${escapeHtml(entry.name)}</span>
                ${entry.is_project ? `<span class="badge completed">PROJECT</span>` : ""}
              </div>
              <div class="file-actions">
                ${
                  entry.is_project
                    ? `<button class="mini-btn primary" data-set-proj="${escapeAttr(entry.rel_path)}">Set Project</button>`
                    : ""
                }
                <button class="mini-btn" data-browse="${escapeAttr(entry.rel_path)}">Open ➔</button>
              </div>
            </div>`;
        } else {
          const kb = (entry.size_bytes / 1024).toFixed(1);
          const isCurrent = activeFilePath === entry.rel_path || activeFileName === entry.name;
          const isMd = /\.(md|markdown|mdx)$/i.test(entry.name);
          const isImg = /\.(jpg|jpeg|png|gif|webp|svg)$/i.test(entry.name);
          const fIcon = isMd ? "📘" : isImg ? "🖼️" : "📄";
          return `
            <div class="file-row clickable-file-row ${
              isCurrent ? "active-file-row" : ""
            }" data-open-file="${escapeAttr(entry.rel_path)}">
              <div class="file-name">${fIcon} ${escapeHtml(entry.name)}</div>
              <div class="file-actions">
                <span style="font-size:10.5px;color:var(--text-muted);">${kb} KB</span>
                <button type="button" class="mini-btn ${
                  isCurrent ? "primary" : ""
                }" data-open-file="${escapeAttr(entry.rel_path)}">📖 View on Main</button>
              </div>
            </div>`;
        }
      })
      .join("");

    pane.innerHTML = `
      ${sessionDocsHtml}
      <div class="card">
        <div class="card-header">
          <div>
            <div class="card-title">${escapeHtml(pathLabel)}</div>
            <div style="font-size:11px;color:var(--terracotta-strong);">Workspace Files • Click any file to view on Main Screen</div>
          </div>
          <button class="mini-btn" id="up-dir-btn" ${tree.can_navigate_up ? "" : "disabled"}>⬆ Up</button>
        </div>
      </div>
      <div style="display:flex;flex-direction:column;gap:6px;">${rowsHtml}</div>
    `;

    document.getElementById("up-dir-btn")?.addEventListener("click", () => {
      const cur = tree.current_rel_path || "";
      const idx = cur.lastIndexOf("/");
      const parent = idx >= 0 ? cur.slice(0, idx) : "";
      sendCommand({ command: "BROWSE_DIR", path: parent });
    });

    pane.querySelectorAll("[data-browse]").forEach((btn) => {
      btn.addEventListener("click", () => {
        sendCommand({ command: "BROWSE_DIR", path: btn.dataset.browse });
      });
    });

    pane.querySelectorAll("[data-open-file]").forEach((el) => {
      el.addEventListener("click", (e) => {
        e.stopPropagation();
        const target = el.dataset.openFile;
        if (target) {
          openFileOnMainScreen(target);
        }
      });
    });

    pane.querySelectorAll("[data-set-proj]").forEach((btn) => {
      btn.addEventListener("click", () => {
        sendCommand({ command: "SET_PROJECT", path: btn.dataset.setProj });
      });
    });
  }

  function renderArtifactsTab() {
    const pane = document.getElementById("pane-artifacts");
    if (!state.artifacts.length) {
      pane.innerHTML = `<div class="card"><p style="font-size:12px;color:var(--text-muted);">No artifacts in this session.</p></div>`;
      return;
    }

    pane.innerHTML = state.artifacts
      .map((art) => {
        const fileName = art.path.split("/").pop() || `${art.name}.md`;
        return `
        <div class="card">
          <div class="card-header">
            <a href="#" class="card-title md-link" data-open-file-path="${escapeAttr(
              art.path
            )}" style="color:var(--terracotta-strong);">📘 ${escapeHtml(fileName)}</a>
            <div style="display:flex;align-items:center;gap:6px;">
              <button type="button" class="mini-btn primary" data-open-art-main="${escapeAttr(
                art.path
              )}">📖 Open on Main Screen</button>
              <span class="badge ${
                art.approved ? "completed" : art.request_feedback ? "awaiting" : ""
              }">
                ${
                  art.approved
                    ? "APPROVED ✅"
                    : art.request_feedback
                    ? "NEEDS FEEDBACK ⚡"
                    : "ARTIFACT"
                }
              </span>
            </div>
          </div>
          <div style="font-size:12px;color:var(--text-muted);">${escapeHtml(art.summary)}</div>
          ${
            art.request_feedback && !art.approved
              ? `<button class="action-btn emerald" data-approve-art="${escapeAttr(
                  art.name
                )}">✅ APPROVE ARTIFACT</button>`
              : ""
          }
        </div>`;
      })
      .join("");

    pane.querySelectorAll("[data-open-art-main]").forEach((btn) => {
      btn.addEventListener("click", () => {
        if (btn.dataset.openArtMain) {
          openFileOnMainScreen(btn.dataset.openArtMain);
        }
      });
    });

    pane.querySelectorAll("[data-approve-art]").forEach((btn) => {
      btn.addEventListener("click", () => {
        sendCommand({
          command: "APPROVE_ARTIFACT",
          session_id: state.activeSessionId,
          artifact_name: btn.dataset.approveArt,
        });
      });
    });
  }

  function renderProvidersTab() {
    const pane = document.getElementById("pane-providers");
    pane.innerHTML = state.providers
      .map(
        (p) => `
        <div class="card" style="border-left: 3.5px solid ${escapeAttr(p.color)};">
          <div class="card-header">
            <span class="card-title">${p.icon} ${escapeHtml(p.name)}</span>
            <span class="badge ${p.model_class === "DECISION_SYSTEM_1" ? "completed" : p.mode.startsWith("LIVE") ? "completed" : "awaiting"}">
              ${escapeHtml(p.model_class === "DECISION_SYSTEM_1" ? "SYSTEM-1 DECISION" : p.mode)}
            </span>
          </div>
          <div style="font-size:11.5px;color:var(--text-muted);line-height:1.45;">
            Specialty: <strong style="color:var(--terracotta-strong);">${escapeHtml(p.task_specialty || "")}</strong><br/>
            Default Model: <strong style="color:var(--text-main);">${escapeHtml(p.default_model)}</strong><br/>
            Runtime Mode: <code>${escapeHtml(p.cli_path || p.mode)}</code>
          </div>
          <div style="display:flex;gap:6px;">
            <button class="mini-btn primary" data-run-provider="${escapeAttr(p.id)}">
              ${p.model_class === "DECISION_SYSTEM_1" ? "🎯 Evaluate System-1 Gate" : "⚡ Run Provider"}
            </button>
          </div>
        </div>`
      )
      .join("");

    pane.querySelectorAll("[data-run-provider]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const pid = btn.dataset.runProvider;
        if (!pid) return;
        keysDialog?.close();
        dispatchUserPrompt({
          promptText: `Perform rapid architectural & health check on ${state.activeProject}`,
          isCouncil: false,
          provider: pid,
        });
      });
    });
  }

  // =========================================================================
  // 5. Warm Organic HTML5 Canvas2D Node Graph (Variant A, B, C Surfaces)
  // =========================================================================
  function resizeCanvas() {
    const dpr = window.devicePixelRatio || 1;
    const w = graphViewport.clientWidth;
    const h = graphViewport.clientHeight;
    if (canvas.width !== w * dpr || canvas.height !== h * dpr) {
      canvas.width = w * dpr;
      canvas.height = h * dpr;
    }
    requestDraw();
  }

  window.addEventListener("resize", resizeCanvas);

  function screenToWorld(sx, sy) {
    return {
      x: (sx - state.panX) / state.zoom,
      y: (sy - state.panY) / state.zoom,
    };
  }

  function getNodeDimensions(agent) {
    if (agent && agent.node_kind === "task") {
      return { w: 204, h: 66, halfW: 102, halfH: 33 };
    }
    if (agent && agent.node_kind === "subagent") {
      return { w: 210, h: 94, halfW: 105, halfH: 47 };
    }
    return { w: 214, h: 100, halfW: 107, halfH: 50 };
  }

  function findNodeAt(wx, wy) {
    for (let i = state.agents.length - 1; i >= 0; i--) {
      const a = state.agents[i];
      const { halfW, halfH } = getNodeDimensions(a);
      if (
        wx >= a.x - halfW &&
        wx <= a.x + halfW &&
        wy >= a.y - halfH &&
        wy <= a.y + halfH
      ) {
        return a;
      }
    }
    return null;
  }

  function beginCanvasPointer(clientX, clientY, autoSwitchTab) {
    const rect = canvas.getBoundingClientRect();
    const sx = clientX - rect.left;
    const sy = clientY - rect.top;
    const { x: wx, y: wy } = screenToWorld(sx, sy);
    const hit = findNodeAt(wx, wy);
    if (hit) {
      state.selectedAgentId = hit.agent_id;
      state.draggingNode = hit;
      renderAgentTab();
      if (autoSwitchTab && state.activeTab !== "agent" && state.activeTab !== "council") {
        switchTab("agent");
      }
    } else {
      state.isDraggingCanvas = true;
    }
    state.dragStartX = sx;
    state.dragStartY = sy;
    requestDraw();
  }

  function moveCanvasPointer(clientX, clientY) {
    if (!state.draggingNode && !state.isDraggingCanvas) return;
    const rect = canvas.getBoundingClientRect();
    const sx = clientX - rect.left;
    const sy = clientY - rect.top;
    const dx = sx - state.dragStartX;
    const dy = sy - state.dragStartY;
    state.dragStartX = sx;
    state.dragStartY = sy;
    if (state.draggingNode) {
      state.draggingNode.x += dx / state.zoom;
      state.draggingNode.y += dy / state.zoom;
    } else if (state.isDraggingCanvas) {
      state.panX += dx;
      state.panY += dy;
    }
    requestDraw();
  }

  canvas.addEventListener("mousedown", (e) => beginCanvasPointer(e.clientX, e.clientY, true));
  window.addEventListener("mousemove", (e) => moveCanvasPointer(e.clientX, e.clientY));
  window.addEventListener("mouseup", () => {
    state.draggingNode = null;
    state.isDraggingCanvas = false;
  });

  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const sx = e.clientX - rect.left;
      const sy = e.clientY - rect.top;
      const zoomFactor = e.deltaY < 0 ? 1.09 : 0.91;
      const nextZoom = Math.min(2.2, Math.max(0.42, state.zoom * zoomFactor));

      state.panX = sx - ((sx - state.panX) / state.zoom) * nextZoom;
      state.panY = sy - ((sy - state.panY) / state.zoom) * nextZoom;
      state.zoom = nextZoom;
      requestDraw();
    },
    { passive: false }
  );

  canvas.addEventListener(
    "touchstart",
    (e) => {
      if (e.touches.length === 2) {
        const dx = e.touches[0].clientX - e.touches[1].clientX;
        const dy = e.touches[0].clientY - e.touches[1].clientY;
        state.pinchStartDist = Math.hypot(dx, dy);
        state.pinchStartZoom = state.zoom;
        state.draggingNode = null;
        state.isDraggingCanvas = false;
      } else if (e.touches.length === 1) {
        beginCanvasPointer(e.touches[0].clientX, e.touches[0].clientY, false);
      }
    },
    { passive: true }
  );

  canvas.addEventListener(
    "touchmove",
    (e) => {
      if (e.touches.length === 2 && state.pinchStartDist) {
        const dx = e.touches[0].clientX - e.touches[1].clientX;
        const dy = e.touches[0].clientY - e.touches[1].clientY;
        const dist = Math.hypot(dx, dy);
        state.zoom = Math.min(
          2.2,
          Math.max(0.42, state.pinchStartZoom * (dist / state.pinchStartDist))
        );
        requestDraw();
      } else if (e.touches.length === 1) {
        moveCanvasPointer(e.touches[0].clientX, e.touches[0].clientY);
      }
    },
    { passive: true }
  );

  canvas.addEventListener("touchend", () => {
    state.draggingNode = null;
    state.isDraggingCanvas = false;
    state.pinchStartDist = null;
  });

  function hasActiveAnimations() {
    return state.agents.some(
      (a) =>
        a.node_kind === "task" ||
        a.status === "Running" ||
        a.status === "Negotiating" ||
        a.status === "Voting" ||
        a.status === "AwaitingToolApproval" ||
        a.status === "AwaitingUserInput"
    );
  }

  function requestDraw() {
    if (state.canvasPaused || state.animFrameId) return;
    state.animFrameId = requestAnimationFrame(drawCanvasFrame);
  }

  function drawTaskOrbitalSpinner(cx, cy, t, accentColor) {
    ctx.save();
    ctx.translate(cx, cy);

    // Outer subtle track
    ctx.strokeStyle = "rgba(247, 244, 235, 0.16)";
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.arc(0, 0, 11, 0, Math.PI * 2);
    ctx.stroke();

    // Rotating dual orbital arcs
    ctx.rotate(t * 2.8);
    ctx.strokeStyle = accentColor;
    ctx.lineWidth = 2.2;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.arc(0, 0, 11, 0, Math.PI * 0.65);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(0, 0, 11, Math.PI, Math.PI * 1.65);
    ctx.stroke();

    // Counter-rotating inner gear ring
    ctx.rotate(-t * 4.6);
    ctx.strokeStyle = "#F7F4EB";
    ctx.lineWidth = 1.4;
    ctx.beginPath();
    ctx.arc(0, 0, 6.2, 0.2, Math.PI * 0.8);
    ctx.stroke();

    // Center live core
    ctx.fillStyle = accentColor;
    ctx.beginPath();
    ctx.arc(0, 0, 2.6, 0, Math.PI * 2);
    ctx.fill();

    ctx.restore();
  }

  function drawCanvasFrame(timestamp) {
    state.animFrameId = null;
    if (state.canvasPaused) return;

    const dpr = window.devicePixelRatio || 1;
    const width = canvas.width / dpr;
    const height = canvas.height / dpr;
    const t = timestamp * 0.001;

    ctx.save();
    ctx.scale(dpr, dpr);

    // 1. Trellis Linen / Warm Bone (#F7F4EB) Surface + Perpendicular Hairline Lattice (0.5px #7D796F at 20% opacity)
    ctx.fillStyle = "#F7F4EB";
    ctx.fillRect(0, 0, width, height);
    const step = 64 * state.zoom;
    if (step >= 20) {
      const startX = ((state.panX % step) + step) % step;
      const startY = ((state.panY % step) + step) % step;

      ctx.strokeStyle = "rgba(125, 121, 111, 0.20)";
      ctx.lineWidth = 0.5;
      ctx.beginPath();
      for (let x = startX; x < width; x += step) {
        ctx.moveTo(x, 0);
        ctx.lineTo(x, height);
      }
      for (let y = startY; y < height; y += step) {
        ctx.moveTo(0, y);
        ctx.lineTo(width, y);
      }
      ctx.stroke();

      // Subtle lattice tie-knots at grid intersections
      ctx.strokeStyle = "rgba(125, 121, 111, 0.38)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      for (let x = startX; x < width; x += step) {
        for (let y = startY; y < height; y += step) {
          ctx.moveTo(x - 2.5, y);
          ctx.lineTo(x + 2.5, y);
          ctx.moveTo(x, y - 2.5);
          ctx.lineTo(x, y + 2.5);
        }
      }
      ctx.stroke();
    }

    // Apply Pan & Zoom transform
    ctx.translate(state.panX, state.panY);
    ctx.scale(state.zoom, state.zoom);

    const posMap = new Map();
    for (const a of state.agents) {
      posMap.set(a.agent_id, a);
    }

    // 2. Draw Orthogonal Espalier Branch Connectors (Central Vertical Spine + Symmetrical Horizontal Tiers)
    for (const agent of state.agents) {
      if (!agent.parent_id) continue;
      const parent = posMap.get(agent.parent_id);
      if (!parent) continue;

      const parentDim = getNodeDimensions(parent);
      const childDim = getNodeDimensions(agent);

      const x1 = parent.x;
      const y1 = parent.y + parentDim.halfH;
      const x2 = agent.x;
      const y2 = agent.y - childDim.halfH;
      const cy = (y1 + y2) * 0.5;

      const isTaskNode = agent.node_kind === "task";
      const isActive =
        isTaskNode ||
        agent.status === "Running" ||
        agent.status === "Negotiating" ||
        agent.status === "Voting";

      ctx.save();
      if (isTaskNode) {
        // Dashed animated conduit stem for running tasks
        ctx.setLineDash([5, 4]);
        ctx.lineDashOffset = -t * 20;
        ctx.strokeStyle = "#A8202A";
        ctx.lineWidth = 1.75;
      } else {
        ctx.strokeStyle = agent.is_council_winner
          ? "#A8202A"
          : isActive
          ? "#A8202A"
          : "#2D4A3E";
        ctx.lineWidth = agent.is_council_winner ? 2.6 : 1.9;
      }

      // Orthogonal Espalier stem with 10px rounded elbows
      const elbow = Math.min(10, Math.abs(x2 - x1) * 0.45, Math.abs(y2 - y1) * 0.25);
      ctx.beginPath();
      ctx.moveTo(x1, y1);
      if (Math.abs(x2 - x1) < 2) {
        ctx.lineTo(x2, y2);
      } else {
        ctx.arcTo(x1, cy, x2, cy, elbow);
        ctx.arcTo(x2, cy, x2, y2, elbow);
        ctx.lineTo(x2, y2);
      }
      ctx.stroke();
      ctx.restore();

      // Pomegranate / Laurel bud node on the espalier horizontal tier junction
      ctx.fillStyle = isActive || agent.is_council_winner ? "#A8202A" : "#2D4A3E";
      ctx.beginPath();
      ctx.arc(x2, cy, isTaskNode ? 2.8 : 3.2, 0, Math.PI * 2);
      ctx.fill();

      // Active Pomegranate Crimson pulse along the espalier branch
      if (isActive || agent.is_council_winner) {
        const speed = isTaskNode ? 1.15 : 0.72;
        const u = (t * speed) % 1.0;
        let px = x1;
        let py = y1;
        if (u < 0.35) {
          const seg = u / 0.35;
          px = x1;
          py = y1 + (cy - y1) * seg;
        } else if (u < 0.7) {
          const seg = (u - 0.35) / 0.35;
          px = x1 + (x2 - x1) * seg;
          py = cy;
        } else {
          const seg = (u - 0.7) / 0.3;
          px = x2;
          py = cy + (y2 - cy) * seg;
        }

        ctx.fillStyle = isTaskNode ? "#D9414D" : "#A8202A";
        ctx.beginPath();
        ctx.arc(px, py, isTaskNode ? 3.5 : 4.2, 0, Math.PI * 2);
        ctx.fill();
      }
    }

    // 3. Draw Cross-Council Peer Lattice Tier during Stage 2 Negotiation
    const negotiatingNodes = state.agents.filter(
      (a) => a.status === "Negotiating" || a.status === "Voting"
    );
    if (negotiatingNodes.length > 1) {
      ctx.save();
      ctx.setLineDash([6, 5]);
      ctx.lineDashOffset = -t * 22;
      ctx.strokeStyle = "rgba(168, 32, 42, 0.55)";
      ctx.lineWidth = 1.6;
      ctx.beginPath();
      for (let i = 0; i < negotiatingNodes.length - 1; i++) {
        const n1 = negotiatingNodes[i];
        const n2 = negotiatingNodes[i + 1];
        ctx.moveTo(n1.x, n1.y);
        ctx.lineTo(n2.x, n2.y);
      }
      ctx.stroke();
      ctx.restore();
    }

    // 4. Render Nodes: Visually Distinct Running Task Capsules vs Subagent Nodes vs Lead Agent Cards
    for (const agent of state.agents) {
      const dim = getNodeDimensions(agent);
      const cardW = dim.w;
      const cardH = dim.h;
      const x = agent.x - cardW / 2;
      const y = agent.y - cardH / 2;
      const isSelected = agent.agent_id === state.selectedAgentId;
      const pColor = PROVIDER_COLORS[agent.provider] || "#A8202A";

      // -----------------------------------------------------------------------
      // A. RUNNING TASK / DAEMON CAPSULE NODE (node_kind === "task")
      // Dark Basalt (#181816 / #262420) Stadium Pill with Live Orbital Spinner
      // -----------------------------------------------------------------------
      if (agent.node_kind === "task") {
        const pillRadius = cardH / 2; // 33px full stadium pill
        const isDaemonTask =
          (agent.model || "").includes("daemon") || (agent.role || "").includes("♾️");
        const taskAccent = isDaemonTask ? "#4E9F82" : "#E04F5A";

        // Subtle breathing outer ring
        const pulse = 3.5 + Math.sin(t * 4.5) * 2.2;
        ctx.strokeStyle = isDaemonTask
          ? "rgba(78, 159, 130, 0.34)"
          : "rgba(168, 32, 42, 0.36)";
        ctx.lineWidth = 1.6;
        ctx.beginPath();
        ctx.roundRect(
          x - pulse,
          y - pulse,
          cardW + pulse * 2,
          cardH + pulse * 2,
          pillRadius + pulse
        );
        ctx.stroke();

        // Dark Basalt / Espresso capsule body
        ctx.save();
        ctx.shadowColor = "rgba(24, 24, 22, 0.22)";
        ctx.shadowBlur = 14;
        ctx.shadowOffsetY = 4;
        ctx.fillStyle = isSelected ? "#141311" : "#211F1C";
        ctx.beginPath();
        ctx.roundRect(x, y, cardW, cardH, pillRadius);
        ctx.fill();
        ctx.restore();

        // Capsule border
        ctx.strokeStyle = isSelected ? "#F7F4EB" : taskAccent;
        ctx.lineWidth = isSelected ? 2.2 : 1.5;
        ctx.beginPath();
        ctx.roundRect(x, y, cardW, cardH, pillRadius);
        ctx.stroke();

        // Left Rotating Orbital / Gear Spinner
        drawTaskOrbitalSpinner(x + 27, y + cardH / 2, t, taskAccent);

        // Top badge label: DAEMON TASK vs RUNNING TASK
        ctx.fillStyle = taskAccent;
        ctx.font = "700 9px JetBrains Mono, SF Mono, Menlo, monospace";
        const kindLabel = isDaemonTask ? "DAEMON TASK" : "RUNNING TASK";
        ctx.fillText(kindLabel, x + 48, y + 20);

        // Step pill on top-right
        ctx.fillStyle = "#A39E93";
        ctx.font = "600 9px JetBrains Mono, SF Mono, Menlo, monospace";
        ctx.fillText(`S${agent.execution_step}`, x + cardW - 38, y + 20);

        // Task title in Warm Bone (#F7F4EB)
        ctx.fillStyle = "#F7F4EB";
        ctx.font = "700 11px Inter, -apple-system, BlinkMacSystemFont, sans-serif";
        const cleanTitle = (agent.role || "Running Task").replace(/^[⚡♾️⚙]\s*/, "");
        const roleTitle = cleanTitle.length > 20 ? cleanTitle.slice(0, 19) + "…" : cleanTitle;
        ctx.fillText(roleTitle, x + 48, y + 36);

        // Command / telemetry preview in muted bone monospace
        ctx.fillStyle = "#BDB7AA";
        ctx.font = "500 9.5px JetBrains Mono, SF Mono, Menlo, monospace";
        const cmdPreview =
          (agent.recent_log || "").length > 23
            ? agent.recent_log.slice(0, 22) + "…"
            : agent.recent_log || "";
        ctx.fillText(cmdPreview, x + 48, y + 51);
        continue;
      }

      // -----------------------------------------------------------------------
      // B. AGENT / ACTIVE SUBAGENT / DECISION GATE ARCHITECTURAL CARDS
      // -----------------------------------------------------------------------
      const isSubagent = agent.node_kind === "subagent";
      const radius = isSubagent ? 12 : 8;

      // Active Pomegranate Crimson (#A8202A) or Burnt Umber (#9E472A) pulse halo
      if (
        agent.status === "Running" ||
        agent.status === "Negotiating" ||
        agent.status === "AwaitingToolApproval" ||
        agent.status === "AwaitingUserInput"
      ) {
        const pulse = 5 + Math.sin(t * 4.0) * 3.5;
        ctx.strokeStyle =
          agent.status === "AwaitingToolApproval"
            ? "rgba(158, 71, 42, 0.45)"
            : "rgba(168, 32, 42, 0.38)";
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.roundRect(x - pulse, y - pulse, cardW + pulse * 2, cardH + pulse * 2, radius + 3);
        ctx.stroke();
      }

      // Soft warm drop shadow + Warm Bone card body
      ctx.save();
      ctx.shadowColor = "rgba(38, 36, 32, 0.09)";
      ctx.shadowBlur = 16;
      ctx.shadowOffsetY = 5;
      ctx.fillStyle = isSelected ? "#FCFAF4" : isSubagent ? "#F3F6F4" : "#F7F4EB";
      ctx.beginPath();
      ctx.roundRect(x, y, cardW, cardH, radius);
      ctx.fill();
      ctx.restore();

      // Header band: Deep Laurel tint for active Subagents vs Weathered Trellis for Lead Agent
      ctx.fillStyle = isSubagent ? "#E1EBE6" : "#EFECE1";
      ctx.beginPath();
      ctx.roundRect(x, y, cardW, 30, { tl: radius, tr: radius, br: 0, bl: 0 });
      ctx.fill();

      // Card border (dashed Laurel/Crimson for ephemeral Subagent, solid for Lead Agent)
      ctx.save();
      if (isSubagent && !isSelected) {
        ctx.setLineDash([5, 3]);
      }
      ctx.strokeStyle = agent.is_council_winner
        ? "#A8202A"
        : isSelected
        ? pColor
        : isSubagent
        ? "#2D4A3E"
        : "#D5CFC1";
      ctx.lineWidth = isSelected || agent.is_council_winner ? 2.2 : isSubagent ? 1.5 : 1.2;
      ctx.beginPath();
      ctx.roundRect(x, y, cardW, cardH, radius);
      ctx.stroke();
      ctx.restore();

      // Trellis Brand Status Indicator Dot
      let dotColor = "#4A6B5D";
      if (agent.status === "Running" || agent.status === "Negotiating" || agent.status === "Voting") {
        dotColor = "#A8202A";
      } else if (agent.status === "Completed" || agent.status === "DecisionGate" || agent.status === "Winner") {
        dotColor = "#2D4A3E";
      } else if (agent.status === "AwaitingToolApproval" || agent.status === "Errored") {
        dotColor = "#9E472A";
      }

      ctx.fillStyle = dotColor;
      ctx.beginPath();
      ctx.arc(x + 17, y + 16, 4.5, 0, Math.PI * 2);
      ctx.fill();

      // Role title in Espresso Trunk (#262420)
      ctx.fillStyle = "#262420";
      ctx.font = "700 12px Inter, -apple-system, BlinkMacSystemFont, sans-serif";
      const roleTitle = agent.role.length > 19 ? agent.role.slice(0, 18) + "…" : agent.role;
      ctx.fillText(roleTitle, x + 28, y + 20);

      // Step or Score pill on top-right
      ctx.fillStyle = agent.is_council_winner
        ? "#A8202A"
        : isSubagent
        ? "#2D4A3E"
        : "#7D796F";
      ctx.font = "600 10px JetBrains Mono, SF Mono, Menlo, monospace";
      const rightTag =
        agent.council_score != null
          ? `${agent.council_score.toFixed(1)}pt`
          : isSubagent
          ? "SUB"
          : `S${agent.execution_step}`;
      ctx.fillText(rightTag, x + cardW - 40, y + 19);

      // Provider + Model line
      ctx.fillStyle = isSubagent ? "#2D4A3E" : pColor;
      ctx.font = "700 10.5px JetBrains Mono, SF Mono, Menlo, monospace";
      const modelTag = `${agent.provider.toUpperCase()} • ${agent.model}`;
      ctx.fillText(
        modelTag.length > 27 ? modelTag.slice(0, 26) + "…" : modelTag,
        x + 16,
        y + 49
      );

      // Recent log line in Espresso Trunk
      ctx.fillStyle = "#262420";
      ctx.font = "400 11px Inter, -apple-system, BlinkMacSystemFont, sans-serif";
      const logPreview =
        (agent.recent_log || "").length > 29
          ? agent.recent_log.slice(0, 28) + "…"
          : agent.recent_log || "";
      ctx.fillText(logPreview, x + 16, y + 68);

      // Token & status footer in Weathered Trellis (#7D796F)
      ctx.fillStyle = "#7D796F";
      ctx.font = "500 10px JetBrains Mono, SF Mono, Menlo, monospace";
      const totalTok =
        (agent.tokens?.input_tokens || 0) + (agent.tokens?.output_tokens || 0);
      ctx.fillText(
        `${agent.status.toUpperCase()} • ${(totalTok / 1000).toFixed(1)}k tok`,
        x + 16,
        y + 85
      );
    }

    ctx.restore();

    if (hasActiveAnimations()) {
      requestDraw();
    }
  }

  // =========================================================================
  // 7. Developer Lifecycle Cockpit: Git Diff, Anchor Tests, Deploy & Pipeline
  // =========================================================================
  const LIFECYCLE_SLASH_CATALOG = [
    {
      cmd: "/greenfield-architecture",
      stage: "📐 Plan",
      desc: "Day-1 architecture discovery, 8-pillar specification & deep-module decomposition",
      buildPrompt: () =>
        `/greenfield-architecture Analyze ~/code/${state.activeProject} and design the next architectural specification and deep-module interface.`,
    },
    {
      cmd: "/tdd",
      stage: "🔴🟢 TDD",
      desc: "Red-Green-Refactor test-first vertical slice construction",
      buildPrompt: () =>
        `/tdd Build the next vertical slice in ~/code/${state.activeProject} using strict Red-Green-Refactor test-driven development.`,
    },
    {
      cmd: "/bushog",
      stage: "🐗 Debug",
      desc: "Parallel root-cause bug clearing with red-to-green deterministic test anchors",
      buildPrompt: () =>
        `/bushog Diagnose and clear current defects or failing tests in ~/code/${state.activeProject} with red-to-green test anchors.`,
    },
    {
      cmd: "/code-review",
      stage: "🔍 Review",
      desc: "Dual-axis Standards & Spec code review of current git diff / branch",
      buildPrompt: () => {
        const changedCount = state.devops?.git?.changed_files?.length || 0;
        const branch = state.devops?.git?.branch || "HEAD";
        return `/code-review Review the current changes on branch '${branch}' (${changedCount} changed files) in ~/code/${state.activeProject} along both Standards and Spec axes.`;
      },
    },
    {
      cmd: "/survey",
      stage: "📋 Audit PR",
      desc: "Spec-traceable PR compilation and zero-trust adversarial verification",
      buildPrompt: () =>
        `/survey Perform a zero-trust adversarial audit and compile a spec-traceable PR report for ~/code/${state.activeProject}.`,
    },
    {
      cmd: "/deforestation",
      stage: "🪓 Refactor",
      desc: "Multi-agent codebase modernization and elimination of architectural smells",
      buildPrompt: () =>
        `/deforestation Audit ~/code/${state.activeProject} for architectural smells and modernize shallow modules into deep modules.`,
    },
    {
      cmd: "/bonsai",
      stage: "🪴 Bonsai",
      desc: "Surgical code pruning, 12->3 line compression & 1MB->10KB dependency diet",
      buildPrompt: () =>
        `/bonsai Audit ~/code/${state.activeProject} using the 5 Pruning Shears to compress ceremony, eliminate dead code, and prune heavy dependencies.`,
    },
    {
      cmd: "/seed-catalog",
      stage: "📖 Catalog",
      desc: "Ubiquitous language (GLOSSARY.md), semantic rename proposals & verified core docs",
      buildPrompt: () =>
        `/seed-catalog Extract the canonical domain & 5-Tier seam vocabulary into GLOSSARY.md, propose semantic renames, and verify core documentation for ~/code/${state.activeProject}.`,
    },
    {
      cmd: "/prod",
      stage: "🚀 Deploy",
      desc: "Production release compilation, adversarial review & Change Ticket dashboard",
      buildPrompt: () =>
        `/prod Prepare a production release verification and Change Ticket for ~/code/${state.activeProject}.`,
    },
    {
      cmd: "/tidy",
      stage: "🧹 Tidy",
      desc: "Post-approval release finalization: PR merge, issue closure & deploy check",
      buildPrompt: () =>
        `/tidy Finalize approved PRs, clean up branches, and verify post-merge state for ~/code/${state.activeProject}.`,
    },
    {
      cmd: "/health",
      stage: "🩺 Monitor",
      desc: "Deterministic platform, application, database & endpoint health verification",
      buildPrompt: () =>
        `/health Run a deterministic health probe across endpoints, build artifacts, and services for ~/code/${state.activeProject}.`,
    },
    {
      cmd: "/database-hygiene",
      stage: "🗄️ DB Hygiene",
      desc: "Database integrity, orphan remediation, and schema drift audit",
      buildPrompt: () =>
        `/database-hygiene Audit relational integrity, schema drift, and migration readiness for ~/code/${state.activeProject}.`,
    },
  ];

  function triggerLifecycleWorkflow(promptText, autoSubmit = false) {
    if (!promptInput) return;
    document.getElementById("slash-cmd-popover")?.classList.add("hidden");
    if (autoSubmit) {
      setPromptValue("");
      dispatchUserPrompt({
        promptText,
        isCouncil: state.executionMode === "council",
      });
      return;
    }
    setPromptValue(promptText, true);
  }

  document.querySelectorAll("#lifecycle-pipeline-dock [data-slash-cmd]").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      const cmd = btn.dataset.slashCmd;
      const entry = LIFECYCLE_SLASH_CATALOG.find((x) => x.cmd === cmd);
      const text = entry ? entry.buildPrompt() : `${cmd} on ~/code/${state.activeProject}`;
      // Shift+click or normal click: populate composer and focus so user can refine or press Enter, or auto-submit if shiftKey
      triggerLifecycleWorkflow(text, e.shiftKey);
    });
  });

  function renderSlashCmdPopover(query) {
    const pop = document.getElementById("slash-cmd-popover");
    if (!pop) return;
    const trimmed = String(query || "").trim();
    if (!trimmed.startsWith("/") || trimmed.includes(" ")) {
      pop.classList.add("hidden");
      return;
    }
    const q = trimmed.toLowerCase();
    const matches = LIFECYCLE_SLASH_CATALOG.filter(
      (item) =>
        item.cmd.toLowerCase().includes(q) ||
        item.stage.toLowerCase().includes(q) ||
        item.desc.toLowerCase().includes(q)
    );
    if (!matches.length) {
      pop.classList.add("hidden");
      return;
    }
    pop.classList.remove("hidden");
    pop.innerHTML = matches
      .map(
        (m) => `
        <button type="button" class="slash-cmd-item" data-pick-slash="${escapeAttr(m.cmd)}">
          <div style="display:flex;align-items:center;justify-content:space-between;gap:6px;">
            <code style="font-family:var(--font-mono);font-size:11.5px;font-weight:800;color:var(--terracotta-strong);">${escapeHtml(
              m.cmd
            )}</code>
            <span style="font-size:10px;font-weight:700;color:var(--moss-strong);">${escapeHtml(
              m.stage
            )}</span>
          </div>
          <div style="font-size:10.5px;color:var(--text-muted);line-height:1.35;">${escapeHtml(
            m.desc
          )}</div>
        </button>
      `
      )
      .join("");

    pop.querySelectorAll("[data-pick-slash]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const cmd = btn.dataset.pickSlash;
        const entry = LIFECYCLE_SLASH_CATALOG.find((x) => x.cmd === cmd);
        if (entry) {
          triggerLifecycleWorkflow(entry.buildPrompt(), false);
        }
      });
    });
  }

  promptInput?.addEventListener("input", () => {
    renderSlashCmdPopover(promptInput.value);
  });

  function renderTopbarDevOpsStrip() {
    const gitBtn = document.getElementById("top-git-diff-btn");
    const testBtn = document.getElementById("top-test-gate-btn");
    const deployBtn = document.getElementById("top-deploy-health-btn");
    if (!gitBtn || !testBtn || !deployBtn) return;

    const devops = state.devops;
    gitBtn.classList.toggle(
      "active-workbench-tab",
      state.devopsMainOpen && state.devopsActiveTab === "diff"
    );
    testBtn.classList.toggle(
      "active-workbench-tab",
      state.devopsMainOpen && state.devopsActiveTab === "tests"
    );
    deployBtn.classList.toggle(
      "active-workbench-tab",
      state.devopsMainOpen && state.devopsActiveTab === "deploy"
    );

    if (!devops) return;

    // 1. Git Diff Pill
    const git = devops.git || {};
    const branch = git.branch || "main";
    const fileCount = (git.changed_files || []).length;
    const totalAdd = git.total_additions ?? git.total_insertions ?? 0;
    const totalDel = git.total_deletions ?? 0;
    gitBtn.classList.remove("clean", "dirty", "failed");
    if (git.is_clean) {
      gitBtn.classList.add("clean");
      gitBtn.textContent = `🔀 ${branch} • clean`;
    } else {
      gitBtn.classList.add("dirty");
      gitBtn.textContent = `🔀 ${branch} • ±${fileCount} (+${totalAdd}/-${totalDel})`;
    }

    // 2. Anchor Test Gate Pill
    const tests = devops.tests || {};
    testBtn.classList.remove("clean", "passed", "dirty", "running", "failed");
    if (tests.status === "Running") {
      testBtn.classList.add("running");
      testBtn.textContent = `⏳ Tests: Running…`;
    } else if (tests.status === "Passed") {
      testBtn.classList.add("passed");
      testBtn.textContent = `🧪 ✓ ${tests.passed_count || 0} Passed`;
    } else if (tests.status === "Failed") {
      testBtn.classList.add("failed");
      testBtn.textContent = `🧪 ✕ ${tests.failed_count || 1} Failed`;
    } else {
      testBtn.textContent = `🧪 Tests (${tests.framework || "cargo"})`;
    }

    // 3. Deploy & Health Pill
    const dep = devops.deploy_health || devops.deploy || {};
    const healthyEndpoints = (dep.endpoints || []).filter(
      (e) => e.ok || e.status === "HEALTHY" || e.status_code === 200
    ).length;
    const totalEndpoints = (dep.endpoints || []).length;
    deployBtn.classList.remove("clean", "passed", "dirty", "failed");
    if (dep.release_ready) {
      deployBtn.classList.add("passed");
      deployBtn.textContent = `🚀 Ready • ${healthyEndpoints}/${totalEndpoints} UP`;
    } else {
      deployBtn.classList.add("dirty");
      deployBtn.textContent = `🚀 Hold • ${healthyEndpoints}/${totalEndpoints} UP`;
    }
  }

  function formatUnifiedDiffHtml(diffText) {
    const raw = String(diffText || "");
    if (!raw.trim()) {
      return `<div style="padding:24px;color:#A39E93;font-family:var(--font-mono);font-size:12px;">✓ Working tree is clean — no uncommitted changes in this scope.</div>`;
    }
    const lines = raw.replace(/\r\n/g, "\n").split("\n");
    return lines
      .map((line) => {
        let cls = "diff-line";
        if (line.startsWith("+++") || line.startsWith("---") || line.startsWith("diff --git") || line.startsWith("index ")) {
          cls += " diff-line-meta";
        } else if (line.startsWith("@@")) {
          cls += " diff-line-hunk";
        } else if (line.startsWith("+")) {
          cls += " diff-line-add";
        } else if (line.startsWith("-")) {
          cls += " diff-line-del";
        }
        return `<div class="${cls}">${escapeHtml(line)}</div>`;
      })
      .join("");
  }

  function renderMainDevOpsWorkbench() {
    const container = document.getElementById("main-devops-container");
    if (!container) return;

    const devops = state.devops || {
      project_id: state.activeProject,
      git: {
        branch: "main",
        is_clean: true,
        ahead: 0,
        behind: 0,
        changed_files: [],
        total_insertions: 0,
        total_deletions: 0,
        recent_commits: [],
      },
      tests: {
        framework: "cargo",
        command: "cargo test",
        status: "Idle",
        passed_count: 0,
        failed_count: 0,
        skipped_count: 0,
        duration_ms: 0,
        last_run_at: "",
        output_tail: "",
      },
      deploy: {
        gh_available: false,
        open_pr_number: null,
        open_pr_title: null,
        open_pr_url: null,
        ci_runs: [],
        endpoints: [],
        release_ready: false,
        release_summary: "Probing workspace telemetry…",
      },
    };

    const tab = state.devopsActiveTab || "diff";
    const git = devops.git || {};
    const tests = devops.tests || {};
    const deploy = devops.deploy_health || devops.deploy || {};
    const changedFiles = git.changed_files || [];
    const totalAdd = git.total_additions ?? git.total_insertions ?? 0;
    const totalDel = git.total_deletions ?? 0;

    let bodyHtml = "";
    if (tab === "diff") {
      const diffPayload = state.gitDiff;
      const diffText =
        diffPayload && diffPayload.project_id === state.activeProject
          ? diffPayload.unified_diff || diffPayload.diff_text || ""
          : "";
      const activeFile = state.selectedDiffFile || null;

      bodyHtml = `
        <div class="devops-workbench-split">
          <div style="display:flex;flex-direction:column;gap:14px;">
            <div class="devops-sidebar-card">
              <div style="display:flex;align-items:center;justify-content:space-between;">
                <span style="font-family:var(--font-display);font-size:11.5px;font-weight:800;letter-spacing:0.06em;text-transform:uppercase;color:var(--text-main);">
                  🔀 Working Tree (${changedFiles.length})
                </span>
                <button type="button" class="icon-btn" id="devops-all-diff-btn" style="font-size:10.5px;padding:3px 8px;">
                  All Files
                </button>
              </div>
              <div style="font-family:var(--font-mono);font-size:11px;color:var(--text-muted);">
                Branch: <strong style="color:var(--text-main);">${escapeHtml(
                  git.branch || "main"
                )}</strong> • <span class="diff-stat-add">+${totalAdd}</span> / <span class="diff-stat-del">-${totalDel}</span>
              </div>
              <div style="display:flex;flex-direction:column;gap:5px;max-block-size:290px;overflow-y:auto;">
                ${
                  changedFiles.length === 0
                    ? `<div style="font-size:11.5px;color:var(--moss-strong);padding:8px;background:var(--moss-tint);border-radius:8px;">✓ Working tree is clean. Showing latest HEAD commit diff on right.</div>`
                    : changedFiles
                        .map((f) => {
                          const stCode = f.status || "M";
                          const stClass =
                            stCode === "A" || stCode === "??"
                              ? "add"
                              : stCode === "D"
                              ? "del"
                              : "mod";
                          const isSel = activeFile === f.path;
                          const fAdd = f.additions ?? f.insertions ?? 0;
                          const fDel = f.deletions ?? 0;
                          return `
                            <div class="diff-file-item ${isSel ? "active" : ""}" data-diff-file="${escapeAttr(
                            f.path
                          )}">
                              <div style="display:flex;align-items:center;gap:7px;min-inline-size:0;">
                                <span class="diff-status-code ${stClass}">${escapeHtml(stCode)}</span>
                                <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="${escapeAttr(
                                  f.path
                                )}">${escapeHtml(f.path)}</span>
                              </div>
                              <div style="display:flex;align-items:center;gap:5px;flex-shrink:0;">
                                <span class="diff-stat-add">+${fAdd}</span>
                                <span class="diff-stat-del">-${fDel}</span>
                              </div>
                            </div>
                          `;
                        })
                        .join("")
                }
              </div>
            </div>

            <div class="devops-sidebar-card">
              <div style="font-family:var(--font-display);font-size:11.5px;font-weight:800;letter-spacing:0.06em;text-transform:uppercase;color:var(--text-main);">
                📜 Recent Commits
              </div>
              <div style="display:flex;flex-direction:column;gap:6px;">
                ${(git.recent_commits || [])
                  .map(
                    (c) => `
                    <div style="padding:6px 8px;background:var(--bg-card);border:1px solid var(--border-subtle);border-radius:8px;font-size:11px;">
                      <div style="display:flex;align-items:center;justify-content:space-between;gap:6px;">
                        <code style="color:var(--terracotta-strong);font-weight:700;">${escapeHtml(
                          c.sha || c.short_hash || ""
                        )}</code>
                        <span style="font-size:10px;color:var(--text-muted);">${escapeHtml(
                          c.relative_time
                        )}</span>
                      </div>
                      <div style="color:var(--text-main);font-weight:600;margin-top:2px;">${escapeHtml(
                        c.subject
                      )}</div>
                    </div>
                  `
                  )
                  .join("")}
              </div>
            </div>
          </div>

          <div class="diff-viewer-box">
            <div class="diff-viewer-header">
              <div>
                <span style="color:#E2C07D;font-weight:700;">${
                  activeFile ? `FILE DIFF: ${escapeHtml(activeFile)}` : "UNIFIED WORKSPACE DIFF"
                }</span>
              </div>
              <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
                ${
                  activeFile
                    ? `<button type="button" class="icon-btn" id="devops-open-full-file-btn" style="background:#2D4A3E;color:#FFFDF9;border-color:#4E9F82;font-size:11px;">📄 Open Full File</button>`
                    : ""
                }
                <button type="button" class="icon-btn" id="devops-dispatch-review-btn" style="background:#A8202A;color:#FFFDF9;border-color:#E04F5A;font-size:11px;">
                  🔍 Review Diff (/code-review)
                </button>
                <button type="button" class="icon-btn" id="devops-dispatch-survey-btn" style="background:#262420;color:#F7F4EB;border-color:#7D796F;font-size:11px;">
                  📋 Compile PR Audit (/survey)
                </button>
              </div>
            </div>
            <div class="diff-lines-container">
              ${formatUnifiedDiffHtml(diffText)}
            </div>
          </div>
        </div>
      `;
    } else if (tab === "tests") {
      const stColor =
        tests.status === "Passed"
          ? "#2D4A3E"
          : tests.status === "Failed"
          ? "#A8202A"
          : tests.status === "Running"
          ? "#9E6B20"
          : "#7D796F";
      bodyHtml = `
        <div style="max-inline-size:1020px;margin:0 auto;display:flex;flex-direction:column;gap:16px;">
          <div class="devops-sidebar-card" style="flex-direction:row;align-items:center;justify-content:space-between;flex-wrap:wrap;padding:16px 20px;">
            <div style="display:flex;flex-direction:column;gap:4px;">
              <div style="display:flex;align-items:center;gap:10px;">
                <span style="font-family:var(--font-display);font-size:15px;font-weight:800;color:var(--text-main);">
                  🧪 Deterministic Anchor Test Gate
                </span>
                <span class="badge" style="background:${stColor};color:#FFFDF9;font-size:11px;padding:3px 10px;">
                  ${escapeHtml(tests.status || "Idle")}
                </span>
              </div>
              <div style="font-family:var(--font-mono);font-size:12px;color:var(--text-muted);">
                Framework: <strong>${escapeHtml(
                  tests.framework || "cargo"
                )}</strong> • Command: <code>${escapeHtml(
        tests.command || "cargo test"
      )}</code> ${
        tests.duration_ms ? `• Duration: <strong>${tests.duration_ms}ms</strong>` : ""
      } ${tests.last_run_at ? `• Last run: ${escapeHtml(tests.last_run_at)}` : ""}
              </div>
            </div>
            <div style="display:flex;align-items:center;gap:10px;flex-wrap:wrap;">
              <button type="button" class="send-btn" id="devops-run-tests-btn" style="min-block-size:40px;padding:0 18px;font-size:12.5px;" ${
                tests.status === "Running" ? "disabled" : ""
              }>
                ${tests.status === "Running" ? "⏳ Running Anchor Suite…" : "▶ Run Anchor Tests Now"}
              </button>
              ${
                tests.status === "Failed"
                  ? `<button type="button" class="icon-btn" id="devops-fix-tests-btn" style="background:#A8202A;color:#FFFDF9;border-color:#A8202A;padding:8px 14px;font-weight:800;">🔧 Dispatch Agent to Fix (/bushog)</button>`
                  : ""
              }
            </div>
          </div>

          <div style="display:grid;grid-template-columns:repeat(3, 1fr);gap:12px;">
            <div class="devops-sidebar-card" style="border-left:4px solid #2D4A3E;">
              <div style="font-size:11px;font-weight:700;color:var(--text-muted);text-transform:uppercase;">Passed Assertions</div>
              <div style="font-family:var(--font-mono);font-size:24px;font-weight:800;color:#2D4A3E;">${
                tests.passed_count || 0
              }</div>
            </div>
            <div class="devops-sidebar-card" style="border-left:4px solid #A8202A;">
              <div style="font-size:11px;font-weight:700;color:var(--text-muted);text-transform:uppercase;">Failed Assertions</div>
              <div style="font-family:var(--font-mono);font-size:24px;font-weight:800;color:#A8202A;">${
                tests.failed_count || 0
              }</div>
            </div>
            <div class="devops-sidebar-card" style="border-left:4px solid #7D796F;">
              <div style="font-size:11px;font-weight:700;color:var(--text-muted);text-transform:uppercase;">Skipped / Ignored</div>
              <div style="font-family:var(--font-mono);font-size:24px;font-weight:800;color:var(--text-muted);">${
                tests.skipped_count || 0
              }</div>
            </div>
          </div>

          <div>
            <div style="font-family:var(--font-display);font-size:11.5px;font-weight:800;letter-spacing:0.06em;text-transform:uppercase;color:var(--text-muted);margin-bottom:6px;">
              Anchor Test Runner Output Stream
            </div>
            <div class="test-terminal-box">${escapeHtml(
              tests.output_excerpt ||
                tests.output_tail ||
                "Click '▶ Run Anchor Tests Now' to execute the deterministic test suite for ~/code/" +
                  state.activeProject +
                  ".\nWhile running, a live capsule node will appear on the Agent Graph and can be cancelled at any time with ⏹ STOP."
            )}</div>
          </div>
        </div>
      `;
    } else {
      // Deploy, CI & Health Tab
      bodyHtml = `
        <div style="max-inline-size:1020px;margin:0 auto;display:flex;flex-direction:column;gap:16px;">
          <div class="devops-sidebar-card" style="flex-direction:row;align-items:center;justify-content:space-between;flex-wrap:wrap;padding:16px 20px;border-left:4px solid ${
            deploy.release_ready ? "#2D4A3E" : "#9E6B20"
          };">
            <div>
              <div style="font-family:var(--font-display);font-size:15px;font-weight:800;color:var(--text-main);">
                ${escapeHtml(
                  deploy.release_summary ||
                    (deploy.release_ready
                      ? "✓ Release Gate READY — Working Tree Clean & Endpoints Healthy"
                      : "⚠ Release Gate HOLD — Uncommitted Changes or Pending Verification")
                )}
              </div>
              <div style="font-size:12px;color:var(--text-muted);margin-top:4px;">
                Workspace: <code>~/code/${escapeHtml(
                  state.activeProject
                )}</code> • Branch: <code>${escapeHtml(git.branch || "main")}</code> (↑${
        git.ahead || 0
      } ↓${git.behind || 0}) ${
        deploy.checked_at ? `• Checked: ${escapeHtml(deploy.checked_at)}` : ""
      }
              </div>
            </div>
            <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
              <button type="button" class="icon-btn" data-deploy-slash="/prod" style="background:#A8202A;color:#FFFDF9;border-color:#A8202A;font-weight:800;">
                🚀 Cut Release (/prod)
              </button>
              <button type="button" class="icon-btn" data-deploy-slash="/tidy" style="border-color:var(--moss);color:var(--moss-strong);font-weight:700;">
                🧹 Finalize &amp; Merge (/tidy)
              </button>
              <button type="button" class="icon-btn" data-deploy-slash="/health" style="font-weight:700;">
                🩺 Deep Health Audit (/health)
              </button>
              <button type="button" class="icon-btn" data-deploy-slash="/database-hygiene" style="font-weight:700;">
                🗄️ DB Hygiene (/database-hygiene)
              </button>
            </div>
          </div>

          <div style="display:grid;grid-template-columns:repeat(auto-fit, minmax(320px, 1fr));gap:16px;">
            <div class="devops-sidebar-card">
              <div style="font-family:var(--font-display);font-size:12px;font-weight:800;letter-spacing:0.06em;text-transform:uppercase;color:var(--text-main);">
                🩺 Live Endpoint &amp; Service Probes
              </div>
              <div style="display:flex;flex-direction:column;gap:8px;">
                ${
                  (deploy.endpoints || []).length === 0
                    ? `<div style="font-size:12px;color:var(--text-muted);">No endpoints configured.</div>`
                    : (deploy.endpoints || [])
                        .map((ep) => {
                          const isOk =
                            ep.ok || ep.status === "HEALTHY" || ep.status_code === 200;
                          return `
                          <div style="display:flex;align-items:center;justify-content:space-between;padding:10px 12px;background:var(--bg-card);border:1px solid var(--border);border-radius:10px;">
                            <div>
                              <div style="font-weight:700;font-size:12.5px;color:var(--text-main);">${escapeHtml(
                                ep.name
                              )}</div>
                              <div style="font-family:var(--font-mono);font-size:11px;color:var(--text-muted);">${escapeHtml(
                                ep.url
                              )}</div>
                            </div>
                            <div style="text-align:right;">
                              <span class="badge ${isOk ? "completed" : "awaiting"}">
                                ${isOk ? `✓ ${ep.status_code} OK` : "✕ UNREACHABLE"}
                              </span>
                              <div style="font-family:var(--font-mono);font-size:10px;color:var(--text-muted);margin-top:3px;">
                                ${ep.latency_ms}ms
                              </div>
                            </div>
                          </div>
                        `;
                        })
                        .join("")
                }
              </div>
            </div>

            <div class="devops-sidebar-card">
              <div style="font-family:var(--font-display);font-size:12px;font-weight:800;letter-spacing:0.06em;text-transform:uppercase;color:var(--text-main);">
                🐙 GitHub PR &amp; CI Workflow Telemetry
              </div>
              ${
                deploy.pr_summary || deploy.open_pr_number
                  ? `<div style="padding:10px 12px;background:var(--moss-tint);border:1px solid var(--moss);border-radius:10px;font-size:12px;">
                      <div style="font-weight:800;color:var(--moss-strong);">${escapeHtml(
                        deploy.pr_summary ||
                          `PR #${deploy.open_pr_number}: ${deploy.open_pr_title || ""}`
                      )} ${
                      deploy.pr_state ? `[${escapeHtml(deploy.pr_state)}]` : ""
                    }</div>
                    </div>`
                  : `<div style="font-size:11.5px;color:var(--text-muted);padding:8px;background:var(--bg-well);border-radius:8px;">No open GitHub PR detected for current branch.</div>`
              }
              <div style="display:flex;flex-direction:column;gap:6px;">
                ${
                  (deploy.ci_runs || []).length === 0
                    ? `<div style="font-size:11.5px;color:var(--text-muted);">No recent GitHub Actions runs found for this repo.</div>`
                    : (deploy.ci_runs || [])
                        .map(
                          (r) => `
                          <div style="display:flex;align-items:center;justify-content:space-between;padding:8px 10px;background:var(--bg-card);border:1px solid var(--border-subtle);border-radius:8px;font-size:11.5px;">
                            <div>
                              <div style="font-weight:700;color:var(--text-main);">${escapeHtml(
                                r.name
                              )}</div>
                              <div style="font-size:10px;color:var(--text-muted);">${escapeHtml(
                                r.branch
                              )} • ${escapeHtml(r.created_at)}</div>
                            </div>
                            <span class="badge ${
                              r.conclusion === "success"
                                ? "completed"
                                : r.conclusion === "failure"
                                ? "awaiting"
                                : "running"
                            }">
                              ${escapeHtml(r.conclusion || r.status)}
                            </span>
                          </div>
                        `
                        )
                        .join("")
                }
              </div>
            </div>
          </div>
        </div>
      `;
    }

    container.innerHTML = `
      <div class="devops-toolbar">
        <div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;">
          <div>
            <div style="font-family:var(--font-display);font-size:14px;font-weight:800;letter-spacing:0.05em;text-transform:uppercase;color:var(--text-main);">
              ⚙️ Developer Lifecycle Cockpit — ~/code/${escapeHtml(state.activeProject)}
            </div>
            <div style="font-size:11.5px;color:var(--text-muted);">
              Live Git Diff Review, Deterministic Anchor Test Gate, and Deployment &amp; Health Telemetry
            </div>
          </div>
          <div class="skills-segmented" role="tablist">
            <button type="button" class="skills-seg-btn ${
              tab === "diff" ? "active" : ""
            }" data-devops-tab="diff">
              🔀 Git Diff &amp; Review (${changedFiles.length})
            </button>
            <button type="button" class="skills-seg-btn ${
              tab === "tests" ? "active" : ""
            }" data-devops-tab="tests">
              🧪 Anchor Test Gate (${escapeHtml(tests.status || "Idle")})
            </button>
            <button type="button" class="skills-seg-btn ${
              tab === "deploy" ? "active" : ""
            }" data-devops-tab="deploy">
              🚀 Deploy &amp; Health
            </button>
          </div>
        </div>
        <div style="display:flex;align-items:center;gap:8px;">
          <button type="button" class="icon-btn" id="devops-refresh-btn" title="Refresh Git, Test & Health Telemetry">
            ↻ Refresh
          </button>
          <button type="button" class="icon-btn" id="close-devops-main-btn" style="border-color:var(--moss);color:var(--moss-strong);">
            🌿 Back to Agent Graph (Esc)
          </button>
        </div>
      </div>
      <div class="devops-workbench-body">
        ${bodyHtml}
      </div>
    `;

    container.querySelectorAll("[data-devops-tab]").forEach((btn) => {
      btn.addEventListener("click", () => {
        state.devopsActiveTab = btn.dataset.devopsTab || "diff";
        renderTopbarDevOpsStrip();
        renderMainDevOpsWorkbench();
      });
    });

    container.querySelector("#close-devops-main-btn")?.addEventListener("click", () => {
      setDevOpsMainOpen(false);
    });

    container.querySelector("#devops-refresh-btn")?.addEventListener("click", () => {
      sendCommand({ command: "REFRESH_DEVOPS", project_id: state.activeProject });
      sendCommand({
        command: "OPEN_GIT_DIFF",
        project_id: state.activeProject,
        path: state.selectedDiffFile || undefined,
      });
    });

    container.querySelector("#devops-all-diff-btn")?.addEventListener("click", () => {
      state.selectedDiffFile = null;
      sendCommand({ command: "OPEN_GIT_DIFF", project_id: state.activeProject });
    });

    container.querySelectorAll("[data-diff-file]").forEach((row) => {
      row.addEventListener("click", () => {
        const fp = row.dataset.diffFile;
        state.selectedDiffFile = fp || null;
        sendCommand({
          command: "OPEN_GIT_DIFF",
          project_id: state.activeProject,
          path: fp || undefined,
        });
      });
    });

    container.querySelector("#devops-open-full-file-btn")?.addEventListener("click", () => {
      if (state.selectedDiffFile) {
        openFileOnMainScreen(state.selectedDiffFile);
      }
    });

    container.querySelector("#devops-dispatch-review-btn")?.addEventListener("click", () => {
      const entry = LIFECYCLE_SLASH_CATALOG.find((x) => x.cmd === "/code-review");
      if (entry) {
        setDevOpsMainOpen(false);
        triggerLifecycleWorkflow(entry.buildPrompt(), true);
      }
    });

    container.querySelector("#devops-dispatch-survey-btn")?.addEventListener("click", () => {
      const entry = LIFECYCLE_SLASH_CATALOG.find((x) => x.cmd === "/survey");
      if (entry) {
        setDevOpsMainOpen(false);
        triggerLifecycleWorkflow(entry.buildPrompt(), true);
      }
    });

    container.querySelector("#devops-run-tests-btn")?.addEventListener("click", () => {
      sendCommand({ command: "RUN_WORKSPACE_TESTS", project_id: state.activeProject });
    });

    container.querySelector("#devops-fix-tests-btn")?.addEventListener("click", () => {
      const failTail = (
        state.devops?.tests?.output_excerpt ||
        state.devops?.tests?.output_tail ||
        ""
      ).slice(-1200);
      const prompt = `/bushog Fix the failing deterministic anchor tests in ~/code/${state.activeProject}.\n\nRecent test runner failure output:\n\`\`\`\n${failTail}\n\`\`\``;
      setDevOpsMainOpen(false);
      triggerLifecycleWorkflow(prompt, true);
    });

    container.querySelectorAll("[data-deploy-slash]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const cmd = btn.dataset.deploySlash;
        const entry = LIFECYCLE_SLASH_CATALOG.find((x) => x.cmd === cmd);
        if (entry) {
          setDevOpsMainOpen(false);
          triggerLifecycleWorkflow(entry.buildPrompt(), false);
        }
      });
    });
  }

  // =========================================================================
  // 8. "+ New Session / New Greenfield Project" Provisioning Modal
  // =========================================================================
  const newSessionBtn = document.getElementById("new-session-btn");
  const newSessionDialog = document.getElementById("new-session-dialog");
  const closeNewSessionBtn = document.getElementById("close-new-session-btn");
  const cancelNewSessionBtn = document.getElementById("cancel-new-session-btn");
  const newSessionForm = document.getElementById("new-session-form");
  const newModeExistingBtn = document.getElementById("new-mode-existing-btn");
  const newModeProjectBtn = document.getElementById("new-mode-project-btn");
  const newExistingRepoFields = document.getElementById("new-existing-repo-fields");
  const newGreenfieldProjectFields = document.getElementById("new-greenfield-project-fields");
  const newSessionRepoSelect = document.getElementById("new-session-repo-select");
  const newProjectSlugInput = document.getElementById("new-project-slug-input");
  const newProjectDescInput = document.getElementById("new-project-desc-input");
  const newProjectInitGit = document.getElementById("new-project-init-git");
  const newSessionTitleInput = document.getElementById("new-session-title-input");
  const newSessionAutoKickoff = document.getElementById("new-session-auto-kickoff");
  const newSessionError = document.getElementById("new-session-error");
  const submitNewSessionBtn = document.getElementById("submit-new-session-btn");

  function populateNewSessionRepoDropdown() {
    if (!newSessionRepoSelect) return;
    const projects =
      state.availableProjects && state.availableProjects.length > 0
        ? state.availableProjects
        : [state.activeProject || "exo-suit"];
    newSessionRepoSelect.innerHTML = "";
    for (const slug of projects) {
      const opt = document.createElement("option");
      opt.value = slug;
      opt.selected = slug === state.activeProject;
      opt.textContent = `~/code/${slug}`;
      newSessionRepoSelect.appendChild(opt);
    }
  }

  function setNewSessionDialogMode(mode) {
    state.newSessionMode = mode === "new_project" ? "new_project" : "existing_repo";
    const isNewProj = state.newSessionMode === "new_project";
    newModeExistingBtn?.classList.toggle("active", !isNewProj);
    newModeProjectBtn?.classList.toggle("active", isNewProj);
    newExistingRepoFields?.classList.toggle("hidden", isNewProj);
    newGreenfieldProjectFields?.classList.toggle("hidden", !isNewProj);
    newSessionError?.classList.add("hidden");
    if (submitNewSessionBtn) {
      submitNewSessionBtn.textContent = isNewProj
        ? "🌱 CREATE PROJECT & SESSION ➔"
        : "+ CREATE SESSION ➔";
    }
    if (isNewProj && newSessionAutoKickoff) {
      newSessionAutoKickoff.checked = true;
    }
    if (isNewProj) {
      newProjectSlugInput?.focus();
    } else {
      newSessionTitleInput?.focus();
    }
  }

  newSessionBtn?.addEventListener("click", () => {
    if (!newSessionDialog) return;
    populateNewSessionRepoDropdown();
    if (newProjectSlugInput) newProjectSlugInput.value = "";
    if (newProjectDescInput) newProjectDescInput.value = "";
    if (newSessionTitleInput) newSessionTitleInput.value = "";
    if (newProjectInitGit) newProjectInitGit.checked = true;
    if (newSessionAutoKickoff) newSessionAutoKickoff.checked = false;
    setNewSessionDialogMode("existing_repo");
    newSessionDialog.showModal();
  });

  closeNewSessionBtn?.addEventListener("click", () => {
    newSessionDialog?.close();
  });

  cancelNewSessionBtn?.addEventListener("click", () => {
    newSessionDialog?.close();
  });

  bindDialogLightDismiss(newSessionDialog);

  newModeExistingBtn?.addEventListener("click", () => {
    setNewSessionDialogMode("existing_repo");
  });

  newModeProjectBtn?.addEventListener("click", () => {
    setNewSessionDialogMode("new_project");
  });

  newSessionForm?.addEventListener("submit", (e) => {
    e.preventDefault();
    newSessionError?.classList.add("hidden");

    const isNewProj = state.newSessionMode === "new_project";
    const targetSlug = isNewProj
      ? (newProjectSlugInput?.value || "").trim()
      : (newSessionRepoSelect?.value || state.activeProject || "").trim();

    if (!targetSlug) {
      if (newSessionError) {
        newSessionError.textContent = isNewProj
          ? "⚠️ Please enter a valid project directory slug (e.g. pulse-engine)."
          : "⚠️ Please select a target repository.";
        newSessionError.classList.remove("hidden");
      }
      return;
    }

    if (isNewProj && !/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/.test(targetSlug)) {
      if (newSessionError) {
        newSessionError.textContent =
          "⚠️ Project slug must start with a letter or number and contain only letters, numbers, hyphens (-), or underscores (_).";
        newSessionError.classList.remove("hidden");
      }
      return;
    }

    const customTitle = (newSessionTitleInput?.value || "").trim();
    const projDesc = (newProjectDescInput?.value || "").trim();
    const initGit = isNewProj ? Boolean(newProjectInitGit?.checked) : false;
    const autoKickoff = Boolean(newSessionAutoKickoff?.checked);

    sendCommand({
      command: "CREATE_SESSION_OR_PROJECT",
      mode: state.newSessionMode,
      project_id: targetSlug,
      title: customTitle || undefined,
      description: projDesc || undefined,
      init_git: initGit,
      auto_kickoff: autoKickoff,
      provider: state.selectedProvider,
      model: state.selectedModel,
    });
  });

  // Initialize
  renderAtelierLeftRail();
  resizeCanvas();
  connectWebSocket();
})();
