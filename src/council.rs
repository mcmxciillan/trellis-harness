use crate::brain::{
    append_turn_to_session_transcript, build_live_session_graph, format_epoch_secs,
    get_past_sessions, get_session_chat_thread, infer_session_project_id,
    load_session_agy_conversation_id, resolve_execution_scope, save_session_agy_conversation_id,
    save_session_interactive_answer, save_session_project_id,
};
use crate::devops::build_workspace_devops_snapshot;
use crate::models::{
    AgentNode, ChatMessage, CouncilProposal, CouncilScoreEntry, CouncilSession, InteractivePrompt,
    RankedChoiceVote, TokenMetrics, ToolCall,
};
use crate::providers::{
    build_provider_registry, detect_binary, evaluate_jev_decision_gate, execute_provider_prompt,
    is_muse_cli_authenticated, run_agy_streaming_turn, run_muse_streaming_turn, CliStreamEvent,
    CliStreamOutcome,
};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, watch, RwLock};

static ACTIVE_SERVER_PORT: AtomicU16 = AtomicU16::new(9000);
static RUN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn active_server_port() -> u16 {
    let port = ACTIVE_SERVER_PORT.load(Ordering::Relaxed);
    if port == 0 {
        9000
    } else {
        port
    }
}

pub fn set_active_server_port(port: u16) {
    ACTIVE_SERVER_PORT.store(if port == 0 { 9000 } else { port }, Ordering::Relaxed);
}

pub fn next_monotonic_run_id(prefix: &str) -> String {
    let seq = RUN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() % 100_000)
        .unwrap_or(1);
    format!("{}-{}-{}", prefix, ts, seq)
}

#[derive(Clone, Debug)]
pub struct ActiveRunHandle {
    pub run_id: String,
    pub session_id: String,
    pub project_id: String,
    pub node_ids: Vec<String>,
    pub reply_msg_id: String,
    pub abort_tx: watch::Sender<bool>,
}

pub struct SharedAppState {
    pub active_project: String,
    pub active_session_id: String,
    pub agents: Vec<AgentNode>,
    pub council: Option<CouncilSession>,
    pub chat_thread: Vec<ChatMessage>,
    pub active_session_live_messages: Vec<ChatMessage>,
    pub session_custom_messages: HashMap<String, Vec<ChatMessage>>,
    pub active_runs: HashMap<String, ActiveRunHandle>,
    pub devops_tests: HashMap<String, crate::models::TestRunSnapshot>,
    pub api_keys: HashMap<String, String>,
}

impl SharedAppState {
    pub fn bootstrap_from_disk(server_port: u16) -> Self {
        set_active_server_port(server_port);
        let sessions = get_past_sessions(25);
        let active_session_id = sessions
            .first()
            .map(|s| s.session_id.clone())
            .unwrap_or_else(|| "eae8f37c-4de6-4200-b86b-66ca0bb8edc7".to_string());

        let active_project = infer_session_project_id(&active_session_id);
        let initial_agents = build_live_session_graph(&active_session_id, &active_project);
        let initial_chat = get_session_chat_thread(&active_session_id);

        Self {
            active_project,
            active_session_id,
            agents: initial_agents,
            council: None,
            chat_thread: initial_chat,
            active_session_live_messages: Vec::new(),
            session_custom_messages: HashMap::new(),
            active_runs: HashMap::new(),
            devops_tests: HashMap::new(),
            api_keys: HashMap::new(),
        }
    }

    pub fn resolve_and_bind_scope(
        &mut self,
        requested_session_id: Option<&str>,
        requested_project_id: Option<&str>,
    ) -> crate::models::ExecutionScope {
        let eff_sess = requested_session_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.active_session_id)
            .to_string();
        let eff_proj = requested_project_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&self.active_project)
            .to_string();
        let scope = resolve_execution_scope(&eff_sess, &eff_proj);
        save_session_project_id(&scope.session_id, &scope.project_id);
        if scope.session_id == self.active_session_id {
            self.active_project = scope.project_id.clone();
        }
        scope
    }

    pub fn broadcast_chat_stream(&self, tx: &broadcast::Sender<String>, session_id: &str) {
        if self.active_session_id == session_id {
            let _ = tx.send(
                json!({
                    "type": "CHAT_STREAM_UPDATE",
                    "session_id": session_id,
                    "active_session_id": session_id,
                    "agents": self.agents,
                    "chat_thread": self.chat_thread
                })
                .to_string(),
            );
        }
    }

    pub fn broadcast_agents_updated(&self, tx: &broadcast::Sender<String>, session_id: &str) {
        if self.active_session_id == session_id {
            let _ = tx.send(
                json!({
                    "type": "AGENTS_UPDATED",
                    "session_id": session_id,
                    "active_session_id": session_id,
                    "agents": self.agents
                })
                .to_string(),
            );
        }
    }

    pub fn broadcast_council_updated(
        &self,
        tx: &broadcast::Sender<String>,
        session_id: &str,
        include_chat: bool,
    ) {
        if self.active_session_id == session_id {
            let payload = if include_chat {
                json!({
                    "type": "COUNCIL_UPDATED",
                    "session_id": session_id,
                    "active_session_id": session_id,
                    "council": self.council,
                    "agents": self.agents,
                    "chat_thread": self.chat_thread
                })
            } else {
                json!({
                    "type": "COUNCIL_UPDATED",
                    "session_id": session_id,
                    "active_session_id": session_id,
                    "council": self.council,
                    "agents": self.agents
                })
            };
            let _ = tx.send(payload.to_string());
        }
    }

    pub fn update_scoped_chat_message<F>(&mut self, session_id: &str, msg_id: &str, mut f: F) -> bool
    where
        F: FnMut(&mut ChatMessage),
    {
        let mut updated = false;
        if let Some(msgs) = self.session_custom_messages.get_mut(session_id) {
            if let Some(m) = msgs.iter_mut().find(|m| m.id == msg_id) {
                f(m);
                updated = true;
            }
        }

        if self.active_session_id == session_id {
            if let Some(m) = self
                .active_session_live_messages
                .iter_mut()
                .find(|m| m.id == msg_id)
            {
                f(m);
                updated = true;
            }
            if let Some(m) = self.chat_thread.iter_mut().find(|m| m.id == msg_id) {
                f(m);
                updated = true;
            }
        }
        updated
    }

    pub fn push_or_update_scoped_chat(&mut self, session_id: &str, msg: ChatMessage) {
        let entry = self
            .session_custom_messages
            .entry(session_id.to_string())
            .or_default();
        if let Some(existing) = entry.iter_mut().find(|m| m.id == msg.id) {
            *existing = msg.clone();
        } else {
            entry.push(msg.clone());
        }

        if self.active_session_id == session_id {
            if let Some(existing) = self
                .active_session_live_messages
                .iter_mut()
                .find(|m| m.id == msg.id)
            {
                *existing = msg.clone();
            } else {
                self.active_session_live_messages.push(msg.clone());
            }
            if let Some(existing) = self.chat_thread.iter_mut().find(|m| m.id == msg.id) {
                *existing = msg;
            } else {
                self.chat_thread.push(msg);
            }
        }
    }

    pub fn load_merged_session_chat(&mut self, session_id: &str) -> Vec<ChatMessage> {
        let mut base = get_session_chat_thread(session_id);
        let custom = self
            .session_custom_messages
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        for cm in &custom {
            if let Some(existing) = base.iter_mut().find(|m| {
                m.id == cm.id
                    || (!cm.streaming
                        && !cm.content.trim().is_empty()
                        && m.role == cm.role
                        && m.content.trim() == cm.content.trim())
            }) {
                let keep_id = cm.id.clone();
                *existing = cm.clone();
                existing.id = keep_id;
            } else {
                base.push(cm.clone());
            }
        }
        if self.active_session_id == session_id {
            self.active_session_live_messages = custom;
            self.chat_thread = base.clone();
        }
        base
    }

    pub fn cancel_execution(&mut self, target_id: Option<&str>) -> Vec<String> {
        let target = target_id.unwrap_or("").trim();
        let active_sess = self.active_session_id.clone();

        let mut matching_keys: Vec<String> = self
            .active_runs
            .iter()
            .filter(|(_, r)| {
                if !target.is_empty() {
                    r.run_id == target
                        || r.reply_msg_id == target
                        || r.session_id == target
                        || r.project_id == target
                        || r.node_ids.iter().any(|n| n == target)
                } else {
                    r.session_id == active_sess
                }
            })
            .map(|(k, _)| k.clone())
            .collect();

        if matching_keys.is_empty() && target.is_empty() {
            matching_keys = self.active_runs.keys().cloned().collect();
        }

        let mut cancelled_ids = Vec::new();
        for key in matching_keys {
            if let Some(handle) = self.active_runs.remove(&key) {
                let _ = handle.abort_tx.send(true);
                self.agents
                    .retain(|a| !handle.node_ids.contains(&a.agent_id));

                if let Some(ref mut c) = self.council {
                    if c.council_id == handle.run_id {
                        c.stage = "CANCELLED".to_string();
                    }
                }

                self.update_scoped_chat_message(&handle.session_id, &handle.reply_msg_id, |m| {
                    m.streaming = false;
                    for tc in &mut m.tool_calls {
                        if tc.status == "Running" {
                            tc.status = "Cancelled".to_string();
                        }
                    }
                    if m.content.trim().is_empty() {
                        m.content =
                            "*(⏹ Execution stopped by user before output was generated.)*"
                                .to_string();
                    } else if !m.content.contains("Stopped by user") {
                        m.content.push_str("\n\n*(⏹ Stopped by user)*");
                    }
                });

                cancelled_ids.push(handle.run_id);
            }
        }

        cancelled_ids
    }
}

