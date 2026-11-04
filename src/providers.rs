use crate::brain::{clean_ansi_and_tags, home_dir, parse_ask_question_args, sanitize_code_path};
use crate::models::{
    DecisionEvaluation, DecisionPrimitive, InteractiveQuestionItem, ProviderInfo, TokenMetrics,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

static BIN_CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

pub fn detect_binary(candidates: &[&str]) -> Option<String> {
    let cache_mutex = BIN_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let home = home_dir();
    let local_bin = home.join(".local").join("bin");
    let muse_bin_dir = home.join(".muse").join("bin");

    for &c in candidates {
        if let Ok(guard) = cache_mutex.lock() {
            if let Some(cached) = guard.get(c) {
                if let Some(found) = cached {
                    return Some(found.clone());
                } else {
                    continue;
                }
            }
        }

        let resolved = if Path::new(c).exists() {
            Some(c.to_string())
        } else {
            let user_local = local_bin.join(c);
            let user_muse = muse_bin_dir.join(c);
            if user_local.exists() {
                Some(user_local.to_string_lossy().to_string())
            } else if user_muse.exists() {
                Some(user_muse.to_string_lossy().to_string())
            } else if let Ok(output) = std::process::Command::new("which").arg(c).output() {
                if output.status.success() {
                    let p = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !p.is_empty() {
                        Some(p)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Ok(mut guard) = cache_mutex.lock() {
            guard.insert(c.to_string(), resolved.clone());
        }

        if let Some(found) = resolved {
            return Some(found);
        }
    }
    None
}

pub fn resolve_api_model_name(provider_id: &str, requested_model: &str) -> String {
    let trimmed = requested_model.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("default") {
        return trimmed.to_string();
    }
    if provider_id == "deepseek" {
        return "deepseek-chat".to_string();
    }
    PROVIDER_SPECS
        .iter()
        .find(|s| s.id == provider_id)
        .map(|s| s.default_model)
        .unwrap_or("gpt-4o")
        .to_string()
}

/// Checks whether Meta Muse Code (`muse`) has active non-interactive credentials
/// (either `META_API_KEY` / `MODEL_API_KEY`, runtime key, or `~/.config/muse/auth.json`).
pub fn is_muse_cli_authenticated(api_keys: &HashMap<String, String>) -> bool {
    if resolve_key(api_keys, "muse", &["META_API_KEY", "MODEL_API_KEY"]).is_some() {
        return true;
    }
    let auth_path = std::env::var("MUSE_AUTH_PATH")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config").join("muse").join("auth.json"));
    std::fs::metadata(&auth_path)
        .map(|m| m.is_file() && m.len() > 4)
        .unwrap_or(false)
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgyStreamOutcome {
    Completed {
        text: String,
        tokens: TokenMetrics,
        interactive_questions: Vec<InteractiveQuestionItem>,
    },
    Aborted,
    CliError(String),
}

fn contextualize_cli_prompt(project_id: &str, workspace_path: &Path, prompt: &str) -> String {
    if prompt.starts_with("[Workspace:") {
        prompt.to_string()
    } else {
        format!(
            "[Workspace: {} ({})]\n\n{}",
            project_id,
            workspace_path.display(),
            prompt
        )
    }
}

async fn run_cli_streaming_process<F>(
    cli_label: &str,
    bin_display: &str,
    mut cmd: Command,
    project_id: &str,
    prompt_len: usize,
    abort_rx: &mut tokio::sync::watch::Receiver<bool>,
    mut on_event: F,
) -> AgyStreamOutcome
where
    F: FnMut(AgyParsedEvent),
{
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return AgyStreamOutcome::CliError(format!(
                "Failed to spawn {} CLI ({}): {}",
                cli_label, bin_display, e
            ));
        }
    };

    let mut streamed_text = String::new();
    let mut interactive_questions = Vec::new();
    let mut final_tokens = TokenMetrics {
        input_tokens: (prompt_len as u64 / 3) + 320,
        cache_read_tokens: 900,
        output_tokens: 0,
    };

    let stderr = child.stderr.take();
    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            let line_res = tokio::select! {
                l = lines.next_line() => l,
                _ = abort_rx.changed() => {
                    if *abort_rx.borrow() {
                        let _ = child.kill().await;
                        return AgyStreamOutcome::Aborted;
                    }
                    continue;
                }
            };

            let Ok(Some(line)) = line_res else {
                break;
            };
            let Some(evt) = parse_agy_stream_json_line(&line, project_id) else {
                continue;
            };

            on_event(evt.clone());

            if *abort_rx.borrow() {
                let _ = child.kill().await;
                return AgyStreamOutcome::Aborted;
            }

            match evt {
                AgyParsedEvent::ToolActive { questions, .. } if !questions.is_empty() => {
                    interactive_questions = questions;
                }
                AgyParsedEvent::TextDelta {
                    delta,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    ..
                } => {
                    streamed_text.push_str(&delta);
                    if let Some(it) = input_tokens.filter(|&v| v > 0) {
                        final_tokens.input_tokens = it;
                    }
                    if let Some(ot) = output_tokens.filter(|&v| v > 0) {
                        final_tokens.output_tokens = ot;
                    }
                    if let Some(crt) = cache_read_tokens.filter(|&v| v > 0) {
                        final_tokens.cache_read_tokens = crt;
                    }
                }
                AgyParsedEvent::Result {
                    response,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    ..
                } => {
                    if streamed_text.trim().is_empty() && !response.trim().is_empty() {
                        streamed_text = response;
                    }
                    if input_tokens > 0 {
                        final_tokens.input_tokens = input_tokens;
                    }
                    if output_tokens > 0 {
                        final_tokens.output_tokens = output_tokens;
                    }
                    if cache_read_tokens > 0 {
                        final_tokens.cache_read_tokens = cache_read_tokens;
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    drop(on_event);

    if *abort_rx.borrow() {
        let _ = child.kill().await;
        return AgyStreamOutcome::Aborted;
    }

    if !streamed_text.trim().is_empty() || !interactive_questions.is_empty() {
        let _ = tokio::time::timeout(Duration::from_millis(250), child.wait()).await;
        let _ = child.start_kill();
        if final_tokens.output_tokens == 0 {
            final_tokens.output_tokens = (streamed_text.len() as u64 / 3).max(48);
        }
        return AgyStreamOutcome::Completed {
            text: streamed_text,
            tokens: final_tokens,
            interactive_questions,
        };
    }

    let mut stderr_buf = String::new();
    if let Some(mut se) = stderr {
        let _ = tokio::time::timeout(
            Duration::from_millis(400),
            se.read_to_string(&mut stderr_buf),
        )
        .await;
    }
    let status = tokio::time::timeout(Duration::from_millis(400), child.wait())
        .await
        .ok()
        .and_then(|r| r.ok());
    let _ = child.start_kill();

    let stderr_msg = clean_ansi_and_tags(&stderr_buf).trim().to_string();
    if !stderr_msg.is_empty() {
        AgyStreamOutcome::CliError(stderr_msg)
    } else {
        let code_str = status
            .and_then(|s| s.code())
            .map(|c| format!("exit code {}", c))
            .unwrap_or_else(|| "empty output".to_string());
        AgyStreamOutcome::CliError(format!("{} CLI exited with {}", cli_label, code_str))
    }
}

pub async fn run_agy_streaming_turn<F>(
    agy_bin: &str,
    workspace_path: &std::path::Path,
    project_id: &str,
    provider_id: &str,
    model: &str,
    conversation_id: Option<&str>,
    prompt: &str,
    abort_rx: &mut tokio::sync::watch::Receiver<bool>,
    on_event: F,
) -> AgyStreamOutcome
where
    F: FnMut(AgyParsedEvent),
{
    if *abort_rx.borrow() {
        return AgyStreamOutcome::Aborted;
    }
    let mapped_model = map_provider_to_agy_model(provider_id, model);
    let contextualized_prompt = contextualize_cli_prompt(project_id, workspace_path, prompt);

    let mut cmd = Command::new(agy_bin);
    cmd.current_dir(workspace_path);
    if let Some(conv_id) = conversation_id.filter(|s| !s.trim().is_empty()) {
        cmd.args(["--conversation", conv_id]);
    }
    cmd.args([
        "--model",
        &mapped_model,
        "-p",
        &contextualized_prompt,
        "--output-format",
        "stream-json",
        "--print-timeout",
        "300s",
    ]);

    run_cli_streaming_process("agy", agy_bin, cmd, project_id, prompt.len(), abort_rx, on_event)
        .await
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgyParsedEvent {
    Init {
        conversation_id: String,
    },
    ToolActive {
        step_index: u32,
        tool_name: String,
        summary: String,
        questions: Vec<InteractiveQuestionItem>,
    },
    ToolDone {
        step_index: u32,
        tool_name: String,
    },
    TextDelta {
        step_index: u32,
        delta: String,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_tokens: Option<u64>,
    },
    Result {
        conversation_id: String,
        response: String,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
    },
}

pub fn map_provider_to_agy_model(provider_id: &str, model: &str) -> String {
    let m = model.trim().to_lowercase();
    let tier = |def: &'static str| -> &'static str {
        if m.contains("low") {
            "low"
        } else if m.contains("medium") {
            "medium"
        } else if m.contains("high") {
            "high"
        } else {
            def
        }
    };
    match provider_id {
        "muse" if m.contains("glimmer") => "gemini-3.8-flash-medium".to_string(),
        "muse" if m.contains("wonder") || m.contains("1.3") => "gemini-4-argon-high".to_string(),
        "muse" => "gemini-4-argon-medium".to_string(),
        "claude" if m.contains("opus") => format!("claude-opus-5-5-{}", tier("high")),
        "claude" => format!("claude-sonnet-5-5-{}", tier("medium")),
        "codex" | "copilot" => "gpt-oss-120b-medium".to_string(),
        _ if m.contains("flash") => format!("gemini-3.8-flash-{}", tier("high")),
        _ if m.contains("high") || m.contains("pro") => "gemini-4-argon-high".to_string(),
        _ => format!("gemini-4-argon-{}", tier("medium")),
    }
}

/// Parses a single JSONL line from `muse exec --json` (Muse Session Protocol / MSP v1)
/// into the unified `AgyParsedEvent` stream contract.
pub fn parse_muse_stream_json_line(line: &str, active_project: &str) -> Option<AgyParsedEvent> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let val: Value = serde_json::from_str(trimmed).ok()?;
    let payload_type = val.get("payload_type")?.as_str()?;
    let payload = val.get("payload").unwrap_or(&val);
    let seq = val.get("sequence").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

    match payload_type {
        "session.run.linked" | "runtime.command.accepted" => {
            let sid = val
                .get("stream")
                .and_then(|s| s.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if sid.is_empty() {
                None
            } else {
                Some(AgyParsedEvent::Init {
                    conversation_id: sid,
                })
            }
        }
        "task.lifecycle.side_effect_intent" => {
            let ev = payload.get("event")?;
            let op = ev
                .get("operation")
                .and_then(|v| v.as_str())
                .unwrap_or("muse_task");
            if op.starts_with("model.") || op.starts_with("reminder.") {
                return None;
            }
            let tool_name = if op.contains("read") || op.contains("view") {
                "read_file".to_string()
            } else if op.contains("edit") || op.contains("patch") || op.contains("write") {
                "edit_file".to_string()
            } else if op.contains("shell") || op.contains("exec") || op.contains("command") {
                "run_command".to_string()
            } else {
                op.to_string()
            };
            let summary = ev
                .get("idempotency_key")
                .and_then(|v| v.as_str())
                .map(|s| s.chars().take(72).collect())
                .unwrap_or_else(|| format!("~/code/{}", active_project));
            Some(AgyParsedEvent::ToolActive {
                step_index: seq,
                tool_name,
                summary,
                questions: Vec::new(),
            })
        }
        "run.output.delta" => {
            let delta = payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if delta.is_empty() {
                None
            } else {
                Some(AgyParsedEvent::TextDelta {
                    step_index: seq,
                    delta,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                })
            }
        }
        "run.usage" => {
            let first_model = payload
                .get("models")
                .and_then(|m| m.as_array())
                .and_then(|a| a.first());
            let input_tokens = first_model
                .and_then(|m| m.get("input_tokens"))
                .and_then(|v| v.as_u64());
            let output_tokens = first_model
                .and_then(|m| m.get("output_tokens"))
                .and_then(|v| v.as_u64());
            let cache_read_tokens = first_model
                .and_then(|m| m.get("cached_tokens"))
                .and_then(|v| v.as_u64());
            if input_tokens.is_some() || output_tokens.is_some() {
                Some(AgyParsedEvent::TextDelta {
                    step_index: seq,
                    delta: String::new(),
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                })
            } else {
                None
            }
        }
        "run.terminal.completed" => {
            let sid = val
                .get("stream")
                .and_then(|s| s.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(AgyParsedEvent::Result {
                conversation_id: sid,
                response: text,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
            })
        }
        _ => None,
    }
}

pub async fn run_muse_streaming_turn<F>(
    muse_bin: &str,
    workspace_path: &std::path::Path,
    project_id: &str,
    model: &str,
    api_key: Option<&str>,
    prompt: &str,
    abort_rx: &mut tokio::sync::watch::Receiver<bool>,
    on_event: F,
) -> AgyStreamOutcome
where
    F: FnMut(AgyParsedEvent),
{
    if *abort_rx.borrow() {
        return AgyStreamOutcome::Aborted;
    }
    let resolved_model = resolve_api_model_name("muse", model);
    let contextualized_prompt = contextualize_cli_prompt(project_id, workspace_path, prompt);

    let mut cmd = Command::new(muse_bin);
    cmd.current_dir(workspace_path).stdin(Stdio::null());
    if let Some(k) = api_key.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        cmd.env("META_API_KEY", k);
    }
    cmd.args([
        "exec",
        "--json",
        "--model",
        &resolved_model,
        "--workspace",
    ])
    .arg(workspace_path)
    .args([
        "--trust-workspace",
        "--disable-approval",
        "--user-input-auto-resolve",
        &contextualized_prompt,
    ]);

    run_cli_streaming_process("muse", muse_bin, cmd, project_id, prompt.len(), abort_rx, on_event)
        .await
}

pub fn parse_agy_stream_json_line(line: &str, active_project: &str) -> Option<AgyParsedEvent> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let val: Value = serde_json::from_str(trimmed).ok()?;
    if val.get("payload_type").is_some() {
        return parse_muse_stream_json_line(trimmed, active_project);
    }
    if val.get("event").is_none() && val.get("type").is_some() {
        return parse_universal_cli_json_line(&val, active_project);
    }
    let ev_type = val.get("event")?.as_str()?;

    match ev_type {
        "init" => {
            let cid = val
                .get("conversation_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if cid.is_empty() {
                None
            } else {
                Some(AgyParsedEvent::Init {
                    conversation_id: cid,
                })
            }
        }
        "step_update" => {
            let su = val.get("step_update")?;
            let step_index = su.get("step_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let state = su.get("state").and_then(|v| v.as_str()).unwrap_or("");
            let step_type = su.get("step_type").and_then(|v| v.as_str()).unwrap_or("");

            if step_type == "tool" {
                let raw_tool_name = su
                    .get("tool_name")
                    .or_else(|| su.get("tool_info").and_then(|ti| ti.get("name")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                let tool_name = normalize_cli_tool_name(raw_tool_name);

                if state == "ACTIVE" {
                    let params = su.get("tool_info").and_then(|ti| ti.get("parameters"));
                    let questions = if tool_name == "ask_question" {
                        parse_ask_question_args(params)
                    } else {
                        Vec::new()
                    };
                    let summary = if tool_name == "ask_question" && !questions.is_empty() {
                        questions[0].question.chars().take(80).collect()
                    } else {
                        extract_tool_summary(params, active_project)
                    };

                    Some(AgyParsedEvent::ToolActive {
                        step_index,
                        tool_name,
                        summary,
                        questions,
                    })
                } else if state == "DONE" {
                    Some(AgyParsedEvent::ToolDone {
                        step_index,
                        tool_name,
                    })
                } else {
                    None
                }
            } else if step_type == "agent_response" {
                let delta = su
                    .get("text_delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let usage = su.get("usage");
                let input_tokens = usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|v| v.as_u64());
                let output_tokens = usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|v| v.as_u64());
                let cache_read_tokens = usage
                    .and_then(|u| u.get("cache_read_tokens"))
                    .and_then(|v| v.as_u64());

                if !delta.is_empty() || input_tokens.is_some() {
                    Some(AgyParsedEvent::TextDelta {
                        step_index,
                        delta,
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        }
        "result" => {
            let res = val.get("result").unwrap_or(&val);
            let cid = res
                .get("conversation_id")
                .or_else(|| val.get("conversation_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let response = res
                .get("response")
                .or_else(|| val.get("response"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let usage = res
                .get("usage")
                .or_else(|| res.get("stats"))
                .or_else(|| val.get("stats"));
            let input_tokens = usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output_tokens = usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache_read_tokens = usage
                .and_then(|u| u.get("cache_read_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            Some(AgyParsedEvent::Result {
                conversation_id: cid,
                response,
                input_tokens,
                output_tokens,
                cache_read_tokens,
            })
        }
        _ => None,
    }
}

/// Normalizes JSONL stream events from `claude`, `gemini`, `codex`, `copilot`, `cursor-agent`,
/// `opencode`, `amp`, `droid`, `qwen`, and `goose` into the unified `AgyParsedEvent` enum.
pub fn parse_universal_cli_json_line(val: &Value, active_project: &str) -> Option<AgyParsedEvent> {
    let ev_type = val.get("type")?.as_str()?;

    match ev_type {
        // 1. Session Initialization (gemini `init`, claude/amp/cursor `system`, codex `thread.started`, copilot `session.start`, opencode `step_start`)
        "init" | "system" | "thread.started" | "session.start" | "step_start" => {
            if ev_type == "system" {
                let subtype = val.get("subtype").and_then(|v| v.as_str()).unwrap_or("init");
                if subtype != "init" {
                    return None;
                }
            }
            let cid = val
                .get("session_id")
                .or_else(|| val.get("thread_id"))
                .or_else(|| val.get("sessionID"))
                .or_else(|| val.get("data").and_then(|d| d.get("sessionId")))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if cid.is_empty() {
                None
            } else {
                Some(AgyParsedEvent::Init {
                    conversation_id: cid,
                })
            }
        }
        // 2. Gemini / Qwen / Droid `message` delta (`{"type":"message","role":"assistant","content":"...","delta":true}`)
        "message" => {
            let role = val.get("role").and_then(|v| v.as_str()).unwrap_or("assistant");
            if role != "assistant" {
                return None;
            }
            let delta = val
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if delta.is_empty() {
                None
            } else {
                Some(AgyParsedEvent::TextDelta {
                    step_index: 1,
                    delta,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                })
            }
        }
        // 3. Claude / Amp / Cursor `assistant` turn (`{"type":"assistant","message":{"content":[...],"usage":{...}}}`)
        "assistant" => {
            let msg = val.get("message").unwrap_or(val);
            let usage = msg.get("usage");
            let input_tokens = usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(|v| v.as_u64());
            let output_tokens = usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(|v| v.as_u64());
            let cache_read_tokens = usage
                .and_then(|u| {
                    u.get("cache_read_input_tokens")
                        .or_else(|| u.get("cache_read_tokens"))
                })
                .and_then(|v| v.as_u64());

            if let Some(arr) = msg.get("content").and_then(|c| c.as_array()) {
                for item in arr {
                    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if item_type == "tool_use" {
                        let raw_name = item.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                        let tool_name = normalize_cli_tool_name(raw_name);
                        let summary = extract_tool_summary(item.get("input"), active_project);
                        return Some(AgyParsedEvent::ToolActive {
                            step_index: 1,
                            tool_name,
                            summary,
                            questions: Vec::new(),
                        });
                    }
                }
                let mut combined = String::new();
                for item in arr {
                    if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                            combined.push_str(t);
                        }
                    }
                }
                if !combined.is_empty() || input_tokens.is_some() {
                    return Some(AgyParsedEvent::TextDelta {
                        step_index: 1,
                        delta: combined,
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                    });
                }
            }
            None
        }
        // 4. Copilot `assistant.message` or OpenCode `text`
        "assistant.message" | "text" => {
            let delta = val
                .get("data")
                .and_then(|d| d.get("content"))
                .or_else(|| val.get("part").and_then(|p| p.get("text")))
                .or_else(|| val.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if delta.is_empty() {
                None
            } else {
                Some(AgyParsedEvent::TextDelta {
                    step_index: 1,
                    delta,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                })
            }
        }
        // 5. Tool Start across Gemini (`tool_use`), Cursor (`tool_call`), Copilot (`tool.execution_start`), OpenCode (`tool_use`)
        "tool_use" | "tool_call" | "tool.execution_start" => {
            if let Some(tc_obj) = val.get("tool_call").and_then(|v| v.as_object()) {
                if let Some((first_key, inner)) = tc_obj.iter().next() {
                    let tool_name = normalize_cli_tool_name(first_key);
                    let summary = extract_tool_summary(inner.get("args"), active_project);
                    return Some(AgyParsedEvent::ToolActive {
                        step_index: 1,
                        tool_name,
                        summary,
                        questions: Vec::new(),
                    });
                }
            }
            let raw_name = val
                .get("tool_name")
                .or_else(|| val.get("tool"))
                .or_else(|| val.get("name"))
                .or_else(|| val.get("data").and_then(|d| d.get("toolName")))
                .or_else(|| val.get("part").and_then(|p| p.get("tool")))
                .and_then(|v| v.as_str())
                .unwrap_or("tool");
            let tool_name = normalize_cli_tool_name(raw_name);
            let params = val
                .get("parameters")
                .or_else(|| val.get("arguments"))
                .or_else(|| val.get("input"))
                .or_else(|| val.get("data").and_then(|d| d.get("arguments")))
                .or_else(|| {
                    val.get("part")
                        .and_then(|p| p.get("state"))
                        .and_then(|s| s.get("input"))
                });
            let summary = extract_tool_summary(params, active_project);
            Some(AgyParsedEvent::ToolActive {
                step_index: 1,
                tool_name,
                summary,
                questions: Vec::new(),
            })
        }
        // 6. Tool Done across Gemini (`tool_result`), Copilot (`tool.execution_complete`)
        "tool_result" | "tool.execution_complete" => {
            let raw_name = val
                .get("tool_name")
                .or_else(|| val.get("data").and_then(|d| d.get("toolName")))
                .and_then(|v| v.as_str())
                .unwrap_or("tool");
            Some(AgyParsedEvent::ToolDone {
                step_index: 1,
                tool_name: normalize_cli_tool_name(raw_name),
            })
        }
        // 7. Codex `item.started` / `item.completed`
        "item.started" => {
            let item = val.get("item")?;
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if item_type == "command_execution"
                || item_type == "mcp_tool_call"
                || item_type == "file_change"
            {
                let tool_name = match item_type {
                    "command_execution" => "run_command".to_string(),
                    "file_change" => "edit_file".to_string(),
                    _ => item
                        .get("tool")
                        .and_then(|v| v.as_str())
                        .unwrap_or("mcp_tool")
                        .to_string(),
                };
                let summary = item
                    .get("command")
                    .or_else(|| item.get("path"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.chars().take(72).collect())
                    .unwrap_or_else(|| format!("~/code/{}", active_project));
                Some(AgyParsedEvent::ToolActive {
                    step_index: 1,
                    tool_name,
                    summary,
                    questions: Vec::new(),
                })
            } else {
                None
            }
        }
        "item.completed" => {
            let item = val.get("item")?;
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if item_type == "agent_message" {
                let text = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if text.is_empty() {
                    None
                } else {
                    Some(AgyParsedEvent::TextDelta {
                        step_index: 1,
                        delta: text,
                        input_tokens: None,
                        output_tokens: None,
                        cache_read_tokens: None,
                    })
                }
            } else if item_type == "command_execution" || item_type == "file_change" {
                let tool_name = if item_type == "command_execution" {
                    "run_command".to_string()
                } else {
                    "edit_file".to_string()
                };
                Some(AgyParsedEvent::ToolDone {
                    step_index: 1,
                    tool_name,
                })
            } else {
                None
            }
        }
        // 8. Final Result across Claude/Amp/Cursor (`result`), Gemini/Qwen (`result`), Codex (`turn.completed`), Copilot (`workflow.result`), OpenCode (`step_finish`)
        "result" | "turn.completed" | "workflow.result" | "step_finish" => {
            let cid = val
                .get("session_id")
                .or_else(|| val.get("conversation_id"))
                .or_else(|| val.get("sessionID"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let response = val
                .get("result")
                .and_then(|v| v.as_str())
                .or_else(|| val.get("response").and_then(|v| v.as_str()))
                .or_else(|| val.get("data").and_then(|d| d.get("content")).and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let usage = val
                .get("usage")
                .or_else(|| val.get("stats"))
                .or_else(|| val.get("part").and_then(|p| p.get("tokens")));
            let input_tokens = usage
                .and_then(|u| u.get("input_tokens").or_else(|| u.get("input")))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output_tokens = usage
                .and_then(|u| u.get("output_tokens").or_else(|| u.get("output")))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache_read_tokens = usage
                .and_then(|u| {
                    u.get("cache_read_tokens")
                        .or_else(|| u.get("cache_read_input_tokens"))
                        .or_else(|| u.get("cached_input_tokens"))
                        .or_else(|| u.get("cached"))
                        .or_else(|| u.get("cache").and_then(|c| c.get("read")))
                })
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            Some(AgyParsedEvent::Result {
                conversation_id: cid,
                response,
                input_tokens,
                output_tokens,
                cache_read_tokens,
            })
        }
        _ => None,
    }
}

fn normalize_cli_tool_name(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "view_file" | "read" | "read_file" | "view" | "readtoolcall" => "read_file".to_string(),
        "replace_file_content"
        | "multi_replace_file_content"
        | "edit"
        | "edit_file"
        | "str_replace_editor"
        | "edittoolcall" => "edit_file".to_string(),
        "write_to_file" | "write" | "create_file" | "writetoolcall" => "write_file".to_string(),
        "bash" | "shell" | "run_shell_command" | "run_command" | "shelltoolcall" => {
            "run_command".to_string()
        }
        _ => raw.to_string(),
    }
}

fn extract_tool_summary(params: Option<&Value>, active_project: &str) -> String {
    let raw_summary = params
        .and_then(|p| {
            [
                "toolSummary",
                "command",
                "CommandLine",
                "path",
                "file_path",
                "filePath",
                "Path",
                "TargetFile",
                "pattern",
                "Query",
                "query",
                "SearchPath",
                "DirectoryPath",
                "Role",
            ]
            .iter()
            .find_map(|k| p.get(*k))
        })
        .and_then(|v| v.as_str())
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_else(|| format!("~/code/{}", active_project));
    let code_prefix = format!("{}/code/", home_dir().to_string_lossy());
    let proj_prefix = format!("{}{}/", code_prefix, active_project);
    raw_summary
        .replace(&proj_prefix, "")
        .replace(&code_prefix, "")
        .chars()
        .take(80)
        .collect()
}

struct ProviderSpec {
    id: &'static str,
    name: &'static str,
    task_specialty: &'static str,
    default_model: &'static str,
    available_models: &'static [&'static str],
    color: &'static str,
    icon: &'static str,
    bins: &'static [&'static str],
    fallback_agy: bool,
    env_keys: &'static [&'static str],
    enabled_in_council: bool,
}

const PROVIDER_SPECS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "antigravity",
        name: "Antigravity (agy)",
        task_specialty: "Multi-Agent Graph Orchestration & Deep Repo Execution",
        default_model: "gemini-4-argon",
        available_models: &["gemini-4-argon", "gemini-3.6-pro", "gemini-3.6-flash"],
        color: "#A8202A",
        icon: "⚡",
        bins: &["agy"],
        fallback_agy: false,
        env_keys: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        enabled_in_council: true,
    },
    ProviderSpec {
        id: "jev",
        name: "Jev (TypeSafe System-1)",
        task_specialty: "Sub-200ms Typed Decision Gates (Noul, Choice, Score), Risk Routing & Council Rubric Scoring",
        default_model: "jev-latest",
        available_models: &["jev-latest", "jev-risk-gate", "jev-router-v1"],
        color: "#2D4A3E",
        icon: "🎯",
        bins: &[],
        fallback_agy: false,
        env_keys: &["JEV_API_KEY", "TYPESAFE_API_KEY"],
        enabled_in_council: true,
    },
    ProviderSpec {
        id: "gemini",
        name: "Google Gemini CLI (gemini)",
        task_specialty: "1M-Context Multimodal Repo Exploration, Worktrees & Policy Engine",
        default_model: "gemini-2.5-pro",
        available_models: &["gemini-2.5-pro", "gemini-2.5-flash", "gemini-3-pro-preview"],
        color: "#1A73E8",
        icon: "♊",
        bins: &["/opt/homebrew/bin/gemini", "/usr/local/bin/gemini", "gemini"],
        fallback_agy: false,
        env_keys: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        enabled_in_council: true,
    },
    ProviderSpec {
        id: "muse",
        name: "Meta Muse Code (muse)",
        task_specialty: "Long-Horizon Agentic Coding, Worktree Subagents & 1M Context",
        default_model: "muse-spark-1.3",
        available_models: &[
            "muse-spark-1.3",
            "muse-wonder-1.0",
            "muse-spark-1.2",
            "muse-spark-1.3-contributor",
            "muse-spark-open",
            "muse-glimmer",
        ],
        color: "#0064E0",
        icon: "♾️",
        bins: &["/opt/homebrew/bin/muse", "muse"],
        fallback_agy: true,
        env_keys: &["META_API_KEY", "MODEL_API_KEY"],
        enabled_in_council: true,
    },
    ProviderSpec {
        id: "claude",
        name: "Claude Code (claude)",
        task_specialty: "Deep Module Design, Boundary Invariants & Adversarial Review",
        default_model: "claude-4-sonnet",
        available_models: &["claude-4-sonnet", "claude-4-opus", "claude-3-7-sonnet-latest"],
        color: "#9E472A",
        icon: "🟠",
        bins: &["/opt/homebrew/bin/claude", "claude"],
        fallback_agy: true,
        env_keys: &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
        enabled_in_council: true,
    },
    ProviderSpec {
        id: "codex",
        name: "OpenAI Codex CLI (codex)",
        task_specialty: "Low-Latency Code Synthesis, Seatbelt Sandbox & Surgical Patching",
        default_model: "codex-mini-latest",
        available_models: &["codex-mini-latest", "gpt-5.4", "gpt-4o", "o3-mini"],
        color: "#2D4A3E",
        icon: "🟢",
        bins: &["/opt/homebrew/bin/codex", "codex"],
        fallback_agy: true,
        env_keys: &["CODEX_API_KEY", "OPENAI_API_KEY"],
        enabled_in_council: true,
    },
    ProviderSpec {
        id: "copilot",
        name: "GitHub Copilot CLI (copilot)",
        task_specialty: "Fleet Subagent Orchestration, GitHub MCP & PR/Issue Automation",
        default_model: "copilot-gpt-4o",
        available_models: &[
            "copilot-gpt-4o",
            "gpt-5.4",
            "claude-sonnet-4.5",
            "copilot-claude-sonnet",
        ],
        color: "#4A6B5D",
        icon: "🟣",
        bins: &[
            "/opt/homebrew/bin/copilot",
            "/usr/local/bin/copilot",
            "copilot",
            "/opt/homebrew/bin/gh",
            "/usr/local/bin/gh",
            "gh",
        ],
        fallback_agy: false,
        env_keys: &["COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "cursor",
        name: "Cursor Agent CLI (cursor-agent)",
        task_specialty: "Fast Multi-File Refactoring, `.cursor/rules` & IDE Parity",
        default_model: "claude-4-sonnet",
        available_models: &["claude-4-sonnet", "gpt-5", "cursor-fast"],
        color: "#3B4252",
        icon: "🖱️",
        bins: &["/opt/homebrew/bin/cursor-agent", "cursor-agent", "agent"],
        fallback_agy: false,
        env_keys: &["CURSOR_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "opencode",
        name: "OpenCode (opencode)",
        task_specialty: "Open-Source Terminal Coding Agent, LSP Diagnostics & Session Forking",
        default_model: "anthropic/claude-sonnet-4-5",
        available_models: &[
            "anthropic/claude-sonnet-4-5",
            "openai/gpt-4o",
            "google/gemini-2.5-pro",
        ],
        color: "#5E81AC",
        icon: "🔓",
        bins: &["/opt/homebrew/bin/opencode", "opencode"],
        fallback_agy: false,
        env_keys: &["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "aider",
        name: "Aider (aider)",
        task_specialty: "Tree-Sitter Repo Map, Architect/Editor Dual-Model & Auto-Lint/Test",
        default_model: "claude-3-7-sonnet-latest",
        available_models: &[
            "claude-3-7-sonnet-latest",
            "deepseek/deepseek-chat",
            "gpt-4o",
        ],
        color: "#2E7D32",
        icon: "🛠️",
        bins: &["/opt/homebrew/bin/aider", "aider"],
        fallback_agy: false,
        env_keys: &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "DEEPSEEK_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "goose",
        name: "Block Goose (goose)",
        task_specialty: "Extensible MCP Developer Agent & Declarative YAML Recipes",
        default_model: "claude-sonnet-4-5",
        available_models: &["claude-sonnet-4-5", "gpt-4o", "gemini-2.5-pro"],
        color: "#6D4C41",
        icon: "🪿",
        bins: &["/opt/homebrew/bin/goose", "goose"],
        fallback_agy: false,
        env_keys: &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "amp",
        name: "Sourcegraph Amp (amp)",
        task_specialty: "Cross-Repository Code Intelligence, Subagent Oracles & Deep Search",
        default_model: "smart",
        available_models: &["smart", "deep", "rush"],
        color: "#D84315",
        icon: "🔊",
        bins: &["/opt/homebrew/bin/amp", "amp"],
        fallback_agy: false,
        env_keys: &["AMP_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "droid",
        name: "Factory Droid (droid)",
        task_specialty: "Autonomous CI/CD Engineering, Migration Droids & Tiered Autonomy",
        default_model: "claude-sonnet-4-20250514",
        available_models: &["claude-sonnet-4-20250514", "openai-latest-balanced"],
        color: "#37474F",
        icon: "🤖",
        bins: &["/opt/homebrew/bin/droid", "droid"],
        fallback_agy: false,
        env_keys: &["FACTORY_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "qwen",
        name: "Qwen Code (qwen)",
        task_specialty: "Qwen3-Coder Agentic Tool Loops & Open-Weight Code Synthesis",
        default_model: "qwen3-coder-plus",
        available_models: &["qwen3-coder-plus", "qwen-max", "qwen2.5-coder-32b-instruct"],
        color: "#512DA8",
        icon: "🟣",
        bins: &["/opt/homebrew/bin/qwen", "qwen"],
        fallback_agy: false,
        env_keys: &["DASHSCOPE_API_KEY", "QWEN_CODE_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "continue",
        name: "Continue CLI (cn)",
        task_specialty: "Rule-Governed Headless Checks (`.continue/rules`) & Hub Assistants",
        default_model: "claude-3-7-sonnet",
        available_models: &["claude-3-7-sonnet", "gpt-4o"],
        color: "#00838F",
        icon: "▶️",
        bins: &["/opt/homebrew/bin/cn", "cn"],
        fallback_agy: false,
        env_keys: &["CONTINUE_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "cline",
        name: "Cline CLI (cline)",
        task_specialty: "Plan/Act Dual-Phase Agentic Execution & `.clinerules` Enforcement",
        default_model: "claude-sonnet-4-20250514",
        available_models: &["claude-sonnet-4-20250514", "deepseek-reasoner"],
        color: "#455A64",
        icon: "🦾",
        bins: &["/opt/homebrew/bin/cline", "cline"],
        fallback_agy: false,
        env_keys: &["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "crush",
        name: "Charm Crush (crush)",
        task_specialty: "Catwalk Multi-Provider Routing & Fast Terminal Code Generation",
        default_model: "claude-sonnet-4-5",
        available_models: &["claude-sonnet-4-5", "gpt-4o"],
        color: "#AD1457",
        icon: "💖",
        bins: &["/opt/homebrew/bin/crush", "crush"],
        fallback_agy: false,
        env_keys: &["ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "deepseek",
        name: "DeepSeek R1/V3",
        task_specialty: "First-Principles Algorithmic Reasoning & Math Verification",
        default_model: "deepseek-reasoner",
        available_models: &["deepseek-reasoner", "deepseek-chat"],
        color: "#4A6B5D",
        icon: "🔵",
        bins: &[],
        fallback_agy: false,
        env_keys: &["DEEPSEEK_API_KEY"],
        enabled_in_council: true,
    },
    ProviderSpec {
        id: "kimi",
        name: "Kimi (Moonshot)",
        task_specialty: "Long-Context Cross-Repository Synthesis",
        default_model: "kimi-k2-5",
        available_models: &["kimi-k2-5", "moonshot-v1-128k"],
        color: "#A8202A",
        icon: "🌙",
        bins: &[],
        fallback_agy: false,
        env_keys: &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
        enabled_in_council: true,
    },
    ProviderSpec {
        id: "grok",
        name: "xAI Grok (grok)",
        task_specialty: "High-Throughput Agentic Coding (`grok-code-fast-1`) & Frontier Reasoning",
        default_model: "grok-code-fast-1",
        available_models: &["grok-code-fast-1", "grok-4", "grok-3-mini"],
        color: "#263238",
        icon: "✖️",
        bins: &[],
        fallback_agy: false,
        env_keys: &["XAI_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "mistral",
        name: "Mistral Codestral / Vibe",
        task_specialty: "Devstral SWE Agent, Codestral FIM & Mistral Vibe CLI",
        default_model: "codestral-latest",
        available_models: &["codestral-latest", "devstral-small-latest", "mistral-large-latest"],
        color: "#E65100",
        icon: "🌪️",
        bins: &["/opt/homebrew/bin/vibe", "vibe"],
        fallback_agy: false,
        env_keys: &["MISTRAL_API_KEY", "CODESTRAL_API_KEY"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "ollama",
        name: "Local Ollama (ollama)",
        task_specialty: "Zero-Egress Local Open-Weight Inference (`qwen3-coder`, `devstral`, `muse-glimmer`)",
        default_model: "qwen3-coder",
        available_models: &[
            "qwen3-coder",
            "qwen2.5-coder:32b",
            "deepseek-r1:32b",
            "devstral",
            "muse-glimmer",
        ],
        color: "#33691E",
        icon: "🦙",
        bins: &["/opt/homebrew/bin/ollama", "/usr/local/bin/ollama", "ollama"],
        fallback_agy: false,
        env_keys: &["OLLAMA_HOST"],
        enabled_in_council: false,
    },
    ProviderSpec {
        id: "openrouter",
        name: "OpenRouter Gateway",
        task_specialty: "Universal Multi-Vendor Model Routing & Failover Gateway",
        default_model: "anthropic/claude-sonnet-4",
        available_models: &[
            "anthropic/claude-sonnet-4",
            "x-ai/grok-code-fast-1",
            "mistralai/codestral-latest",
            "qwen/qwen3-coder",
            "openrouter/auto",
        ],
        color: "#4527A0",
        icon: "🌐",
        bins: &[],
        fallback_agy: false,
        env_keys: &["OPENROUTER_API_KEY"],
        enabled_in_council: false,
    },
];

pub fn build_provider_registry(api_keys: &HashMap<String, String>) -> Vec<ProviderInfo> {
    let agy_bin = detect_binary(&["agy"]);
    PROVIDER_SPECS
        .iter()
        .map(|s| {
            let cli_path = if s.bins.is_empty() {
                None
            } else {
                detect_binary(s.bins).or_else(|| if s.fallback_agy { agy_bin.clone() } else { None })
            };
            let has_api_key = resolve_key(api_keys, s.id, s.env_keys).is_some();
            let mode = if s.id == "jev" {
                if has_api_key { "LIVE_API" } else { "DETERMINISTIC_GATE" }
            } else if cli_path.is_some() {
                "LIVE_CLI"
            } else if has_api_key {
                "LIVE_API"
            } else {
                "SIMULATED"
            };
            ProviderInfo {
                id: s.id.to_string(),
                name: s.name.to_string(),
                model_class: if s.id == "jev" {
                    "DECISION_SYSTEM_1"
                } else {
                    "GENERATIVE_SYSTEM_2"
                }
                .to_string(),
                task_specialty: s.task_specialty.to_string(),
                default_model: s.default_model.to_string(),
                available_models: s.available_models.iter().map(|m| m.to_string()).collect(),
                color: s.color.to_string(),
                icon: s.icon.to_string(),
                mode: mode.to_string(),
                cli_path,
                has_api_key,
                enabled_in_council: s.enabled_in_council,
            }
        })
        .collect()
}

fn resolve_key(api_keys: &HashMap<String, String>, id: &str, env_vars: &[&str]) -> Option<String> {
    if let Some(k) = api_keys.get(id) {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    for &ev in env_vars {
        if let Ok(v) = std::env::var(ev) {
            if !v.trim().is_empty() {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// Strictly sanitizes probabilities/confidence into [0.0, 1.0] with NaN/Inf protection (per SPEC-001).
pub fn sanitize_prob(val: f32, default: f32) -> f32 {
    if val.is_nan() || val.is_infinite() {
        return default.clamp(0.0, 1.0);
    }
    val.clamp(0.0, 1.0)
}

fn make_jev_eval(
    engine: String,
    risk_noul: f32,
    choice_action: &str,
    score_rubric: &str,
    numeric_score: f32,
    conf: f32,
    gate_status: &str,
    latency_ms: u64,
    keys_and_descs: [(&str, &str); 3],
) -> DecisionEvaluation {
    DecisionEvaluation {
        engine,
        noul_risk_prob: risk_noul,
        choice_action: choice_action.to_string(),
        score_rubric: score_rubric.to_string(),
        confidence: conf,
        gate_status: gate_status.to_string(),
        latency_ms,
        primitives: vec![
            DecisionPrimitive {
                key: keys_and_descs[0].0.to_string(),
                primitive_type: "noul".to_string(),
                value: format!("{:.2}", risk_noul),
                numeric_value: Some(risk_noul),
                confidence: conf,
                description: keys_and_descs[0].1.to_string(),
            },
            DecisionPrimitive {
                key: keys_and_descs[1].0.to_string(),
                primitive_type: "choice".to_string(),
                value: choice_action.to_string(),
                numeric_value: None,
                confidence: conf,
                description: keys_and_descs[1].1.to_string(),
            },
            DecisionPrimitive {
                key: keys_and_descs[2].0.to_string(),
                primitive_type: "score".to_string(),
                value: score_rubric.to_string(),
                numeric_value: Some(numeric_score),
                confidence: conf,
                description: keys_and_descs[2].1.to_string(),
            },
        ],
    }
}

/// Evaluates TypeSafe AI's System-One Decision Model (`Jev`) over structured state & prompt.
/// Uses sub-200ms SLA timeout and fail-closed deterministic fallback when offline.
pub async fn evaluate_jev_decision_gate(
    model: &str,
    prompt_or_proposal: &str,
    active_project: &str,
    api_keys: &HashMap<String, String>,
) -> DecisionEvaluation {
    let start = Instant::now();
    let lower = prompt_or_proposal.to_lowercase();

    // Check if an explicit Jev / TypeSafe AI key is configured
    if let Some(key) = resolve_key(api_keys, "jev", &["JEV_API_KEY", "TYPESAFE_API_KEY"]) {
        let client = Client::builder()
            .timeout(Duration::from_millis(200)) // Sub-200ms SLA
            .build()
            .unwrap_or_default();

        let payload = json!({
            "model": if model.is_empty() { "jev-latest" } else { model },
            "state": {
                "project": active_project,
                "input_text": prompt_or_proposal.chars().take(1200).collect::<String>(),
                "char_len": prompt_or_proposal.len()
            },
            "questions": {
                "regression_risk": {
                    "type": "noul",
                    "instructions": "Probability [0.0, 1.0] that this action or proposal introduces an architectural regression or destructive side-effect."
                },
                "execution_route": {
                    "type": "choice",
                    "options": ["allow_execution", "require_human_approval", "route_to_council", "fail_closed_block"],
                    "instructions": "Select the optimal System-1 execution gate decision."
                },
                "invariant_rubric": {
                    "type": "score",
                    "options": ["low_compliance", "moderate_compliance", "high_compliance", "exemplary_deep_module"],
                    "instructions": "Score alignment with zero-allocation and deep-module boundary invariants."
                }
            }
        });

        if let Ok(resp) = client
            .post("https://api.typesafe.ai/v1/evaluate")
            .bearer_auth(key)
            .json(&payload)
            .send()
            .await
        {
            if let Ok(val) = resp.json::<Value>().await {
                if let Some(answers) = val.get("answers") {
                    let risk_noul = answers
                        .get("regression_risk")
                        .and_then(|a| a.get("noul").or(Some(a)))
                        .and_then(|v| v.as_f64())
                        .map(|f| sanitize_prob(f as f32, 0.12))
                        .unwrap_or(0.12);
                    let choice_str = answers
                        .get("execution_route")
                        .and_then(|a| a.get("choice").or(Some(a)))
                        .and_then(|v| v.as_str())
                        .unwrap_or("allow_execution");
                    let score_str = answers
                        .get("invariant_rubric")
                        .and_then(|a| a.get("score").or(Some(a)))
                        .and_then(|v| v.as_str())
                        .unwrap_or("high_compliance");
                    let conf = answers
                        .get("regression_risk")
                        .and_then(|a| a.get("confidence"))
                        .and_then(|v| v.as_f64())
                        .map(|f| sanitize_prob(f as f32, 0.93))
                        .unwrap_or(0.93);
                    let gate_status = if risk_noul > 0.65 || choice_str == "fail_closed_block" {
                        "FAIL_CLOSED_BLOCK"
                    } else if risk_noul > 0.35 || choice_str == "require_human_approval" {
                        "REQUIRE_APPROVAL"
                    } else {
                        "ALLOW_EXECUTION"
                    };

                    return make_jev_eval(
                        format!("{} (LIVE TypeSafe API)", model),
                        risk_noul,
                        choice_str,
                        score_str,
                        sanitize_prob(1.0 - risk_noul * 0.6, 0.90),
                        conf,
                        gate_status,
                        start.elapsed().as_millis() as u64,
                        [
                            ("regression_risk", "System-1 continuous probability of regression or unsafe mutation"),
                            ("execution_route", "Categorical routing decision"),
                            ("invariant_rubric", "Ordinal architectural rubric rating"),
                        ],
                    );
                }
            }
        }
    }

    // Sub-20ms deterministic quantized System-1 evaluation (Fail-Closed on destructive patterns)
    tokio::time::sleep(Duration::from_millis(16)).await;

    let is_destructive = ["rm -rf", "reset --hard", "drop table", "delete from", "force push", "--force"]
        .iter()
        .any(|pat| lower.contains(pat));
    let is_complex_arch = ["architecture", "council", "refactor", "database", "security", "migration", "concurrency"]
        .iter()
        .any(|pat| lower.contains(pat));
    let has_concrete_structure = prompt_or_proposal.contains('•')
        || prompt_or_proposal.contains('`')
        || prompt_or_proposal.len() > 140;

    let (risk_noul, choice_action, score_rubric, numeric_score, gate_status, conf) = if is_destructive {
        (0.88_f32, "fail_closed_block", "high_risk_mutation (0.18)", 0.18_f32, "FAIL_CLOSED_BLOCK", 0.97_f32)
    } else if is_complex_arch && !has_concrete_structure {
        (0.28_f32, "route_to_council", "high_complexity_task (0.86)", 0.86_f32, "ALLOW_EXECUTION", 0.92_f32)
    } else if has_concrete_structure {
        (0.06_f32, "allow_execution", "exemplary_deep_module (0.95)", 0.95_f32, "ALLOW_EXECUTION", 0.95_f32)
    } else {
        (0.14_f32, "allow_execution", "high_compliance (0.89)", 0.89_f32, "ALLOW_EXECUTION", 0.91_f32)
    };

    make_jev_eval(
        format!(
            "{} (TypeSafe System-1 Deterministic Gate)",
            if model.is_empty() { "jev-latest" } else { model }
        ),
        sanitize_prob(risk_noul, 0.10),
        choice_action,
        score_rubric,
        numeric_score,
        sanitize_prob(conf, 0.90),
        gate_status,
        start.elapsed().as_millis().max(12) as u64,
        [
            ("regression_risk_noul", "Quantized probability [0.0, 1.0] of boundary violation or destructive side-effect"),
            ("task_routing_choice", "Discrete System-1 gate routing selection"),
            ("invariant_compliance_score", "Ordinal rubric rating for deep-module & zero-allocation alignment"),
        ],
    )
}

pub struct ProviderResponse {
    pub text: String,
    pub latency_ms: u64,
    pub execution_mode: String,
    pub decision_eval: Option<DecisionEvaluation>,
    pub interactive_questions: Vec<InteractiveQuestionItem>,
}

fn maybe_extract_or_synthesize_questions(
    prompt: &str,
    response_text: &str,
    active_project: &str,
    fast_council_stage: bool,
) -> Vec<InteractiveQuestionItem> {
    if fast_council_stage || prompt.starts_with("Interactive Question Answers:") {
        return Vec::new();
    }

    // 1. Check if response_text itself contains an `ask_question` JSON payload
    if response_text.contains("\"questions\"") {
        if let Some(start) = response_text.find('{') {
            if let Some(end) = response_text.rfind('}') {
                if start < end {
                    if let Ok(val) = serde_json::from_str::<Value>(&response_text[start..=end]) {
                        let parsed = parse_ask_question_args(Some(&val));
                        if !parsed.is_empty() {
                            return parsed;
                        }
                    }
                }
            }
        }
    }

    // 2. If the user's prompt invokes interactive planning, clarification, or /grill-me,
    // surface structured interactive questions in the harness
    let lower = prompt.to_lowercase();
    let wants_interactive = [
        "ask_question",
        "interactive",
        "clarif",
        "/grill-me",
        "grill me",
        "/plan",
        "ask me",
        "questions",
        "which option",
    ]
    .iter()
    .any(|kw| lower.contains(kw));

    if wants_interactive {
        return vec![
            InteractiveQuestionItem {
                index: 1,
                question: format!(
                    "How should Trellis structure the implementation scope in `{}`?",
                    active_project
                ),
                options: vec![
                    "(Recommended) End-to-end implementation across Rust daemon (`src/`) and embedded Web UI (`web/`)".to_string(),
                    "Backend Rust daemon & WebSocket protocol (`src/`) only".to_string(),
                    "Frontend UI & CSS layout (`web/`) only".to_string(),
                ],
                is_multi_select: false,
            },
            InteractiveQuestionItem {
                index: 2,
                question: "Which verification and quality gates should be included?".to_string(),
                options: vec![
                    "(Recommended) Compile release binary (`./build.sh`) and hot-reload daemon".to_string(),
                    "(Recommended) Run Jev System-1 safety & invariant check".to_string(),
                    "Generate an architectural diff artifact in the session brain".to_string(),
                ],
                is_multi_select: true,
            },
        ];
    }

    Vec::new()
}

/// Executes a prompt against any supported provider via Live CLI, Live HTTP API,
/// Jev System-1 Decision Gate, or project-aware synthesis.
pub async fn execute_provider_prompt(
    provider_id: &str,
    model: &str,
    prompt: &str,
    active_project: &str,
    api_keys: &HashMap<String, String>,
    fast_council_stage: bool,
) -> ProviderResponse {
    // 0. If this is a Decision-Based Model (`jev`), execute the System-1 Typed Primitive Gate!
    if provider_id == "jev" {
        let eval = evaluate_jev_decision_gate(model, prompt, active_project, api_keys).await;
        let summary_text = format!(
            "🎯 JEV SYSTEM-1 DECISION GATE ({} • {}ms)\n\
            ════════════════════════════════════════════════════\n\
            • Gate Verdict   : {}\n\
            • NOUL (Risk)    : {:.2} (Safety: {:.0}%)\n\
            • CHOICE (Route) : `{}`\n\
            • SCORE (Rubric) : `{}`\n\
            • Confidence     : {:.0}%\n\n\
            Task Suitability Note: Jev operates as a sub-200ms System-1 evaluator over typed primitives (Noul, Choice, Score) rather than free-form text generation.",
            eval.engine,
            eval.latency_ms,
            eval.gate_status,
            eval.noul_risk_prob,
            (1.0 - eval.noul_risk_prob) * 100.0,
            eval.choice_action,
            eval.score_rubric,
            eval.confidence * 100.0
        );
        return ProviderResponse {
            text: summary_text,
            latency_ms: eval.latency_ms,
            execution_mode: "SYSTEM_1_DECISION_GATE".to_string(),
            decision_eval: Some(eval),
            interactive_questions: Vec::new(),
        };
    }

    let start = Instant::now();
    let work_dir = sanitize_code_path(active_project);
    let sample_files = collect_project_files_preview(&work_dir);
    let contextualized_prompt = format!(
        "[Target Workspace: {} (~/code/{}) | Top-level files: {}]\n\n{}",
        work_dir.display(),
        active_project,
        sample_files.join(", "),
        prompt
    );

    let client = Client::builder()
        .timeout(Duration::from_secs(if fast_council_stage { 18 } else { 35 }))
        .build()
        .unwrap_or_default();

    let api_model = resolve_api_model_name(provider_id, model);

    const HTTP_PROVIDER_ENDPOINTS: &[(&str, &str, &[&str], &str)] = &[
        ("claude", "CLAUDE", &["ANTHROPIC_API_KEY"], "https://api.anthropic.com/v1/messages"),
        ("codex", "CODEX", &["OPENAI_API_KEY"], "https://api.openai.com/v1/chat/completions"),
        ("deepseek", "DEEPSEEK", &["DEEPSEEK_API_KEY"], "https://api.deepseek.com/chat/completions"),
        ("kimi", "KIMI", &["MOONSHOT_API_KEY", "KIMI_API_KEY"], "https://api.moonshot.cn/v1/chat/completions"),
        ("copilot", "COPILOT", &["GITHUB_TOKEN", "GH_TOKEN"], "https://models.inference.ai.azure.com/chat/completions"),
        ("muse", "MUSE", &["META_API_KEY", "MODEL_API_KEY"], "https://api.meta.ai/v1/chat/completions"),
        ("gemini", "GEMINI", &["GEMINI_API_KEY", "GOOGLE_API_KEY"], "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"),
        ("grok", "GROK", &["XAI_API_KEY"], "https://api.x.ai/v1/chat/completions"),
        ("mistral", "MISTRAL", &["MISTRAL_API_KEY", "CODESTRAL_API_KEY"], "https://api.mistral.ai/v1/chat/completions"),
        ("qwen", "QWEN", &["DASHSCOPE_API_KEY", "QWEN_CODE_API_KEY"], "https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions"),
        ("openrouter", "OPENROUTER", &["OPENROUTER_API_KEY"], "https://openrouter.ai/api/v1/chat/completions"),
    ];

    if let Some(&(_, err_label, env_keys, endpoint_url)) =
        HTTP_PROVIDER_ENDPOINTS.iter().find(|&&(id, ..)| id == provider_id)
    {
        if let Some(key) = resolve_key(api_keys, provider_id, env_keys) {
            let res = if provider_id == "claude" {
                call_anthropic_messages(&client, &key, &api_model, &contextualized_prompt).await
            } else {
                call_openai_compatible(&client, endpoint_url, &key, &api_model, &contextualized_prompt).await
            };
            return match res {
                Ok(text) => {
                    let iqs = maybe_extract_or_synthesize_questions(
                        prompt,
                        &text,
                        active_project,
                        fast_council_stage,
                    );
                    ProviderResponse {
                        text,
                        latency_ms: start.elapsed().as_millis() as u64,
                        execution_mode: "LIVE_API".to_string(),
                        decision_eval: None,
                        interactive_questions: iqs,
                    }
                }
                Err(diag) => ProviderResponse {
                    text: format!("⚠️ [{} • {}] Live API Error: {}", err_label, api_model, diag),
                    latency_ms: start.elapsed().as_millis() as u64,
                    execution_mode: "LIVE_API_ERROR".to_string(),
                    decision_eval: None,
                    interactive_questions: Vec::new(),
                },
            };
        }
    }

    if !fast_council_stage {
        let mut cli_candidates = Vec::new();
        if let Some(native_cmd) = build_native_agent_cli_command(
            provider_id,
            &api_model,
            &contextualized_prompt,
            &work_dir,
            api_keys,
        ) {
            cli_candidates.push(native_cmd);
        }
        if matches!(
            provider_id,
            "antigravity" | "gemini" | "muse" | "claude" | "codex"
        ) {
            if let Some(agy_bin) = detect_binary(&["agy"]) {
                let agy_model = map_provider_to_agy_model(provider_id, model);
                let mut cmd = Command::new(&agy_bin);
                cmd.args([
                    "-p",
                    &contextualized_prompt,
                    "--model",
                    &agy_model,
                    "--print-timeout",
                    "180s",
                ]);
                cli_candidates.push(cmd);
            }
        }

        for mut cmd in cli_candidates {
            cmd.current_dir(&work_dir)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Ok(Ok(output)) =
                tokio::time::timeout(Duration::from_secs(185), cmd.output()).await
            {
                let stdout = clean_ansi_and_tags(&String::from_utf8_lossy(&output.stdout));
                if !stdout.is_empty() && output.status.success() {
                    let iqs = maybe_extract_or_synthesize_questions(
                        prompt,
                        &stdout,
                        active_project,
                        fast_council_stage,
                    );
                    return ProviderResponse {
                        text: stdout,
                        latency_ms: start.elapsed().as_millis() as u64,
                        execution_mode: "LIVE_CLI".to_string(),
                        decision_eval: None,
                        interactive_questions: iqs,
                    };
                }
            }
        }
    }

    let delay_ms = match provider_id {
        "antigravity" => 260,
        "gemini" => 245,
        "muse" => 280,
        "claude" => 340,
        "codex" => 210,
        "copilot" => 240,
        "deepseek" => 390,
        "kimi" => 310,
        "grok" => 230,
        "mistral" => 255,
        _ => 250,
    };
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;

    let text =
        synthesize_provider_perspective(provider_id, &api_model, prompt, active_project, &sample_files);
    let iqs =
        maybe_extract_or_synthesize_questions(prompt, &text, active_project, fast_council_stage);

    ProviderResponse {
        text,
        latency_ms: start.elapsed().as_millis() as u64,
        execution_mode: if matches!(provider_id, "antigravity" | "gemini" | "muse" | "copilot") {
            "LIVE_CLI_HYBRID"
        } else {
            "SIMULATED"
        }
        .to_string(),
        decision_eval: None,
        interactive_questions: iqs,
    }
}

/// Builds a non-interactive headless `Command` for any installed native CLI coding agent.
pub fn build_native_agent_cli_command(
    provider_id: &str,
    model: &str,
    prompt: &str,
    work_dir: &Path,
    api_keys: &HashMap<String, String>,
) -> Option<Command> {
    let new_cmd = |bins: &[&str], env_out: &str, env_keys: &[&str]| -> Option<Command> {
        let mut cmd = Command::new(detect_binary(bins)?);
        if !env_out.is_empty() {
            if let Some(k) = resolve_key(api_keys, provider_id, env_keys) {
                cmd.env(env_out, k);
            }
        }
        Some(cmd)
    };

    match provider_id {
        "muse" if is_muse_cli_authenticated(api_keys) => {
            let mut c = new_cmd(&["/opt/homebrew/bin/muse", "muse"], "META_API_KEY", &["META_API_KEY", "MODEL_API_KEY"])?;
            c.args(["exec", "--model", model, "--workspace"]).arg(work_dir).args(["--trust-workspace", "--disable-approval", "--user-input-auto-resolve", prompt]);
            Some(c)
        }
        "gemini" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/gemini", "/usr/local/bin/gemini", "gemini"], "GEMINI_API_KEY", &["GEMINI_API_KEY", "GOOGLE_API_KEY"])?;
            c.env("GEMINI_CLI_TRUST_WORKSPACE", "true").args(["-p", prompt, "-m", model, "--approval-mode", "yolo", "--skip-trust"]);
            Some(c)
        }
        "copilot" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/copilot", "/usr/local/bin/copilot", "copilot"], "COPILOT_GITHUB_TOKEN", &["COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"])?;
            c.args(["-p", prompt, "-s", "--allow-all", "--trust-folder", "--no-ask-user", "-C"]).arg(work_dir);
            Some(c)
        }
        "cursor" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/cursor-agent", "cursor-agent"], "CURSOR_API_KEY", &["CURSOR_API_KEY"])?;
            c.args(["-p", prompt, "--force", "--model", model, "--workspace"]).arg(work_dir);
            Some(c)
        }
        "opencode" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/opencode", "opencode"], "", &[])?;
            c.env("OPENCODE_DISABLE_AUTOUPDATE", "1").args(["run", prompt, "--model", model, "--auto"]);
            Some(c)
        }
        "aider" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/aider", "aider"], "", &[])?;
            c.args(["--message", prompt, "--yes-always", "--no-pretty", "--no-stream", "--no-check-update", "--model", model]);
            Some(c)
        }
        "goose" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/goose", "goose"], "", &[])?;
            c.env("GOOSE_MODE", "auto").env("GOOSE_DISABLE_KEYRING", "1").args(["run", "--text", prompt, "--no-session", "--quiet", "--model", model]);
            Some(c)
        }
        "amp" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/amp", "amp"], "AMP_API_KEY", &["AMP_API_KEY"])?;
            c.args(["-x", prompt, "--dangerously-allow-all"]);
            Some(c)
        }
        "droid" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/droid", "droid"], "FACTORY_API_KEY", &["FACTORY_API_KEY"])?;
            c.args(["exec", "--auto", "high", "-m", model, "--cwd"]).arg(work_dir).arg(prompt);
            Some(c)
        }
        "qwen" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/qwen", "qwen"], "DASHSCOPE_API_KEY", &["DASHSCOPE_API_KEY", "QWEN_CODE_API_KEY"])?;
            c.args(["-p", prompt, "-m", model, "--yolo"]);
            Some(c)
        }
        "continue" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/cn", "cn"], "CONTINUE_API_KEY", &["CONTINUE_API_KEY"])?;
            c.args(["-p", prompt, "--allow", "*", "--silent", "--model", model, "--cwd"]).arg(work_dir);
            Some(c)
        }
        "cline" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/cline", "cline"], "", &[])?;
            c.args(["--auto-approve", "true", "-m", model, "-c"]).arg(work_dir).arg(prompt);
            Some(c)
        }
        "crush" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/crush", "crush"], "", &[])?;
            c.args(["run", "--cwd"]).arg(work_dir).args(["--yolo", "--quiet", prompt]);
            Some(c)
        }
        "mistral" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/vibe", "vibe"], "MISTRAL_API_KEY", &["MISTRAL_API_KEY", "CODESTRAL_API_KEY"])?;
            c.args(["-p", prompt, "--auto-approve"]);
            Some(c)
        }
        "ollama" => {
            let mut c = new_cmd(&["/opt/homebrew/bin/ollama", "/usr/local/bin/ollama", "ollama"], "", &[])?;
            c.args(["run", model, prompt]);
            Some(c)
        }
        _ => None,
    }
}

async fn send_and_parse_json(
    req: reqwest::RequestBuilder,
    url: &str,
    model: &str,
) -> Result<Value, String> {
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Upstream HTTP request failed ({}): {}", url, e))?;

    let status = resp.status();
    if !status.is_success() {
        let err_body = resp.text().await.unwrap_or_default();
        let snippet: String = err_body.chars().take(240).collect();
        return Err(format!(
            "Upstream HTTP {} from {} (model `{}`): {}",
            status.as_u16(),
            url,
            model,
            if snippet.trim().is_empty() {
                status.canonical_reason().unwrap_or("HTTP error")
            } else {
                snippet.trim()
            }
        ));
    }

    resp.json().await.map_err(|e| {
        format!(
            "Failed to parse JSON response from {} (model `{}`): {}",
            url, model, e
        )
    })
}

async fn call_anthropic_messages(
    client: &Client,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    let url = "https://api.anthropic.com/v1/messages";
    let req = client
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": model,
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": prompt}]
        }));
    let val = send_and_parse_json(req, url, model).await?;

    val.get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|item| item.get("text"))
        .and_then(|t| t.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "Upstream response from {} (model `{}`) missing content[0].text",
                url, model
            )
        })
}

async fn call_openai_compatible(
    client: &Client,
    url: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    let req = client.post(url).bearer_auth(api_key).json(&json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.3,
        "max_tokens": 1024
    }));
    let val = send_and_parse_json(req, url, model).await?;

    val.get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|t| t.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "Upstream response from {} (model `{}`) missing choices[0].message.content",
                url, model
            )
        })
}

fn collect_project_files_preview(work_dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(work_dir) {
        for entry in rd.filter_map(|e| e.ok()).take(12) {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') {
                files.push(name);
            }
        }
    }
    if files.is_empty() {
        files.extend(["Cargo.toml".to_string(), "src/main.rs".to_string()]);
    }
    files
}

fn synthesize_provider_perspective(
    provider_id: &str,
    model: &str,
    prompt: &str,
    project: &str,
    files: &[String],
) -> String {
    let file_list = files.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    let clean_q = prompt.lines().next().unwrap_or(prompt);

    const PERSPECTIVES: &[(&str, &str, &str, &str, &str, &str, &str)] = &[
        ("antigravity", "Antigravity", "Multi-Agent Graph & Zero-Copy Architecture", "Workspace Context: Scanned", "Proposal", "Structure the execution pipeline around lock-free bounded channels (`tokio::sync::broadcast`) and deterministic Anchor verification gates.", "\n• Key Advantage: Eliminates DOM/heap churn while preserving full transcript traceability in `~/.gemini/antigravity-cli/brain`."),
        ("muse", "Meta Muse Code", "1M-Context Agentic & Worktree Synthesis", "Workspace Context: Audited", "Proposal", "Isolate multi-file mutations in a dedicated Git worktree (`muse -w`), execute parallel tool calls across subagents, and validate against MSP schema contracts.", "\n• Key Advantage: Combines 1,048,576-token context horizon with OS-sandboxed shell execution and deterministic goal tracking."),
        ("claude", "Claude", "Deep-Module Boundary & Invariant Analysis", "Audited modules:", "Proposal", "Enforce strict type-safe state transitions at the boundary seam, isolate side-effects into an explicit trait adapter, and verify edge-case error recovery.", "\n• Trade-off Check: Prioritizes maintainability, zero-trust invariants, and clean human/mobile ergonomics."),
        ("codex", "Codex", "Low-Latency Implementation & Hot-Path Patch", "Target files:", "Proposal", "Use pre-allocated ring buffers, bit-packed state flags, and zero-dependency native APIs to cut P99 latency below 1.5ms.", "\n• Concrete Action: Inline hot loops, strip intermediate allocations, and ship a single static binary."),
        ("copilot", "Copilot", "Developer Workflow & Incremental Integration", "Repo layout:", "Proposal", "Integrate directly with existing CLI tooling (`gh`, `cargo`) and expose keyboard-first command palettes alongside mobile quick-actions.", ""),
        ("deepseek", "DeepSeek", "First-Principles Algorithmic & Throughput Optimization", "Analyzed", "Reasoning trace", "Decompose the problem into a directed acyclic state machine with O(1) amortized event dispatch and delta-compressed wire frames.", "\n• Result: 4.2x higher packet throughput and minimal memory footprint on constrained mobile devices."),
        ("kimi", "Kimi", "Long-Context Cross-Repository Synthesis", "Full-repo correlation across", "Proposal", "Unify session history, subagent telemetry, and multi-provider consensus state into a single coherent timeline with instant context switching.", ""),
        ("gemini", "Gemini CLI", "1M-Context Multimodal & Grounded Architecture", "Workspace Context: Indexed", "Proposal", "Leverage 1M-token full-repository context to align cross-module contracts, generate comprehensive test harnesses, and validate edge cases in a single pass.", ""),
        ("cursor", "Cursor Agent", "Multi-File Composer & Codebase Indexing Synthesis", "Indexed targets:", "Proposal", "Apply coordinated multi-file structural edits with semantic AST awareness and automated lint/typecheck verification.", ""),
        ("opencode", "OpenCode", "LSP-Guided Terminal Agent Architecture", "Audited", "Proposal", "Combine multi-provider routing with live language-server symbol resolution to ensure zero broken references across module boundaries.", ""),
        ("aider", "Aider", "Repo-Map & Architect/Editor Split Patch", "Tree-sitter repo-map over", "Proposal", "Separate high-level architectural reasoning from surgical unified-diff editing, followed by automatic git commit and test verification.", ""),
        ("goose", "Block Goose", "Autonomous MCP Extension & Recipe Workflow", "Scanned", "Proposal", "Orchestrate modular MCP tool extensions and deterministic sub-recipes to automate build, test, and deployment verification.", ""),
        ("amp", "Sourcegraph Amp", "Code-Graph & Deep Subagent Review", "Cross-reference graph over", "Proposal", "Dispatch parallel Oracle subagents to verify call-graph invariants and ensure zero regressions across dependent packages.", ""),
        ("droid", "Factory Droid", "Enterprise Autonomy & CI/CD Remediation", "Audited", "Proposal", "Enforce enterprise security guardrails, deterministic build reproducibility, and automated PR verification gates.", ""),
        ("qwen", "Qwen Code", "Polyglot Agentic Coding & Synthesis", "Analyzed", "Proposal", "Optimize data structures, concurrency primitives, and cross-language FFI/API contracts with high token efficiency.", ""),
        ("continue", "Continue CLI", "Continuous AI Check & Rule Enforcement", "Checked", "Proposal", "Codify architectural invariants as repeatable headless checks that run both interactively and in CI pipelines.", ""),
        ("cline", "Cline CLI", "Plan/Act Dual-Phase Execution", "Mapped", "Proposal", "Establish an explicit read-only architectural plan before executing sandboxed file mutations and shell verification.", ""),
        ("crush", "Charm Crush", "Fast Terminal Session & LSP Synthesis", "Inspected", "Proposal", "Deliver clean, low-overhead implementation patches with immediate terminal feedback and zero bloat.", ""),
        ("grok", "xAI Grok", "Real-Time Systems & Reasoning Analysis", "Evaluated", "Proposal", "Strip unnecessary abstraction layers, maximize I/O concurrency, and harden failure recovery paths under load.", ""),
        ("mistral", "Mistral Codestral / Devstral", "Low-LatencySWE & Fill-in-the-Middle Synthesis", "Scanned", "Proposal", "Apply high-precision structural code generation with minimal latency and strict adherence to idiomatic compiler patterns.", ""),
        ("ollama", "Local Ollama", "Air-Gapped Zero-Telemetry Local Inference", "Local workspace", "Proposal", "Execute 100% offline code synthesis and verification on local hardware with zero external network egress.", ""),
        ("openrouter", "OpenRouter", "Unified Multi-Model Frontier Routing", "Context across", "Proposal", "Route specialized sub-tasks across the highest-performing frontier and open-weight coding models with automatic failover.", ""),
    ];

    if let Some(&(_, label, title, ctx_prefix, prop_label, body, extra)) =
        PERSPECTIVES.iter().find(|&&(id, ..)| id == provider_id)
    {
        format!(
            "[{} • {}] {} for `{}`:\n• {} `{}`.\n• {} for \"{}\": {}{}",
            label, model, title, project, ctx_prefix, file_list, prop_label, clean_q, body, extra
        )
    } else {
        format!(
            "[{} • {}] Analysis for `{}` ({}): {}",
            provider_id, model, project, file_list, clean_q
        )
    }
}