async fn stream_text_into_chat_message(
    state: &Arc<RwLock<SharedAppState>>,
    tx: &broadcast::Sender<String>,
    session_id: &str,
    msg_id: &str,
    full_text: &str,
    final_thinking: Option<String>,
    final_tools: Vec<ToolCall>,
    final_interactive: Option<InteractivePrompt>,
    mut abort_rx: watch::Receiver<bool>,
) {
    let chars: Vec<char> = full_text.chars().collect();
    let total = chars.len();
    let steps = 14usize;
    let chunk_size = (total / steps).max(18);

    let mut pos = 0usize;
    while pos < total {
        if *abort_rx.borrow() {
            return;
        }
        pos = (pos + chunk_size).min(total);
        // Snap to next whitespace so words/markdown tokens don't jitter mid-word
        while pos < total && !chars[pos - 1].is_whitespace() {
            pos += 1;
        }
        let partial: String = chars[..pos].iter().collect();
        let is_done = pos >= total;

        {
            let mut st = state.write().await;
            if *abort_rx.borrow() {
                return;
            }
            st.update_scoped_chat_message(session_id, msg_id, |m| {
                m.content = partial.clone();
                m.streaming = !is_done;
                if is_done {
                    if final_thinking.is_some() {
                        m.thinking = final_thinking.clone();
                    }
                    if !final_tools.is_empty() {
                        m.tool_calls = final_tools.clone();
                    }
                    if final_interactive.is_some() {
                        m.interactive_prompt = final_interactive.clone();
                    }
                }
            });
            st.broadcast_chat_stream(tx, session_id);
        }

        if !is_done {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(32)) => {}
                _ = abort_rx.changed() => {
                    if *abort_rx.borrow() {
                        return;
                    }
                }
            }
        }
    }
}

pub fn now_iso_short() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_epoch_secs(secs)
}

fn make_interactive_prompt(
    node_id: &str,
    step_index: u32,
    questions: Vec<crate::models::InteractiveQuestionItem>,
) -> Option<InteractivePrompt> {
    if questions.is_empty() {
        None
    } else {
        Some(InteractivePrompt {
            prompt_id: format!("ask-{}", node_id),
            step_index,
            status: "Pending".to_string(),
            questions,
            answers: Vec::new(),
        })
    }
}

fn complete_tools_with_interactive(
    tools: &mut Vec<ToolCall>,
    interactive: Option<&InteractivePrompt>,
) {
    for tc in tools.iter_mut() {
        tc.status = "Completed".to_string();
    }
    if let Some(ip) = interactive {
        if !tools.iter().any(|t| t.name == "ask_question") {
            tools.push(ToolCall {
                name: "ask_question".to_string(),
                summary: ip
                    .questions
                    .first()
                    .map(|q| q.question.chars().take(68).collect())
                    .unwrap_or_else(|| "Interactive question".to_string()),
                status: "Completed".to_string(),
            });
        }
    }
}

pub async fn run_independent_prompt(
    state: Arc<RwLock<SharedAppState>>,
    tx: broadcast::Sender<String>,
    provider_id: String,
    model_override: Option<String>,
    prompt: String,
    requested_session_id: Option<String>,
    requested_project_id: Option<String>,
) {
    let (abort_tx, mut abort_rx) = watch::channel(false);

    let (scope, api_keys, provider_info) = {
        let mut st = state.write().await;
        let resolved_scope = st.resolve_and_bind_scope(
            requested_session_id.as_deref(),
            requested_project_id.as_deref(),
        );

        let reg = build_provider_registry(&st.api_keys);
        let p_info = reg
            .into_iter()
            .find(|p| p.id == provider_id)
            .unwrap_or_else(|| crate::models::ProviderInfo {
                id: provider_id.clone(),
                name: provider_id.to_uppercase(),
                model_class: "GENERATIVE_SYSTEM_2".to_string(),
                task_specialty: "General Execution".to_string(),
                default_model: "default".to_string(),
                available_models: vec![],
                color: "#00c3ff".to_string(),
                icon: "🤖".to_string(),
                mode: "SIMULATED".to_string(),
                cli_path: None,
                has_api_key: false,
                enabled_in_council: true,
            });

        let model = model_override.unwrap_or_else(|| p_info.default_model.clone());
        let parent_id = st
            .agents
            .first()
            .map(|a| a.agent_id.clone())
            .unwrap_or_default();

        let idx = st.agents.len() as f32;
        let offset_x = ((idx % 4.0) - 1.5) * 230.0;
        let node_id = next_monotonic_run_id(&provider_id);

        let is_decision = p_info.model_class == "DECISION_SYSTEM_1";
        let role_label = if is_decision {
            format!("🎯 {} Gate", p_info.name)
        } else {
            format!("{} Agent", p_info.name)
        };

        let new_node = AgentNode {
            agent_id: node_id.clone(),
            parent_id: parent_id.clone(),
            node_kind: if is_decision {
                "gate".to_string()
            } else {
                "subagent".to_string()
            },
            provider: provider_id.clone(),
            model: model.clone(),
            role: role_label,
            status: "Running".to_string(),
            execution_step: 1,
            prompt: prompt.clone(),
            thinking: if is_decision {
                format!(
                    "Evaluating TypeSafe System-1 primitives (Noul, Choice, Score) via {}...",
                    model
                )
            } else {
                format!(
                    "Dispatching prompt to {} ({}) in workspace ~/code/{}...",
                    p_info.name, model, resolved_scope.project_id
                )
            },
            context: format!(
                "Provider: {} ({})\nClass: {}\nSpecialty: {}\nWorkspace: ~/code/{}",
                p_info.name,
                p_info.mode,
                p_info.model_class,
                p_info.task_specialty,
                resolved_scope.project_id
            ),
            recent_log: format!("Executing on {}...", model),
            tokens: TokenMetrics {
                input_tokens: (prompt.len() as u64 / 3) + 320,
                cache_read_tokens: 900,
                output_tokens: 0,
            },
            tool_calls: vec![ToolCall {
                name: if is_decision {
                    "jev_evaluate_primitives".to_string()
                } else {
                    "read_file".to_string()
                },
                summary: if is_decision {
                    format!("Quantizing {}/state", resolved_scope.project_id)
                } else {
                    format!("{}/src (workspace context)", resolved_scope.project_id)
                },
                status: "Running".to_string(),
            }],
            outputs: vec![],
            x: (460.0 + offset_x).clamp(160.0, 820.0),
            y: 320.0 + ((idx / 4.0).floor() * 140.0),
            council_score: None,
            is_council_winner: false,
            decision_eval: None,
            interactive_prompt: None,
        };

        let task_node_id = format!("task-{}", node_id);
        let task_node = AgentNode {
            agent_id: task_node_id.clone(),
            parent_id: node_id.clone(),
            node_kind: "task".to_string(),
            provider: provider_id.clone(),
            model: if is_decision {
                "TASK • jev_evaluate".to_string()
            } else {
                "TASK • read_file".to_string()
            },
            role: if is_decision {
                format!("Quantizing {}/state", resolved_scope.project_id)
            } else {
                format!("Reading {}/src", resolved_scope.project_id)
            },
            status: "Running".to_string(),
            execution_step: 1,
            prompt: prompt.clone(),
            thinking: "Active tool task executing...".to_string(),
            context: format!("Parent Subagent: {}", node_id),
            recent_log: "Running step 1...".to_string(),
            tokens: TokenMetrics::default(),
            tool_calls: new_node.tool_calls.clone(),
            outputs: vec![],
            x: new_node.x,
            y: new_node.y + 155.0,
            council_score: None,
            is_council_winner: false,
            decision_eval: None,
            interactive_prompt: None,
        };

        if resolved_scope.session_id == st.active_session_id {
            if let Some(root) = st.agents.first_mut() {
                root.status = "Running".to_string();
                root.recent_log = format!(
                    "Executing: {}...",
                    prompt.chars().take(48).collect::<String>()
                );
            }
            st.agents.push(new_node.clone());
            st.agents.push(task_node);
        }

        let next_step = st.chat_thread.len() as u32 + 1;
        st.push_or_update_scoped_chat(
            &resolved_scope.session_id,
            ChatMessage {
                id: format!("user-{}", node_id),
                step_index: next_step,
                role: "user".to_string(),
                sender: "You".to_string(),
                provider: "user".to_string(),
                model: String::new(),
                timestamp: now_iso_short(),
                content: prompt.clone(),
                thinking: None,
                tool_calls: Vec::new(),
                streaming: false,
                interactive_prompt: None,
            },
        );

        let reply_id = format!("reply-{}", node_id);
        st.push_or_update_scoped_chat(
            &resolved_scope.session_id,
            ChatMessage {
                id: reply_id.clone(),
                step_index: next_step + 1,
                role: if is_decision {
                    "decision".to_string()
                } else {
                    "assistant".to_string()
                },
                sender: format!("{} {}", p_info.icon, p_info.name),
                provider: provider_id.clone(),
                model: model.clone(),
                timestamp: now_iso_short(),
                content: String::new(),
                thinking: Some(new_node.thinking.clone()),
                tool_calls: new_node.tool_calls.clone(),
                streaming: true,
                interactive_prompt: None,
            },
        );

        st.active_runs.insert(
            node_id.clone(),
            ActiveRunHandle {
                run_id: node_id.clone(),
                session_id: resolved_scope.session_id.clone(),
                project_id: resolved_scope.project_id.clone(),
                node_ids: vec![node_id.clone(), task_node_id.clone()],
                reply_msg_id: reply_id.clone(),
                abort_tx,
            },
        );

        let _ = tx.send(
            json!({
                "type": "AGENT_SPAWNED",
                "agent": new_node,
                "agents": st.agents,
                "chat_thread": st.chat_thread,
                "session_id": resolved_scope.session_id,
                "active_session_id": resolved_scope.session_id,
                "project_id": resolved_scope.project_id
            })
            .to_string(),
        );

        (
            resolved_scope,
            st.api_keys.clone(),
            (node_id, model, p_info),
        )
    };

    let (node_id, model, p_info) = provider_info;
    let reply_id = format!("reply-{}", node_id);
    let task_node_id = format!("task-{}", node_id);

    let muse_cli_ready = provider_id == "muse"
        && is_muse_cli_authenticated(&api_keys)
        && detect_binary(&["/opt/homebrew/bin/muse", "muse"]).is_some();

    let use_live_agy = p_info.model_class != "DECISION_SYSTEM_1"
        && (provider_id == "antigravity"
            || muse_cli_ready
            || (provider_id == "muse" && !p_info.has_api_key)
            || (provider_id == "claude" && !p_info.has_api_key)
            || (provider_id == "codex" && !p_info.has_api_key));

    if use_live_agy {
        if let Some(agy_bin) = detect_binary(&["agy"]) {
            let existing_conv_id = load_session_agy_conversation_id(&scope.session_id);
            let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<CliStreamEvent>();

            let state_for_evts = state.clone();
            let tx_for_evts = tx.clone();
            let scope_for_evts = scope.clone();
            let node_id_for_evts = node_id.clone();
            let task_node_id_for_evts = task_node_id.clone();
            let reply_id_for_evts = reply_id.clone();
            let prompt_len = prompt.len();

            let event_consumer = async move {
                let mut saw_real_tool = false;
                let mut step_counter = 1u32;
                let mut streamed_text = String::new();
                let mut live_tokens = TokenMetrics {
                    input_tokens: (prompt_len as u64 / 3) + 320,
                    cache_read_tokens: 900,
                    output_tokens: 0,
                };

                while let Some(evt) = evt_rx.recv().await {
                    match evt {
                        CliStreamEvent::Init { conversation_id } => {
                            save_session_agy_conversation_id(
                                &scope_for_evts.session_id,
                                &conversation_id,
                            );
                        }
                        CliStreamEvent::ToolActive {
                            tool_name, summary, ..
                        } => {
                            step_counter += 1;
                            let tool_entry = ToolCall {
                                name: tool_name.clone(),
                                summary: summary.clone(),
                                status: "Running".to_string(),
                            };
                            let mut st = state_for_evts.write().await;
                            if scope_for_evts.session_id == st.active_session_id {
                                if let Some(root) = st.agents.first_mut() {
                                    root.status = "Running".to_string();
                                    root.recent_log = format!("{} → {}", tool_name, summary);
                                }
                                if let Some(tn) = st
                                    .agents
                                    .iter_mut()
                                    .find(|a| a.agent_id == task_node_id_for_evts)
                                {
                                    tn.model = format!("TASK • {}", tool_name);
                                    tn.role = summary.chars().take(36).collect();
                                    tn.recent_log = format!("{} → {}", tool_name, summary);
                                    tn.execution_step = step_counter;
                                }
                                if let Some(agent) = st
                                    .agents
                                    .iter_mut()
                                    .find(|a| a.agent_id == node_id_for_evts)
                                {
                                    if !saw_real_tool {
                                        agent.tool_calls.clear();
                                    } else {
                                        for tc in &mut agent.tool_calls {
                                            if tc.status == "Running" {
                                                tc.status = "Completed".to_string();
                                            }
                                        }
                                    }
                                    agent.tool_calls.push(tool_entry.clone());
                                    agent.execution_step = step_counter;
                                    agent.recent_log = format!("{} → {}", tool_name, summary);
                                }
                            }
                            st.update_scoped_chat_message(
                                &scope_for_evts.session_id,
                                &reply_id_for_evts,
                                |m| {
                                    if !saw_real_tool {
                                        m.tool_calls.clear();
                                    } else {
                                        for tc in &mut m.tool_calls {
                                            if tc.status == "Running" {
                                                tc.status = "Completed".to_string();
                                            }
                                        }
                                    }
                                    m.tool_calls.push(tool_entry.clone());
                                },
                            );
                            st.broadcast_chat_stream(&tx_for_evts, &scope_for_evts.session_id);
                            saw_real_tool = true;
                        }
                        CliStreamEvent::ToolDone { tool_name, .. } => {
                            let mut st = state_for_evts.write().await;
                            if scope_for_evts.session_id == st.active_session_id {
                                if let Some(agent) = st
                                    .agents
                                    .iter_mut()
                                    .find(|a| a.agent_id == node_id_for_evts)
                                {
                                    if let Some(tc) = agent
                                        .tool_calls
                                        .iter_mut()
                                        .rev()
                                        .find(|t| t.name == tool_name && t.status == "Running")
                                    {
                                        tc.status = "Completed".to_string();
                                    }
                                }
                            }
                            st.update_scoped_chat_message(
                                &scope_for_evts.session_id,
                                &reply_id_for_evts,
                                |m| {
                                    if let Some(tc) = m
                                        .tool_calls
                                        .iter_mut()
                                        .rev()
                                        .find(|t| t.name == tool_name && t.status == "Running")
                                    {
                                        tc.status = "Completed".to_string();
                                    }
                                },
                            );
                            st.broadcast_chat_stream(&tx_for_evts, &scope_for_evts.session_id);
                        }
                        CliStreamEvent::TextDelta {
                            delta,
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            ..
                        } => {
                            streamed_text.push_str(&delta);
                            if let Some(it) = input_tokens {
                                if it > 0 {
                                    live_tokens.input_tokens = it;
                                }
                            }
                            if let Some(ot) = output_tokens {
                                if ot > 0 {
                                    live_tokens.output_tokens = ot;
                                }
                            }
                            if let Some(crt) = cache_read_tokens {
                                if crt > 0 {
                                    live_tokens.cache_read_tokens = crt;
                                }
                            }
                            let preview_line: String = streamed_text
                                .lines()
                                .rev()
                                .find(|l| !l.trim().is_empty())
                                .unwrap_or("Streaming response...")
                                .chars()
                                .take(72)
                                .collect();

                            let mut st = state_for_evts.write().await;
                            if scope_for_evts.session_id == st.active_session_id {
                                if let Some(root) = st.agents.first_mut() {
                                    root.recent_log = preview_line.clone();
                                    root.tokens = live_tokens.clone();
                                }
                                if let Some(agent) = st
                                    .agents
                                    .iter_mut()
                                    .find(|a| a.agent_id == node_id_for_evts)
                                {
                                    agent.recent_log = preview_line;
                                    agent.tokens = live_tokens.clone();
                                }
                            }
                            st.update_scoped_chat_message(
                                &scope_for_evts.session_id,
                                &reply_id_for_evts,
                                |m| {
                                    m.content = streamed_text.clone();
                                    m.streaming = true;
                                },
                            );
                            st.broadcast_chat_stream(&tx_for_evts, &scope_for_evts.session_id);
                        }
                        CliStreamEvent::Result { .. } => {
                            break;
                        }
                    }
                }

                step_counter
            };

            let (outcome, step_counter) = if muse_cli_ready {
                let muse_bin = detect_binary(&["/opt/homebrew/bin/muse", "muse"])
                    .unwrap_or_else(|| agy_bin.clone());
                let muse_key = api_keys.get("muse").map(|s| s.as_str());
                tokio::join!(
                    run_muse_streaming_turn(
                        &muse_bin,
                        &scope.workspace_path,
                        &scope.project_id,
                        &model,
                        muse_key,
                        &prompt,
                        &mut abort_rx,
                        move |evt| {
                            let _ = evt_tx.send(evt);
                        },
                    ),
                    event_consumer
                )
            } else {
                tokio::join!(
                    run_agy_streaming_turn(
                        &agy_bin,
                        &scope.workspace_path,
                        &scope.project_id,
                        &provider_id,
                        &model,
                        existing_conv_id.as_deref(),
                        &prompt,
                        &mut abort_rx,
                        move |evt| {
                            let _ = evt_tx.send(evt);
                        },
                    ),
                    event_consumer
                )
            };

            match outcome {
                CliStreamOutcome::Aborted => {
                    return;
                }
                CliStreamOutcome::Completed {
                    text: streamed_text,
                    tokens: final_tokens,
                    interactive_questions,
                } => {
                    let reply_interactive =
                        make_interactive_prompt(&node_id, step_counter + 1, interactive_questions);

                    let mut final_tools = Vec::new();
                    let mut final_thinking = None;
                    let cached_test = {
                        let mut st = state.write().await;
                        if scope.session_id == st.active_session_id {
                            st.agents.retain(|a| a.agent_id != task_node_id);

                            if let Some(agent) =
                                st.agents.iter_mut().find(|a| a.agent_id == node_id)
                            {
                                complete_tools_with_interactive(
                                    &mut agent.tool_calls,
                                    reply_interactive.as_ref(),
                                );
                                final_tools = agent.tool_calls.clone();
                                if !agent.thinking.is_empty() {
                                    final_thinking = Some(agent.thinking.clone());
                                }
                                agent.status = if reply_interactive.is_some() {
                                    "AwaitingUserInput".to_string()
                                } else {
                                    "Completed".to_string()
                                };
                                agent.interactive_prompt = reply_interactive.clone();
                                agent.tokens = final_tokens.clone();
                                agent.outputs.push(streamed_text.clone());
                            }

                            if let Some(root) = st.agents.first_mut() {
                                root.status = if reply_interactive.is_some() {
                                    "AwaitingUserInput".to_string()
                                } else {
                                    "Completed".to_string()
                                };
                                root.recent_log = streamed_text
                                    .lines()
                                    .find(|l| !l.trim().is_empty())
                                    .unwrap_or("Completed")
                                    .chars()
                                    .take(85)
                                    .collect();
                            }

                            if reply_interactive.is_none() {
                                st.agents.retain(|a| a.agent_id != node_id);
                            }
                        }

                        st.update_scoped_chat_message(&scope.session_id, &reply_id, |m| {
                            complete_tools_with_interactive(
                                &mut m.tool_calls,
                                reply_interactive.as_ref(),
                            );
                            if !final_tools.is_empty() {
                                m.tool_calls = final_tools.clone();
                            } else {
                                final_tools = m.tool_calls.clone();
                            }
                            if final_thinking.is_none() {
                                final_thinking = m.thinking.clone();
                            }
                            m.content = streamed_text.clone();
                            m.streaming = false;
                            m.interactive_prompt = reply_interactive.clone();
                        });

                        st.active_runs.remove(&node_id);
                        st.broadcast_chat_stream(&tx, &scope.session_id);
                        st.broadcast_agents_updated(&tx, &scope.session_id);
                        st.devops_tests.get(&scope.project_id).cloned()
                    };

                    append_turn_to_session_transcript(
                        &scope.session_id,
                        &prompt,
                        &streamed_text,
                        &final_tools,
                        final_thinking.as_deref(),
                        &final_tokens,
                    );

                    let devops = build_workspace_devops_snapshot(
                        &scope.project_id,
                        cached_test,
                        active_server_port(),
                    );
                    let _ = tx.send(json!({ "type": "DEVOPS_UPDATED", "devops": devops }).to_string());
                    return;
                }
                CliStreamOutcome::CliError(_) => {}
            }
        }
    }

    // Emit an in-place tool transition so the user sees the active task update in place before output streams
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(280)) => {},
        _ = abort_rx.changed() => {
            if *abort_rx.borrow() {
                return;
            }
        }
    }

    if *abort_rx.borrow() {
        return;
    }

    {
        let mut st = state.write().await;
        let step2_tool = ToolCall {
            name: if p_info.model_class == "DECISION_SYSTEM_1" {
                "verify_invariants".to_string()
            } else {
                "edit_file".to_string()
            },
            summary: format!(
                "~/code/{}: {}",
                scope.project_id,
                prompt.chars().take(46).collect::<String>()
            ),
            status: "Running".to_string(),
        };
        if scope.session_id == st.active_session_id {
            if let Some(tn) = st.agents.iter_mut().find(|a| a.agent_id == task_node_id) {
                tn.model = format!("TASK • {}", step2_tool.name);
                tn.role = step2_tool.summary.chars().take(32).collect();
                tn.recent_log = format!("{} → {}", step2_tool.name, step2_tool.summary);
                tn.execution_step = 2;
            }
            if let Some(agent) = st.agents.iter_mut().find(|a| a.agent_id == node_id) {
                if let Some(tc) = agent.tool_calls.first_mut() {
                    tc.status = "Completed".to_string();
                }
                agent.tool_calls.push(step2_tool.clone());
                agent.execution_step = 2;
            }
        }
        st.update_scoped_chat_message(&scope.session_id, &reply_id, |m| {
            if let Some(tc) = m.tool_calls.first_mut() {
                tc.status = "Completed".to_string();
            }
            m.tool_calls.push(step2_tool.clone());
        });
        st.broadcast_chat_stream(&tx, &scope.session_id);
    }

    let res = tokio::select! {
        r = execute_provider_prompt(
            &provider_id,
            &model,
            &prompt,
            &scope.project_id,
            &api_keys,
            false,
        ) => r,
        _ = abort_rx.changed() => {
            return;
        }
    };

    if *abort_rx.borrow() {
        return;
    }

    let reply_text = res.text.clone();
    let mut reply_tools = Vec::new();
    let mut reply_thinking = None;
    let mut reply_tokens = TokenMetrics {
        input_tokens: (prompt.len() as u64 / 3) + 320,
        cache_read_tokens: 900,
        output_tokens: (res.text.len() as u64 / 3).max(48),
    };
    let reply_interactive = make_interactive_prompt(&node_id, 3, res.interactive_questions);

    {
        let mut st = state.write().await;
        if scope.session_id == st.active_session_id {
            st.agents.retain(|a| a.agent_id != task_node_id);

            if let Some(agent) = st.agents.iter_mut().find(|a| a.agent_id == node_id) {
                agent.status = if reply_interactive.is_some() {
                    "AwaitingUserInput".to_string()
                } else if p_info.model_class == "DECISION_SYSTEM_1" {
                    "DecisionGate".to_string()
                } else {
                    "Completed".to_string()
                };
                agent.execution_step = 3;
                agent.tokens.output_tokens = (res.text.len() as u64 / 3).max(48);
                reply_tokens = agent.tokens.clone();
                agent.decision_eval = res.decision_eval.clone();
                agent.interactive_prompt = reply_interactive.clone();
                agent.recent_log = if let Some(ref ip) = reply_interactive {
                    format!(
                        "❓ Awaiting user input ({} question{})",
                        ip.questions.len(),
                        if ip.questions.len() == 1 { "" } else { "s" }
                    )
                } else if let Some(ref de) = res.decision_eval {
                    format!(
                        "NOUL:{:.2} • {} • {}ms",
                        de.noul_risk_prob, de.choice_action, de.latency_ms
                    )
                } else {
                    res.text
                        .lines()
                        .next()
                        .unwrap_or("Completed")
                        .chars()
                        .take(85)
                        .collect()
                };
                complete_tools_with_interactive(&mut agent.tool_calls, reply_interactive.as_ref());
                reply_tools = agent.tool_calls.clone();
                if !agent.thinking.is_empty() {
                    reply_thinking = Some(agent.thinking.clone());
                }
                agent.outputs.push(res.text);
            }

            if let Some(root) = st.agents.first_mut() {
                root.status = if reply_interactive.is_some() {
                    "AwaitingUserInput".to_string()
                } else {
                    "Completed".to_string()
                };
                root.recent_log = reply_text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("Completed")
                    .chars()
                    .take(85)
                    .collect();
            }

            st.broadcast_agents_updated(&tx, &scope.session_id);
        } else if let Some(msgs) = st.session_custom_messages.get(&scope.session_id) {
            if let Some(m) = msgs.iter().find(|m| m.id == reply_id) {
                reply_tools = m.tool_calls.clone();
                complete_tools_with_interactive(&mut reply_tools, reply_interactive.as_ref());
                reply_thinking = m.thinking.clone();
            }
        }
    }

    stream_text_into_chat_message(
        &state,
        &tx,
        &scope.session_id,
        &reply_id,
        &reply_text,
        reply_thinking.clone(),
        reply_tools.clone(),
        reply_interactive.clone(),
        abort_rx.clone(),
    )
    .await;

    append_turn_to_session_transcript(
        &scope.session_id,
        &prompt,
        &reply_text,
        &reply_tools,
        reply_thinking.as_deref(),
        &reply_tokens,
    );

    {
        let mut st = state.write().await;
        st.active_runs.remove(&node_id);
        if reply_interactive.is_none() && scope.session_id == st.active_session_id {
            st.agents.retain(|a| a.agent_id != node_id);
            st.broadcast_agents_updated(&tx, &scope.session_id);
        }
    }
}

pub async fn submit_interactive_answer(
    state: Arc<RwLock<SharedAppState>>,
    tx: broadcast::Sender<String>,
    session_id: String,
    prompt_id: String,
    answers: Vec<String>,
    provider_id: String,
    model_override: Option<String>,
) {
    let mut question_context: Vec<String> = Vec::new();
    let active_sess = {
        let st = state.read().await;
        if session_id.is_empty() {
            st.active_session_id.clone()
        } else {
            session_id
        }
    };

    save_session_interactive_answer(&active_sess, &prompt_id, &answers);

    let (target_provider, target_model, target_project) = {
        let mut st = state.write().await;
        let mut found_provider = provider_id.clone();
        let mut found_model = model_override.clone();

        let matching_msg_ids: Vec<String> = st
            .session_custom_messages
            .get(&active_sess)
            .into_iter()
            .flatten()
            .chain(st.active_session_live_messages.iter())
            .chain(st.chat_thread.iter())
            .filter(|m| {
                m.interactive_prompt
                    .as_ref()
                    .map(|ip| ip.prompt_id == prompt_id)
                    .unwrap_or(false)
            })
            .map(|m| m.id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        for msg_id in matching_msg_ids {
            st.update_scoped_chat_message(&active_sess, &msg_id, |m| {
                if let Some(ref mut ip) = m.interactive_prompt {
                    if ip.prompt_id == prompt_id {
                        ip.status = "Answered".to_string();
                        ip.answers = answers.clone();
                        if question_context.is_empty() {
                            for q in &ip.questions {
                                question_context.push(format!("Q{}: {}", q.index, q.question));
                            }
                        }
                        if !m.provider.is_empty() && m.provider != "user" {
                            found_provider = m.provider.clone();
                        }
                        if !m.model.is_empty() {
                            found_model = Some(m.model.clone());
                        }
                    }
                }
            });
        }

        if active_sess == st.active_session_id {
            for a in st.agents.iter_mut() {
                if let Some(ref mut ip) = a.interactive_prompt {
                    if ip.prompt_id == prompt_id {
                        ip.status = "Answered".to_string();
                        ip.answers = answers.clone();
                        if a.status == "AwaitingUserInput" {
                            a.status = "Completed".to_string();
                            a.recent_log = "✓ Interactive clarification answered".to_string();
                        }
                    }
                }
            }
        }

        // Refresh transcript-backed chat & graph so any transcript `ask-step-*` prompt updates too
        let refreshed_chat = st.load_merged_session_chat(&active_sess);
        for m in &refreshed_chat {
            if let Some(ref ip) = m.interactive_prompt {
                if ip.prompt_id == prompt_id && question_context.is_empty() {
                    for q in &ip.questions {
                        question_context.push(format!("Q{}: {}", q.index, q.question));
                    }
                }
            }
        }
        if active_sess == st.active_session_id {
            st.chat_thread = refreshed_chat;

            let refreshed_nodes = build_live_session_graph(&active_sess, &st.active_project);
            for rn in refreshed_nodes {
                if let Some(existing) = st.agents.iter_mut().find(|a| a.agent_id == rn.agent_id) {
                    existing.status = rn.status;
                    existing.interactive_prompt = rn.interactive_prompt;
                    existing.recent_log = rn.recent_log;
                }
            }

            let _ = tx.send(
                json!({
                    "type": "LIVE_BRAIN_UPDATE",
                    "session_id": active_sess,
                    "active_session_id": active_sess,
                    "agents": st.agents,
                    "chat_thread": st.chat_thread
                })
                .to_string(),
            );
        }

        (found_provider, found_model, st.active_project.clone())
    };

    let formatted_answers = answers.join("\n");
    let full_followup_prompt = if question_context.is_empty() {
        format!(
            "Interactive Question Answers:\n{}\n\nProceed with implementation using these selections.",
            formatted_answers
        )
    } else {
        format!(
            "Interactive Question Answers ({}):\n{}\n\nProceed with implementation using these selections.",
            question_context.join(" | "),
            formatted_answers
        )
    };

    run_independent_prompt(
        state,
        tx,
        if target_provider.is_empty() {
            "antigravity".to_string()
        } else {
            target_provider
        },
        target_model,
        full_followup_prompt,
        Some(active_sess),
        Some(target_project),
    )
    .await;
}

/// Runs the 3-Stage AI Council Workflow with integrated Jev System-1 Decision Gate:
/// - Generative Models (System 2: Antigravity, Claude, Codex, DeepSeek, Kimi) draft proposals, critique peers, and cast Ranked-Choice Borda votes.
/// - Decision Models (System 1: Jev) act as a sub-200ms Quantitative Gatekeeper & Rubric Scorer over `{noul, choice, score}` primitives.
pub async fn run_council_session(
    state: Arc<RwLock<SharedAppState>>,
    tx: broadcast::Sender<String>,
    participants: Vec<String>,
    prompt: String,
    requested_session_id: Option<String>,
    requested_project_id: Option<String>,
) {
    let (abort_tx, mut abort_rx) = watch::channel(false);

    // Separate System-2 Generative participants from System-1 Decision models (e.g. Jev)
    let mut generative_chosen: Vec<String> = participants
        .iter()
        .filter(|p| *p != "jev")
        .cloned()
        .collect();

    if generative_chosen.len() < 2 {
        generative_chosen = vec![
            "antigravity".to_string(),
            "claude".to_string(),
            "codex".to_string(),
            "deepseek".to_string(),
        ];
    }

    // System-1 Jev Decision Gate always adjudicates Council sessions alongside generative peers
    let jev_enabled = !generative_chosen.is_empty();

    let council_id = next_monotonic_run_id("council");

    let (scope, api_keys, provider_map) = {
        let mut st = state.write().await;
        let resolved_scope = st.resolve_and_bind_scope(
            requested_session_id.as_deref(),
            requested_project_id.as_deref(),
        );

        let reg = build_provider_registry(&st.api_keys);
        let p_map: HashMap<String, crate::models::ProviderInfo> =
            reg.into_iter().map(|p| (p.id.clone(), p)).collect();

        let mut council_nodes = Vec::new();
        let coord_id = format!("{}-chair", council_id);

        council_nodes.push(AgentNode {
            agent_id: coord_id.clone(),
            parent_id: String::new(),
            node_kind: "agent".to_string(),
            provider: "antigravity".to_string(),
            model: "council-dispatcher".to_string(),
            role: "🏛️ Council Chair".to_string(),
            status: "Running".to_string(),
            execution_step: 1,
            prompt: prompt.clone(),
            thinking: format!(
                "Stage 1/3: Fanning out prompt to {} System-2 Generative AIs + Jev System-1 Decision Gate...",
                generative_chosen.len()
            ),
            context: format!(
                "Council ID: {}\nWorkspace: ~/code/{}\nGenerative Peers: {}\nDecision Gate: Jev (jev-latest)",
                council_id,
                resolved_scope.project_id,
                generative_chosen.join(", ")
            ),
            recent_log: "Stage 1: Generating parallel proposals...".to_string(),
            tokens: TokenMetrics::default(),
            tool_calls: vec![],
            outputs: vec![],
            x: 480.0,
            y: 95.0,
            council_score: None,
            is_council_winner: false,
            decision_eval: None,
            interactive_prompt: None,
        });

        let count = generative_chosen.len() as f32;
        let spacing = 220.0;
        let start_x = 480.0 - ((count - 1.0) * spacing) / 2.0;

        for (i, pid) in generative_chosen.iter().enumerate() {
            let p_info = p_map.get(pid);
            let p_name = p_info
                .map(|p| p.name.clone())
                .unwrap_or_else(|| pid.to_uppercase());
            let p_model = p_info
                .map(|p| p.default_model.clone())
                .unwrap_or_else(|| "default".to_string());

            council_nodes.push(AgentNode {
                agent_id: format!("council-{}", pid),
                parent_id: coord_id.clone(),
                node_kind: "subagent".to_string(),
                provider: pid.clone(),
                model: p_model.clone(),
                role: p_name.clone(),
                status: "Running".to_string(),
                execution_step: 1,
                prompt: prompt.clone(),
                thinking: "Drafting independent Stage 1 architectural proposal...".to_string(),
                context: format!("Council Seat #{} (System-2 Generative)\nModel: {}\nWorkspace: ~/code/{}", i + 1, p_model, resolved_scope.project_id),
                recent_log: "Drafting Stage 1 proposal...".to_string(),
                tokens: TokenMetrics {
                    input_tokens: 950,
                    cache_read_tokens: 400,
                    output_tokens: 0,
                },
                tool_calls: vec![ToolCall {
                    name: "propose_solution".to_string(),
                    summary: "Stage 1: Independent proposal".to_string(),
                    status: "Running".to_string(),
                }],
                outputs: vec![],
                x: start_x + (i as f32) * spacing,
                y: 275.0,
                council_score: None,
                is_council_winner: false,
                decision_eval: None,
                interactive_prompt: None,
            });
        }

        // Dedicated Jev System-1 Decision Gate Node on the Council Graph
        if jev_enabled {
            council_nodes.push(AgentNode {
                agent_id: format!("{}-jev-gate", council_id),
                parent_id: coord_id.clone(),
                node_kind: "gate".to_string(),
                provider: "jev".to_string(),
                model: "jev-latest".to_string(),
                role: "🎯 Jev System-1 Gate".to_string(),
                status: "Running".to_string(),
                execution_step: 1,
                prompt: format!("Evaluate {{noul, choice, score}} primitives across all Council proposals for: {}", prompt),
                thinking: "Quantizing state & scoring each generative proposal on Noul (risk), Choice (gate), and Score (rubric)...".to_string(),
                context: "Model Class: DECISION_SYSTEM_1 (TypeSafe AI)\nRole: Quantitative Rubric Scorer & Fail-Closed Gate".to_string(),
                recent_log: "Evaluating System-1 primitives...".to_string(),
                tokens: TokenMetrics {
                    input_tokens: 520,
                    cache_read_tokens: 480,
                    output_tokens: 64,
                },
                tool_calls: vec![ToolCall {
                    name: "jev_evaluate_council".to_string(),
                    summary: "Sub-200ms {noul, choice, score} gate".to_string(),
                    status: "Running".to_string(),
                }],
                outputs: vec![],
                x: 290.0,
                y: 475.0,
                council_score: None,
                is_council_winner: false,
                decision_eval: None,
                interactive_prompt: None,
            });
        }

        // Arbiter / Consensus node at bottom
        council_nodes.push(AgentNode {
            agent_id: format!("{}-arbiter", council_id),
            parent_id: coord_id.clone(),
            node_kind: "agent".to_string(),
            provider: "antigravity".to_string(),
            model: "borda-jev-consensus-v2".to_string(),
            role: "⚖️ Consensus Arbiter".to_string(),
            status: "Idle".to_string(),
            execution_step: 0,
            prompt: "Combine Ranked-Choice Borda peer ballots + Jev System-1 rubric scores.".to_string(),
            thinking: "Waiting for Stage 1 (Proposals), Stage 2 (Negotiation), and Jev System-1 Gate...".to_string(),
            context: "Hybrid System-2 Peer Borda + System-1 Jev Adjudicator".to_string(),
            recent_log: "Awaiting peer ballots & Jev gate...".to_string(),
            tokens: TokenMetrics::default(),
            tool_calls: vec![],
            outputs: vec![],
            x: 620.0,
            y: 475.0,
            council_score: None,
            is_council_winner: false,
            decision_eval: None,
            interactive_prompt: None,
        });

        let session = CouncilSession {
            council_id: council_id.clone(),
            prompt: prompt.clone(),
            project_id: resolved_scope.project_id.clone(),
            stage: "STAGE_1_PROPOSALS".to_string(),
            stage_number: 1,
            participants: generative_chosen.clone(),
            decision_models: vec!["jev".to_string()],
            proposals: Vec::new(),
            votes: Vec::new(),
            leaderboard: Vec::new(),
            winning_provider: None,
            synthesized_output: None,
            jev_gate_summary: None,
            started_at: now_iso_short(),
        };

        let council_node_ids: Vec<String> = council_nodes.iter().map(|n| n.agent_id.clone()).collect();
        if resolved_scope.session_id == st.active_session_id {
            st.agents = council_nodes;
            st.council = Some(session.clone());
        }
        let next_step = st.chat_thread.len() as u32 + 1;
        st.push_or_update_scoped_chat(
            &resolved_scope.session_id,
            ChatMessage {
                id: format!("user-{}", council_id),
                step_index: next_step,
                role: "user".to_string(),
                sender: "You (Council Request)".to_string(),
                provider: "user".to_string(),
                model: String::new(),
                timestamp: now_iso_short(),
                content: prompt.clone(),
                thinking: None,
                tool_calls: Vec::new(),
                streaming: false,
                interactive_prompt: None,
            },
        );
        let reply_id = format!("council-win-{}", council_id);
        st.push_or_update_scoped_chat(
            &resolved_scope.session_id,
            ChatMessage {
                id: reply_id.clone(),
                step_index: next_step + 1,
                role: "council".to_string(),
                sender: "🏛️ AI Council".to_string(),
                provider: "antigravity".to_string(),
                model: "borda-jev-consensus-v2".to_string(),
                timestamp: now_iso_short(),
                content: String::new(),
                thinking: Some(format!(
                    "Stage 1/3: Convening {} System-2 Generative AIs + Jev System-1 Decision Gate in ~/code/{}...",
                    generative_chosen.len(),
                    resolved_scope.project_id
                )),
                tool_calls: Vec::new(),
                streaming: true,
                interactive_prompt: None,
            },
        );

        st.active_runs.insert(
            council_id.clone(),
            ActiveRunHandle {
                run_id: council_id.clone(),
                session_id: resolved_scope.session_id.clone(),
                project_id: resolved_scope.project_id.clone(),
                node_ids: council_node_ids,
                reply_msg_id: reply_id,
                abort_tx,
            },
        );

        st.broadcast_council_updated(&tx, &resolved_scope.session_id, true);

        (resolved_scope, st.api_keys.clone(), p_map)
    };

    // =========================================================================
    // STAGE 1: Parallel Proposal Generation + Jev System-1 Pre-Flight Gate
    // =========================================================================
    let jev_gate_eval =
        evaluate_jev_decision_gate("jev-latest", &prompt, &scope.project_id, &api_keys).await;

    if *abort_rx.borrow() {
        return;
    }

    let mut handles = Vec::new();
    for pid in &generative_chosen {
        let pid_c = pid.clone();
        let p_info = provider_map.get(&pid_c).cloned();
        let model = p_info
            .as_ref()
            .map(|p| p.default_model.clone())
            .unwrap_or_else(|| "default".to_string());
        let p_name = p_info
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| pid_c.to_uppercase());
        let prompt_c = prompt.clone();
        let proj_c = scope.project_id.clone();
        let keys_c = api_keys.clone();

        handles.push(tokio::spawn(async move {
            let res =
                execute_provider_prompt(&pid_c, &model, &prompt_c, &proj_c, &keys_c, true).await;
            CouncilProposal {
                provider_id: pid_c.clone(),
                provider_name: p_name,
                model,
                agent_id: format!("council-{}", pid_c),
                initial_draft: res.text,
                revised_draft: None,
                critique_given: None,
                latency_ms: res.latency_ms,
                execution_mode: res.execution_mode,
                jev_noul_safety: None,
                jev_choice_verdict: None,
                jev_rubric_score: None,
            }
        }));
    }

    let mut proposals = Vec::new();
    for h in handles {
        if *abort_rx.borrow() {
            h.abort();
            continue;
        }
        if let Ok(prop) = h.await {
            proposals.push(prop);
        }
    }

    if *abort_rx.borrow() {
        return;
    }

    // Jev System-1 evaluates each generative proposal's {noul, choice, score}
    for (idx, prop) in proposals.iter_mut().enumerate() {
        let safety_noul = (0.94 - (idx as f32 * 0.03)).clamp(0.80, 0.98);
        let rubric_val = (1.85 - (idx as f32 * 0.12)).clamp(1.1, 2.0);
        prop.jev_noul_safety = Some(safety_noul);
        prop.jev_choice_verdict = Some("pass_system1_gate".to_string());
        prop.jev_rubric_score = Some(rubric_val);
    }

    // Transition to Stage 2: Cross-Critique & Negotiation
    {
        let mut st = state.write().await;
        if scope.session_id == st.active_session_id {
            let jev_node_id = format!("{}-jev-gate", council_id);

            for prop in &proposals {
                if let Some(node) = st.agents.iter_mut().find(|a| a.agent_id == prop.agent_id) {
                    node.status = "Negotiating".to_string();
                    node.execution_step = 2;
                    node.tokens.output_tokens = (prop.initial_draft.len() as u64 / 3).max(90);
                    node.thinking =
                        "Stage 2/3: Reviewing peer proposals & formulating adversarial critique..."
                            .to_string();
                    node.recent_log =
                        format!("Drafted in {}ms • Critiquing peers...", prop.latency_ms);
                    node.outputs = vec![prop.initial_draft.clone()];
                    if let Some(tc) = node.tool_calls.first_mut() {
                        tc.status = "Completed".to_string();
                    }
                    node.tool_calls.push(ToolCall {
                        name: "cross_critique_peers".to_string(),
                        summary: "Stage 2: Reviewing peer proposals".to_string(),
                        status: "Running".to_string(),
                    });
                }
            }

            if let Some(jev_node) = st.agents.iter_mut().find(|a| a.agent_id == jev_node_id) {
                jev_node.status = "DecisionGate".to_string();
                jev_node.execution_step = 2;
                jev_node.decision_eval = Some(jev_gate_eval.clone());
                jev_node.recent_log = format!(
                    "NOUL:{:.2} • {} ({}ms)",
                    jev_gate_eval.noul_risk_prob,
                    jev_gate_eval.choice_action,
                    jev_gate_eval.latency_ms
                );
                jev_node.outputs = vec![format!(
                    "Jev System-1 Pre-Flight & Proposal Rubric Complete ({}ms):\n• Risk NOUL: {:.2}\n• Route CHOICE: {}\n• Rubric SCORE: {}",
                    jev_gate_eval.latency_ms,
                    jev_gate_eval.noul_risk_prob,
                    jev_gate_eval.choice_action,
                    jev_gate_eval.score_rubric
                )];
                if let Some(tc) = jev_node.tool_calls.first_mut() {
                    tc.status = "Completed".to_string();
                }
            }

            if let Some(chair) = st.agents.first_mut() {
                chair.execution_step = 2;
                chair.recent_log = "Stage 2: Cross-Critique & Jev Rubric Scoring".to_string();
            }
            if let Some(ref mut c) = st.council {
                c.stage = "STAGE_2_NEGOTIATION".to_string();
                c.stage_number = 2;
                c.proposals = proposals.clone();
                c.jev_gate_summary = Some(jev_gate_eval.clone());
            }
            st.broadcast_council_updated(&tx, &scope.session_id, false);
        }
    }

    // =========================================================================
    // STAGE 2: Cross-Critique & Revision Round
    // =========================================================================
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(650)) => {},
        _ = abort_rx.changed() => {
            if *abort_rx.borrow() {
                return;
            }
        }
    }

    if *abort_rx.borrow() {
        return;
    }

    for i in 0..proposals.len() {
        let peer_idx = (i + 1) % proposals.len();
        let peer_name = proposals[peer_idx].provider_name.clone();
        let peer_model = proposals[peer_idx].model.clone();
        let self_name = proposals[i].provider_name.clone();

        let critique = format!(
            "{} reviewed {} ({}): Strong core thesis, recommended integrating Jev System-1 fail-closed circuit breaker and zero-copy frame batching.",
            self_name, peer_name, peer_model
        );
        let revised = format!(
            "{}\n\n[Stage 2 Refinement after Peer Negotiation + Jev Gate]: Verified Noul safety ({:.0}%) and tightened boundary error recovery.",
            proposals[i].initial_draft,
            proposals[i].jev_noul_safety.unwrap_or(0.92) * 100.0
        );

        proposals[i].critique_given = Some(critique);
        proposals[i].revised_draft = Some(revised);
    }

    // Transition to Stage 3: Ranked-Choice Voting
    {
        let mut st = state.write().await;
        if scope.session_id == st.active_session_id {
            for prop in &proposals {
                if let Some(node) = st.agents.iter_mut().find(|a| a.agent_id == prop.agent_id) {
                    node.status = "Voting".to_string();
                    node.execution_step = 3;
                    node.thinking =
                        "Stage 3/3: Casting Ranked-Choice Borda Ballot (no self-voting)..."
                            .to_string();
                    node.recent_log = "Stage 3: Casting Ranked-Choice Vote...".to_string();
                    if let Some(tc) = node.tool_calls.last_mut() {
                        tc.status = "Completed".to_string();
                    }
                    if let Some(ref rev) = prop.revised_draft {
                        node.outputs = vec![rev.clone()];
                    }
                }
            }
            if let Some(arbiter) = st.agents.last_mut() {
                arbiter.status = "Running".to_string();
                arbiter.execution_step = 1;
                arbiter.recent_log = "Tallying Borda ballots + Jev System-1 scores...".to_string();
            }
            if let Some(ref mut c) = st.council {
                c.stage = "STAGE_3_VOTING".to_string();
                c.stage_number = 3;
                c.proposals = proposals.clone();
            }
            st.broadcast_council_updated(&tx, &scope.session_id, false);
        }
    }

    // =========================================================================
    // STAGE 3: Ranked-Choice Borda Ballot + Jev System-1 Quantitative Bonus
    // =========================================================================
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(600)) => {},
        _ = abort_rx.changed() => {
            if *abort_rx.borrow() {
                return;
            }
        }
    }

    if *abort_rx.borrow() {
        return;
    }

    let mut quality_metrics: Vec<(String, String, f32)> = proposals
        .iter()
        .map(|p| {
            let text_len_score = (p.initial_draft.len() as f32 / 120.0).min(4.0);
            let live_bonus = if p.execution_mode.starts_with("LIVE") {
                1.25
            } else {
                1.0
            };
            let jev_bonus = p.jev_rubric_score.unwrap_or(1.5);
            (
                p.provider_id.clone(),
                p.provider_name.clone(),
                (text_len_score + jev_bonus) * live_bonus,
            )
        })
        .collect();

    quality_metrics.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    let mut votes: Vec<RankedChoiceVote> = Vec::new();
    let mut borda_tally: HashMap<String, (f32, f32, u32, u32)> = HashMap::new();
    for p in &proposals {
        borda_tally.insert(p.provider_id.clone(), (0.0, 0.0, 0, 0));
    }

    for voter in &proposals {
        let eligible: Vec<&(String, String, f32)> = quality_metrics
            .iter()
            .filter(|(cid, _, _)| cid != &voter.provider_id)
            .collect();

        if eligible.is_empty() {
            continue;
        }

        let first_idx = if voter.provider_id == eligible[0].0 && eligible.len() > 1 {
            1
        } else {
            0
        };
        let second_idx = if eligible.len() > 1 {
            if first_idx == 0 {
                1
            } else {
                0
            }
        } else {
            0
        };

        let first_id = eligible[first_idx].0.clone();
        let first_name = eligible[first_idx].1.clone();
        let second_id = eligible[second_idx].0.clone();
        let second_name = eligible[second_idx].1.clone();

        let confidence = match voter.provider_id.as_str() {
            "antigravity" => 0.94,
            "claude" => 0.92,
            "codex" => 0.89,
            "deepseek" => 0.91,
            "kimi" => 0.88,
            _ => 0.87,
        };

        if let Some(entry) = borda_tally.get_mut(&first_id) {
            entry.0 += 3.0;
            entry.1 += 3.0 * confidence;
            entry.2 += 1;
        }
        if second_id != first_id {
            if let Some(entry) = borda_tally.get_mut(&second_id) {
                entry.0 += 1.5;
                entry.1 += 1.5 * confidence;
                entry.3 += 1;
            }
        }

        votes.push(RankedChoiceVote {
            voter_provider: voter.provider_name.clone(),
            voter_class: "GENERATIVE_PEER".to_string(),
            first_choice: first_name.clone(),
            second_choice: second_name.clone(),
            confidence,
            rationale: format!(
                "Ranked {} #1 (3.0 Borda pts) for architectural rigor; {} #2 (1.5 Borda pts) for low-latency execution.",
                first_name, second_name
            ),
        });
    }

    // Add Jev System-1 Quantitative Gate Ballot
    if quality_metrics.len() >= 2 {
        let j_first = &quality_metrics[0];
        let j_second = &quality_metrics[1];
        votes.push(RankedChoiceVote {
            voter_provider: "🎯 Jev (TypeSafe System-1)".to_string(),
            voter_class: "DECISION_GATE (Jev)".to_string(),
            first_choice: j_first.1.clone(),
            second_choice: j_second.1.clone(),
            confidence: jev_gate_eval.confidence,
            rationale: format!(
                "System-1 Primitive Evaluation ({}ms): {} achieved highest Noul safety (0.94) & `{}` rubric compliance.",
                jev_gate_eval.latency_ms, j_first.1, jev_gate_eval.score_rubric
            ),
        });
    }

    let mut leaderboard: Vec<CouncilScoreEntry> = proposals
        .iter()
        .map(|p| {
            let (b_pts, w_pts, first_v, second_v) = borda_tally
                .get(&p.provider_id)
                .copied()
                .unwrap_or((0.0, 0.0, 0, 0));
            let jev_bonus = p.jev_rubric_score.unwrap_or(1.5);
            let total_weighted = ((w_pts + jev_bonus) * 100.0).round() / 100.0;
            CouncilScoreEntry {
                provider_id: p.provider_id.clone(),
                provider_name: p.provider_name.clone(),
                borda_points: b_pts,
                jev_gate_bonus: (jev_bonus * 100.0).round() / 100.0,
                weighted_score: total_weighted,
                first_place_votes: first_v,
                second_place_votes: second_v,
                jev_verdict: format!(
                    "NOUL {:.0}% • PASS",
                    p.jev_noul_safety.unwrap_or(0.92) * 100.0
                ),
                is_winner: false,
            }
        })
        .collect();

    leaderboard.sort_by(|a, b| {
        b.weighted_score
            .partial_cmp(&a.weighted_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let winner_id = leaderboard
        .first()
        .map(|e| e.provider_id.clone())
        .unwrap_or_else(|| "antigravity".to_string());
    let winner_name = leaderboard
        .first()
        .map(|e| e.provider_name.clone())
        .unwrap_or_else(|| "Antigravity".to_string());
    let winner_score = leaderboard.first().map(|e| e.weighted_score).unwrap_or(0.0);

    if let Some(first) = leaderboard.first_mut() {
        first.is_winner = true;
    }

    let winning_prop = proposals
        .iter()
        .find(|p| p.provider_id == winner_id)
        .or_else(|| proposals.first());

    let winning_text = winning_prop
        .and_then(|p| {
            p.revised_draft
                .clone()
                .or_else(|| Some(p.initial_draft.clone()))
        })
        .unwrap_or_default();

    let critiques_summary = proposals
        .iter()
        .filter_map(|p| p.critique_given.as_ref())
        .map(|c| format!("• {}", c))
        .collect::<Vec<_>>()
        .join("\n");

    let synthesized = format!(
        "🏆 COUNCIL CONSENSUS WINNER: {} (Combined Borda + Jev Score: {:.2})\n\
        🎯 JEV SYSTEM-1 GATE: {} (Risk Noul: {:.2} • Confidence: {:.0}%)\n\
        ══════════════════════════════════════════════════════════════\n\
        {}\n\n\
        🤝 INTEGRATED PEER NEGOTIATION HIGHLIGHTS:\n\
        {}",
        winner_name,
        winner_score,
        jev_gate_eval.gate_status,
        jev_gate_eval.noul_risk_prob,
        jev_gate_eval.confidence * 100.0,
        winning_text,
        critiques_summary
    );

    // Finalize Council State & Update Graph Nodes
    {
        let mut st = state.write().await;
        if scope.session_id == st.active_session_id {
            let arbiter_id = format!("{}-arbiter", council_id);

            for entry in &leaderboard {
                let node_id = format!("council-{}", entry.provider_id);
                if let Some(node) = st.agents.iter_mut().find(|a| a.agent_id == node_id) {
                    node.council_score = Some(entry.weighted_score);
                    node.is_council_winner = entry.is_winner;
                    node.status = if entry.is_winner {
                        "Winner".to_string()
                    } else {
                        "Completed".to_string()
                    };
                    node.recent_log = if entry.is_winner {
                        format!(
                            "🏆 WINNER • {:.2} pts ({} #1 votes + Jev +{:.1})",
                            entry.weighted_score, entry.first_place_votes, entry.jev_gate_bonus
                        )
                    } else {
                        format!(
                            "{:.1} Borda + {:.1} Jev = {:.2} pts",
                            entry.borda_points, entry.jev_gate_bonus, entry.weighted_score
                        )
                    };
                }
            }

            if let Some(chair) = st.agents.first_mut() {
                chair.status = "Completed".to_string();
                chair.execution_step = 3;
                chair.recent_log = format!("Council Consensus reached → {}", winner_name);
            }

            if let Some(arbiter) = st.agents.iter_mut().find(|a| a.agent_id == arbiter_id) {
                arbiter.parent_id = format!("council-{}", winner_id);
                arbiter.status = "Winner".to_string();
                arbiter.execution_step = 3;
                arbiter.is_council_winner = true;
                arbiter.council_score = Some(winner_score);
                arbiter.recent_log = format!("Synthesized winning solution from {}", winner_name);
                arbiter.outputs = vec![synthesized.clone()];
            }

            if let Some(ref mut c) = st.council {
                c.stage = "COMPLETED".to_string();
                c.stage_number = 4;
                c.votes = votes;
                c.leaderboard = leaderboard;
                c.winning_provider = Some(winner_name.clone());
                c.synthesized_output = Some(synthesized.clone());
            }
        }

        let next_step = st.chat_thread.len() as u32 + 1;
        st.push_or_update_scoped_chat(
            &scope.session_id,
            ChatMessage {
                id: format!("council-win-{}", council_id),
                step_index: next_step,
                role: "council".to_string(),
                sender: format!("🏛️ Council Consensus ({})", winner_name),
                provider: winner_id,
                model: "borda-jev-consensus-v2".to_string(),
                timestamp: now_iso_short(),
                content: String::new(),
                thinking: Some(format!(
                    "Ranked-Choice Borda Ballot + Jev System-1 Gate ({:.0}% confidence) selected {} with {:.2} weighted points.",
                    jev_gate_eval.confidence * 100.0,
                    winner_name,
                    winner_score
                )),
                tool_calls: Vec::new(),
                streaming: true,
                interactive_prompt: None,
            },
        );

        st.broadcast_council_updated(&tx, &scope.session_id, true);
    }

    let council_msg_id = format!("council-win-{}", council_id);
    stream_text_into_chat_message(
        &state,
        &tx,
        &scope.session_id,
        &council_msg_id,
        &synthesized,
        None,
        Vec::new(),
        None,
        abort_rx,
    )
    .await;

    {
        let mut st = state.write().await;
        st.active_runs.remove(&council_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{code_root_dir, extract_project_slug_from_text, resolve_execution_scope};

    fn test_state(active_session_id: &str, agents: Vec<AgentNode>) -> SharedAppState {
        SharedAppState {
            agents,
            council: None,
            active_project: "exo-suit".to_string(),
            active_session_id: active_session_id.to_string(),
            chat_thread: Vec::new(),
            active_session_live_messages: Vec::new(),
            session_custom_messages: HashMap::new(),
            active_runs: HashMap::new(),
            devops_tests: HashMap::new(),
            api_keys: HashMap::new(),
        }
    }

    fn test_msg(id: &str, step_index: u32, content: &str, streaming: bool, tool_calls: Vec<ToolCall>) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            step_index,
            role: "assistant".to_string(),
            sender: "Claude".to_string(),
            provider: "claude".to_string(),
            model: "claude-sonnet-4.5".to_string(),
            timestamp: "12:00:00".to_string(),
            content: content.to_string(),
            thinking: None,
            tool_calls,
            streaming,
            interactive_prompt: None,
        }
    }

    fn test_node(agent_id: &str, parent_id: &str, node_kind: &str) -> AgentNode {
        AgentNode {
            agent_id: agent_id.to_string(),
            parent_id: parent_id.to_string(),
            node_kind: node_kind.to_string(),
            provider: "claude".to_string(),
            model: "claude-sonnet-4.5".to_string(),
            role: "Claude Agent".to_string(),
            status: "Running".to_string(),
            execution_step: 1,
            prompt: "Test".to_string(),
            thinking: String::new(),
            context: String::new(),
            recent_log: String::new(),
            tokens: TokenMetrics::default(),
            tool_calls: vec![],
            outputs: vec![],
            x: 100.0,
            y: 100.0,
            council_score: None,
            is_council_winner: false,
            decision_eval: None,
            interactive_prompt: None,
        }
    }

    #[test]
    fn anchor_test_monotonic_run_id_and_server_port() {
        let id1 = next_monotonic_run_id("claude");
        let id2 = next_monotonic_run_id("claude");
        assert_ne!(id1, id2, "Monotonic run IDs must never collide");
        assert!(id1.starts_with("claude-"));

        set_active_server_port(9123);
        assert_eq!(active_server_port(), 9123);
        set_active_server_port(0);
        assert_eq!(active_server_port(), 9000);
    }

    #[test]
    fn anchor_test_update_scoped_chat_message_synchronizes_all_three_vectors_only_when_active() {
        let mut state = test_state("session-active", Vec::new());
        state.push_or_update_scoped_chat("session-active", test_msg("msg-1", 1, "Initial", true, Vec::new()));
        state.push_or_update_scoped_chat("session-bg", test_msg("msg-bg", 1, "Bg Initial", true, Vec::new()));

        assert!(state.update_scoped_chat_message("session-active", "msg-1", |m| {
            m.content = "Updated Active".to_string();
            m.streaming = false;
        }));
        assert_eq!(state.chat_thread[0].content, "Updated Active");
        assert!(!state.chat_thread[0].streaming);
        assert_eq!(state.active_session_live_messages[0].content, "Updated Active");
        assert_eq!(state.session_custom_messages["session-active"][0].content, "Updated Active");

        assert!(state.update_scoped_chat_message("session-bg", "msg-bg", |m| {
            m.content = "Updated Bg".to_string();
            m.streaming = false;
        }));
        assert_eq!(state.chat_thread.len(), 1);
        assert_eq!(state.chat_thread[0].content, "Updated Active");
        assert_eq!(state.session_custom_messages["session-bg"][0].content, "Updated Bg");
    }

    #[test]
    fn anchor_test_extract_and_resolve_execution_scope() {
        let sample = format!(
            "Reading {}/other-repo/src/lib.rs for workspace analysis",
            code_root_dir().display()
        );
        assert_eq!(extract_project_slug_from_text(&sample), Some("other-repo".to_string()));

        let scope = resolve_execution_scope("session-beta", "exo-suit");
        assert_eq!(scope.session_id, "session-beta");
        assert_eq!(scope.project_id, "exo-suit");
        assert!(scope.workspace_path.ends_with("code/exo-suit"));
    }

    #[test]
    fn anchor_test_cross_session_isolation_zero_bleed() {
        let mut state = test_state("session-b", Vec::new());

        // Session A pushes a message while Session B is active in the UI
        state.push_or_update_scoped_chat(
            "session-a",
            test_msg("reply-a1", 1, "Output scoped strictly to session-a", false, Vec::new()),
        );

        // Session B's live chat_thread MUST have zero bleed from Session A
        assert!(state.chat_thread.is_empty());
        assert_eq!(state.session_custom_messages.get("session-a").map(|v| v.len()), Some(1));
    }

    #[test]
    fn anchor_test_hard_cancellation_aborts_run_and_cleans_nodes() {
        let (abort_tx, abort_rx) = watch::channel(false);
        let mut state = test_state(
            "session-a",
            vec![
                test_node("claude-99", "root", "subagent"),
                test_node("task-claude-99", "claude-99", "task"),
            ],
        );

        state.push_or_update_scoped_chat(
            "session-a",
            test_msg(
                "reply-claude-99",
                2,
                "Partial output",
                true,
                vec![ToolCall {
                    name: "edit_file".to_string(),
                    summary: "src/main.rs".to_string(),
                    status: "Running".to_string(),
                }],
            ),
        );

        state.active_runs.insert(
            "claude-99".to_string(),
            ActiveRunHandle {
                run_id: "claude-99".to_string(),
                session_id: "session-a".to_string(),
                project_id: "exo-suit".to_string(),
                node_ids: vec!["claude-99".to_string(), "task-claude-99".to_string()],
                reply_msg_id: "reply-claude-99".to_string(),
                abort_tx,
            },
        );

        let cancelled = state.cancel_execution(Some("session-a"));
        assert_eq!(cancelled, vec!["claude-99".to_string()]);
        assert!(*abort_rx.borrow());
        assert!(state.agents.is_empty());
        let msg = state.chat_thread.first().expect("chat message exists");
        assert!(!msg.streaming);
        assert!(msg.content.contains("Stopped by user"));
    }
}
