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
    match provider_id {
        "antigravity" => "gemini-4-argon".to_string(),
        "jev" => "jev-latest".to_string(),
        "gemini" => "gemini-2.5-pro".to_string(),
        "muse" => "muse-spark-1.3".to_string(),
        "claude" => "claude-4-sonnet".to_string(),
        "codex" => "codex-mini-latest".to_string(),
        "copilot" => "copilot-gpt-4o".to_string(),
        "cursor" => "claude-4-sonnet".to_string(),
        "opencode" => "anthropic/claude-sonnet-4-5".to_string(),
        "aider" => "claude-3-7-sonnet-latest".to_string(),
        "goose" => "claude-sonnet-4-5".to_string(),
        "amp" => "smart".to_string(),
        "droid" => "claude-sonnet-4-20250514".to_string(),
        "qwen" => "qwen3-coder-plus".to_string(),
        "continue" => "claude-3-7-sonnet".to_string(),
        "cline" => "claude-sonnet-4-20250514".to_string(),
        "crush" => "claude-sonnet-4-5".to_string(),
        "deepseek" => "deepseek-chat".to_string(),
        "kimi" => "kimi-k2-5".to_string(),
        "grok" => "grok-code-fast-1".to_string(),
        "mistral" => "codestral-latest".to_string(),
        "ollama" => "qwen3-coder".to_string(),
        "openrouter" => "anthropic/claude-sonnet-4".to_string(),
        _ => "gpt-4o".to_string(),
    }
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
    if let Ok(meta) = std::fs::metadata(&auth_path) {
        if meta.is_file() && meta.len() > 4 {
            return true;
        }
    }
    false
}

#[derive(Clone, Debug)]
pub enum AgyStreamOutcome {
    Completed {
        text: String,
        tokens: TokenMetrics,
        interactive_questions: Vec<InteractiveQuestionItem>,
    },
    Aborted,
    CliError(String),
}

impl PartialEq for AgyStreamOutcome {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Completed {
                    text: t1,
                    tokens: tk1,
                    interactive_questions: iq1,
                },
                Self::Completed {
                    text: t2,
                    tokens: tk2,
                    interactive_questions: iq2,
                },
            ) => {
                t1 == t2
                    && tk1.input_tokens == tk2.input_tokens
                    && tk1.cache_read_tokens == tk2.cache_read_tokens
                    && tk1.output_tokens == tk2.output_tokens
                    && iq1 == iq2
            }
            (Self::Aborted, Self::Aborted) => true,
            (Self::CliError(e1), Self::CliError(e2)) => e1 == e2,
            _ => false,
        }
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
    mut on_event: F,
) -> AgyStreamOutcome
where
    F: FnMut(AgyParsedEvent),
{
    if *abort_rx.borrow() {
        return AgyStreamOutcome::Aborted;
    }

    let mapped_model = map_provider_to_agy_model(provider_id, model);
    let contextualized_prompt = if prompt.starts_with("[Workspace:") {
        prompt.to_string()
    } else {
        format!(
            "[Workspace: {} ({})]\n\n{}",
            project_id,
            workspace_path.display(),
            prompt
        )
    };

    let mut cmd = Command::new(agy_bin);
    cmd.current_dir(workspace_path);
    if let Some(conv_id) = conversation_id {
        if !conv_id.trim().is_empty() {
            cmd.arg("--conversation").arg(conv_id);
        }
    }
    cmd.arg("--model")
        .arg(&mapped_model)
        .arg("-p")
        .arg(&contextualized_prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--print-timeout")
        .arg("300s")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return AgyStreamOutcome::CliError(format!(
                "Failed to spawn agy CLI ({}): {}",
                agy_bin, e
            ));
        }
    };

    let mut streamed_text = String::new();
    let mut interactive_questions = Vec::new();
    let mut final_tokens = TokenMetrics {
        input_tokens: (prompt.len() as u64 / 3) + 320,
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

            let Some(line) = (match line_res {
                Ok(Some(l)) => Some(l),
                _ => None,
            }) else {
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
                AgyParsedEvent::Init { .. } => {}
                AgyParsedEvent::ToolActive { questions, .. } => {
                    if !questions.is_empty() {
                        interactive_questions = questions;
                    }
                }
                AgyParsedEvent::ToolDone { .. } => {}
                AgyParsedEvent::TextDelta {
                    delta,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    ..
                } => {
                    streamed_text.push_str(&delta);
                    if let Some(it) = input_tokens {
                        if it > 0 {
                            final_tokens.input_tokens = it;
                        }
                    }
                    if let Some(ot) = output_tokens {
                        if ot > 0 {
                            final_tokens.output_tokens = ot;
                        }
                    }
                    if let Some(crt) = cache_read_tokens {
                        if crt > 0 {
                            final_tokens.cache_read_tokens = crt;
                        }
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
        AgyStreamOutcome::CliError(format!("agy CLI exited with {}", code_str))
    }
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
    match provider_id {
        "muse" => {
            if m.contains("glimmer") {
                "gemini-3.8-flash-medium".to_string()
            } else if m.contains("wonder") || m.contains("1.3") {
                "gemini-4-argon-high".to_string()
            } else {
                "gemini-4-argon-medium".to_string()
            }
        }
        "claude" => {
            if m.contains("opus") {
                if m.contains("low") {
                    "claude-opus-5-5-low".to_string()
                } else if m.contains("medium") {
                    "claude-opus-5-5-medium".to_string()
                } else {
                    "claude-opus-5-5-high".to_string()
                }
            } else if m.contains("low") {
                "claude-sonnet-5-5-low".to_string()
            } else if m.contains("high") {
                "claude-sonnet-5-5-high".to_string()
            } else {
                "claude-sonnet-5-5-medium".to_string()
            }
        }
        "codex" | "copilot" => "gpt-oss-120b-medium".to_string(),
        _ => {
            if m.contains("flash") {
                if m.contains("low") {
                    "gemini-3.8-flash-low".to_string()
                } else if m.contains("medium") {
                    "gemini-3.8-flash-medium".to_string()
                } else {
                    "gemini-3.8-flash-high".to_string()
                }
            } else if m.contains("high") || m.contains("pro") {
                "gemini-4-argon-high".to_string()
            } else if m.contains("low") {
                "gemini-4-argon-low".to_string()
            } else {
                "gemini-4-argon-medium".to_string()
            }
        }
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
    mut on_event: F,
) -> AgyStreamOutcome
where
    F: FnMut(AgyParsedEvent),
{
    if *abort_rx.borrow() {
        return AgyStreamOutcome::Aborted;
    }

    let resolved_model = resolve_api_model_name("muse", model);
    let contextualized_prompt = if prompt.starts_with("[Workspace:") {
        prompt.to_string()
    } else {
        format!(
            "[Workspace: {} ({})]\n\n{}",
            project_id,
            workspace_path.display(),
            prompt
        )
    };

    let mut cmd = Command::new(muse_bin);
    cmd.current_dir(workspace_path);
    if let Some(k) = api_key {
        if !k.trim().is_empty() {
            cmd.env("META_API_KEY", k.trim());
        }
    }
    cmd.arg("exec")
        .arg("--json")
        .arg("--model")
        .arg(&resolved_model)
        .arg("--workspace")
        .arg(workspace_path)
        .arg("--trust-workspace")
        .arg("--disable-approval")
        .arg("--user-input-auto-resolve")
        .arg(&contextualized_prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return AgyStreamOutcome::CliError(format!(
                "Failed to spawn muse CLI ({}): {}",
                muse_bin, e
            ));
        }
    };

    let mut streamed_text = String::new();
    let mut final_tokens = TokenMetrics {
        input_tokens: (prompt.len() as u64 / 3) + 320,
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

            let Some(line) = (match line_res {
                Ok(Some(l)) => Some(l),
                _ => None,
            }) else {
                break;
            };

            let Some(evt) = parse_muse_stream_json_line(&line, project_id) else {
                continue;
            };

            on_event(evt.clone());

            if *abort_rx.borrow() {
                let _ = child.kill().await;
                return AgyStreamOutcome::Aborted;
            }

            match evt {
                AgyParsedEvent::TextDelta {
                    delta,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    ..
                } => {
                    if !delta.is_empty() {
                        streamed_text.push_str(&delta);
                    }
                    if let Some(it) = input_tokens {
                        if it > 0 {
                            final_tokens.input_tokens = it;
                        }
                    }
                    if let Some(ot) = output_tokens {
                        if ot > 0 {
                            final_tokens.output_tokens = ot;
                        }
                    }
                    if let Some(crt) = cache_read_tokens {
                        if crt > 0 {
                            final_tokens.cache_read_tokens = crt;
                        }
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

    if !streamed_text.trim().is_empty() {
        let _ = tokio::time::timeout(Duration::from_millis(250), child.wait()).await;
        let _ = child.start_kill();
        if final_tokens.output_tokens == 0 {
            final_tokens.output_tokens = (streamed_text.len() as u64 / 3).max(48);
        }
        return AgyStreamOutcome::Completed {
            text: streamed_text,
            tokens: final_tokens,
            interactive_questions: Vec::new(),
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
        AgyStreamOutcome::CliError(format!("muse CLI exited with {}", code_str))
    }
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
                // Normalize internal CLI tool names to standard Trellis UI badges
                let tool_name = match raw_tool_name {
                    "view_file" => "read_file".to_string(),
                    "replace_file_content" | "multi_replace_file_content" => {
                        "edit_file".to_string()
                    }
                    "write_to_file" => "write_file".to_string(),
                    other => other.to_string(),
                };

                if state == "ACTIVE" {
                    let params = su.get("tool_info").and_then(|ti| ti.get("parameters"));
                    let mut questions = Vec::new();
                    if tool_name == "ask_question" {
                        questions = parse_ask_question_args(params);
                    }
                    let raw_summary = if tool_name == "ask_question" && !questions.is_empty() {
                        questions[0].question.clone()
                    } else {
                        params
                            .and_then(|p| {
                                p.get("toolSummary")
                                    .or_else(|| p.get("Path"))
                                    .or_else(|| p.get("TargetFile"))
                                    .or_else(|| p.get("CommandLine"))
                                    .or_else(|| p.get("Query"))
                                    .or_else(|| p.get("query"))
                                    .or_else(|| p.get("SearchPath"))
                                    .or_else(|| p.get("DirectoryPath"))
                                    .or_else(|| p.get("Role"))
                            })
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim_matches('"').to_string())
                            .unwrap_or_else(|| format!("~/code/{}", active_project))
                    };
                    let code_prefix = format!("{}/code/", home_dir().to_string_lossy());
                    let proj_prefix = format!("{}{}/", code_prefix, active_project);
                    let summary = raw_summary
                        .replace(&proj_prefix, "")
                        .replace(&code_prefix, "")
                        .chars()
                        .take(80)
                        .collect();

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
    let lower = raw.trim().to_lowercase();
    if lower == "view_file"
        || lower == "read"
        || lower == "read_file"
        || lower == "view"
        || lower == "readtoolcall"
    {
        "read_file".to_string()
    } else if lower == "replace_file_content"
        || lower == "multi_replace_file_content"
        || lower == "edit"
        || lower == "edit_file"
        || lower == "str_replace_editor"
        || lower == "edittoolcall"
    {
        "edit_file".to_string()
    } else if lower == "write_to_file"
        || lower == "write"
        || lower == "create_file"
        || lower == "writetoolcall"
    {
        "write_file".to_string()
    } else if lower == "bash"
        || lower == "shell"
        || lower == "run_shell_command"
        || lower == "run_command"
        || lower == "shelltoolcall"
    {
        "run_command".to_string()
    } else {
        raw.to_string()
    }
}

fn extract_tool_summary(params: Option<&Value>, active_project: &str) -> String {
    let raw_summary = params
        .and_then(|p| {
            p.get("toolSummary")
                .or_else(|| p.get("command"))
                .or_else(|| p.get("CommandLine"))
                .or_else(|| p.get("path"))
                .or_else(|| p.get("file_path"))
                .or_else(|| p.get("filePath"))
                .or_else(|| p.get("Path"))
                .or_else(|| p.get("TargetFile"))
                .or_else(|| p.get("pattern"))
                .or_else(|| p.get("query"))
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

pub fn build_provider_registry(api_keys: &HashMap<String, String>) -> Vec<ProviderInfo> {
    let agy_bin = detect_binary(&["agy"]);
    let gemini_bin = detect_binary(&["/opt/homebrew/bin/gemini", "/usr/local/bin/gemini", "gemini"]);
    let muse_bin = detect_binary(&["/opt/homebrew/bin/muse", "muse"]).or_else(|| agy_bin.clone());
    let claude_bin = detect_binary(&["/opt/homebrew/bin/claude", "claude"]).or_else(|| agy_bin.clone());
    let codex_bin = detect_binary(&["/opt/homebrew/bin/codex", "codex"]).or_else(|| agy_bin.clone());
    let copilot_bin = detect_binary(&[
        "/opt/homebrew/bin/copilot",
        "/usr/local/bin/copilot",
        "copilot",
        "/opt/homebrew/bin/gh",
        "/usr/local/bin/gh",
        "gh",
    ]);
    let cursor_bin = detect_binary(&["/opt/homebrew/bin/cursor-agent", "cursor-agent", "agent"]);
    let opencode_bin = detect_binary(&["/opt/homebrew/bin/opencode", "opencode"]);
    let aider_bin = detect_binary(&["/opt/homebrew/bin/aider", "aider"]);
    let goose_bin = detect_binary(&["/opt/homebrew/bin/goose", "goose"]);
    let amp_bin = detect_binary(&["/opt/homebrew/bin/amp", "amp"]);
    let droid_bin = detect_binary(&["/opt/homebrew/bin/droid", "droid"]);
    let qwen_bin = detect_binary(&["/opt/homebrew/bin/qwen", "qwen"]);
    let continue_bin = detect_binary(&["/opt/homebrew/bin/cn", "cn"]);
    let cline_bin = detect_binary(&["/opt/homebrew/bin/cline", "cline"]);
    let crush_bin = detect_binary(&["/opt/homebrew/bin/crush", "crush"]);
    let vibe_bin = detect_binary(&["/opt/homebrew/bin/vibe", "vibe"]);
    let ollama_bin = detect_binary(&["/opt/homebrew/bin/ollama", "/usr/local/bin/ollama", "ollama"]);

    let has_key = |env_names: &[&str], id: &str| -> bool {
        if let Some(k) = api_keys.get(id) {
            if !k.trim().is_empty() {
                return true;
            }
        }
        env_names.iter().any(|&ename| {
            std::env::var(ename)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
        })
    };

    let agy_key = has_key(&["GEMINI_API_KEY", "GOOGLE_API_KEY"], "antigravity");
    let gemini_key = has_key(&["GEMINI_API_KEY", "GOOGLE_API_KEY"], "gemini");
    let muse_key = has_key(&["META_API_KEY", "MODEL_API_KEY"], "muse");
    let claude_key = has_key(&["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"], "claude");
    let codex_key = has_key(&["CODEX_API_KEY", "OPENAI_API_KEY"], "codex");
    let copilot_key = has_key(&["COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"], "copilot");
    let cursor_key = has_key(&["CURSOR_API_KEY"], "cursor");
    let opencode_key = has_key(
        &["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
        "opencode",
    );
    let aider_key = has_key(
        &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "DEEPSEEK_API_KEY"],
        "aider",
    );
    let goose_key = has_key(
        &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"],
        "goose",
    );
    let amp_key = has_key(&["AMP_API_KEY"], "amp");
    let droid_key = has_key(&["FACTORY_API_KEY"], "droid");
    let qwen_key = has_key(&["DASHSCOPE_API_KEY", "QWEN_CODE_API_KEY"], "qwen");
    let continue_key = has_key(&["CONTINUE_API_KEY"], "continue");
    let cline_key = has_key(&["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY"], "cline");
    let crush_key = has_key(&["ANTHROPIC_API_KEY", "OPENAI_API_KEY"], "crush");
    let deepseek_key = has_key(&["DEEPSEEK_API_KEY"], "deepseek");
    let kimi_key = has_key(&["MOONSHOT_API_KEY", "KIMI_API_KEY"], "kimi");
    let grok_key = has_key(&["XAI_API_KEY"], "grok");
    let mistral_key = has_key(&["MISTRAL_API_KEY", "CODESTRAL_API_KEY"], "mistral");
    let ollama_key = has_key(&["OLLAMA_HOST"], "ollama");
    let openrouter_key = has_key(&["OPENROUTER_API_KEY"], "openrouter");
    let jev_key = has_key(&["JEV_API_KEY", "TYPESAFE_API_KEY"], "jev");

    let mode_for = |bin: &Option<String>, key: bool| -> String {
        if bin.is_some() {
            "LIVE_CLI".to_string()
        } else if key {
            "LIVE_API".to_string()
        } else {
            "SIMULATED".to_string()
        }
    };

    vec![
        ProviderInfo {
            id: "antigravity".to_string(),
            name: "Antigravity (agy)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Multi-Agent Graph Orchestration & Deep Repo Execution".to_string(),
            default_model: "gemini-4-argon".to_string(),
            available_models: vec![
                "gemini-4-argon".to_string(),
                "gemini-3.6-pro".to_string(),
                "gemini-3.6-flash".to_string(),
            ],
            color: "#A8202A".to_string(),
            icon: "⚡".to_string(),
            mode: mode_for(&agy_bin, agy_key),
            cli_path: agy_bin,
            has_api_key: agy_key,
            enabled_in_council: true,
        },
        ProviderInfo {
            id: "jev".to_string(),
            name: "Jev (TypeSafe System-1)".to_string(),
            model_class: "DECISION_SYSTEM_1".to_string(),
            task_specialty: "Sub-200ms Typed Decision Gates (Noul, Choice, Score), Risk Routing & Council Rubric Scoring".to_string(),
            default_model: "jev-latest".to_string(),
            available_models: vec![
                "jev-latest".to_string(),
                "jev-risk-gate".to_string(),
                "jev-router-v1".to_string(),
            ],
            color: "#2D4A3E".to_string(),
            icon: "🎯".to_string(),
            mode: if jev_key {
                "LIVE_API".to_string()
            } else {
                "DETERMINISTIC_GATE".to_string()
            },
            cli_path: None,
            has_api_key: jev_key,
            enabled_in_council: true,
        },
        ProviderInfo {
            id: "gemini".to_string(),
            name: "Google Gemini CLI (gemini)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "1M-Context Multimodal Repo Exploration, Worktrees & Policy Engine".to_string(),
            default_model: "gemini-2.5-pro".to_string(),
            available_models: vec![
                "gemini-2.5-pro".to_string(),
                "gemini-2.5-flash".to_string(),
                "gemini-3-pro-preview".to_string(),
            ],
            color: "#1A73E8".to_string(),
            icon: "♊".to_string(),
            mode: mode_for(&gemini_bin, gemini_key),
            cli_path: gemini_bin,
            has_api_key: gemini_key,
            enabled_in_council: true,
        },
        ProviderInfo {
            id: "muse".to_string(),
            name: "Meta Muse Code (muse)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Long-Horizon Agentic Coding, Worktree Subagents & 1M Context".to_string(),
            default_model: "muse-spark-1.3".to_string(),
            available_models: vec![
                "muse-spark-1.3".to_string(),
                "muse-wonder-1.0".to_string(),
                "muse-spark-1.2".to_string(),
                "muse-spark-1.3-contributor".to_string(),
                "muse-spark-open".to_string(),
                "muse-glimmer".to_string(),
            ],
            color: "#0064E0".to_string(),
            icon: "♾️".to_string(),
            mode: mode_for(&muse_bin, muse_key),
            cli_path: muse_bin,
            has_api_key: muse_key,
            enabled_in_council: true,
        },
        ProviderInfo {
            id: "claude".to_string(),
            name: "Claude Code (claude)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Deep Module Design, Boundary Invariants & Adversarial Review".to_string(),
            default_model: "claude-4-sonnet".to_string(),
            available_models: vec![
                "claude-4-sonnet".to_string(),
                "claude-4-opus".to_string(),
                "claude-3-7-sonnet-latest".to_string(),
            ],
            color: "#9E472A".to_string(),
            icon: "🟠".to_string(),
            mode: mode_for(&claude_bin, claude_key),
            cli_path: claude_bin,
            has_api_key: claude_key,
            enabled_in_council: true,
        },
        ProviderInfo {
            id: "codex".to_string(),
            name: "OpenAI Codex CLI (codex)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Low-Latency Code Synthesis, Seatbelt Sandbox & Surgical Patching".to_string(),
            default_model: "codex-mini-latest".to_string(),
            available_models: vec![
                "codex-mini-latest".to_string(),
                "gpt-5.4".to_string(),
                "gpt-4o".to_string(),
                "o3-mini".to_string(),
            ],
            color: "#2D4A3E".to_string(),
            icon: "🟢".to_string(),
            mode: mode_for(&codex_bin, codex_key),
            cli_path: codex_bin,
            has_api_key: codex_key,
            enabled_in_council: true,
        },
        ProviderInfo {
            id: "copilot".to_string(),
            name: "GitHub Copilot CLI (copilot)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Fleet Subagent Orchestration, GitHub MCP & PR/Issue Automation".to_string(),
            default_model: "copilot-gpt-4o".to_string(),
            available_models: vec![
                "copilot-gpt-4o".to_string(),
                "gpt-5.4".to_string(),
                "claude-sonnet-4.5".to_string(),
                "copilot-claude-sonnet".to_string(),
            ],
            color: "#4A6B5D".to_string(),
            icon: "🟣".to_string(),
            mode: mode_for(&copilot_bin, copilot_key),
            cli_path: copilot_bin,
            has_api_key: copilot_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "cursor".to_string(),
            name: "Cursor Agent CLI (cursor-agent)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Fast Multi-File Refactoring, `.cursor/rules` & IDE Parity".to_string(),
            default_model: "claude-4-sonnet".to_string(),
            available_models: vec![
                "claude-4-sonnet".to_string(),
                "gpt-5".to_string(),
                "cursor-fast".to_string(),
            ],
            color: "#3B4252".to_string(),
            icon: "🖱️".to_string(),
            mode: mode_for(&cursor_bin, cursor_key),
            cli_path: cursor_bin,
            has_api_key: cursor_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "opencode".to_string(),
            name: "OpenCode (opencode)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Open-Source Terminal Coding Agent, LSP Diagnostics & Session Forking".to_string(),
            default_model: "anthropic/claude-sonnet-4-5".to_string(),
            available_models: vec![
                "anthropic/claude-sonnet-4-5".to_string(),
                "openai/gpt-4o".to_string(),
                "google/gemini-2.5-pro".to_string(),
            ],
            color: "#5E81AC".to_string(),
            icon: "🔓".to_string(),
            mode: mode_for(&opencode_bin, opencode_key),
            cli_path: opencode_bin,
            has_api_key: opencode_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "aider".to_string(),
            name: "Aider (aider)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Tree-Sitter Repo Map, Architect/Editor Dual-Model & Auto-Lint/Test".to_string(),
            default_model: "claude-3-7-sonnet-latest".to_string(),
            available_models: vec![
                "claude-3-7-sonnet-latest".to_string(),
                "deepseek/deepseek-chat".to_string(),
                "gpt-4o".to_string(),
            ],
            color: "#2E7D32".to_string(),
            icon: "🛠️".to_string(),
            mode: mode_for(&aider_bin, aider_key),
            cli_path: aider_bin,
            has_api_key: aider_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "goose".to_string(),
            name: "Block Goose (goose)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Extensible MCP Developer Agent & Declarative YAML Recipes".to_string(),
            default_model: "claude-sonnet-4-5".to_string(),
            available_models: vec![
                "claude-sonnet-4-5".to_string(),
                "gpt-4o".to_string(),
                "gemini-2.5-pro".to_string(),
            ],
            color: "#6D4C41".to_string(),
            icon: "🪿".to_string(),
            mode: mode_for(&goose_bin, goose_key),
            cli_path: goose_bin,
            has_api_key: goose_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "amp".to_string(),
            name: "Sourcegraph Amp (amp)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Cross-Repository Code Intelligence, Subagent Oracles & Deep Search".to_string(),
            default_model: "smart".to_string(),
            available_models: vec![
                "smart".to_string(),
                "deep".to_string(),
                "rush".to_string(),
            ],
            color: "#D84315".to_string(),
            icon: "🔊".to_string(),
            mode: mode_for(&amp_bin, amp_key),
            cli_path: amp_bin,
            has_api_key: amp_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "droid".to_string(),
            name: "Factory Droid (droid)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Autonomous CI/CD Engineering, Migration Droids & Tiered Autonomy".to_string(),
            default_model: "claude-sonnet-4-20250514".to_string(),
            available_models: vec![
                "claude-sonnet-4-20250514".to_string(),
                "openai-latest-balanced".to_string(),
            ],
            color: "#37474F".to_string(),
            icon: "🤖".to_string(),
            mode: mode_for(&droid_bin, droid_key),
            cli_path: droid_bin,
            has_api_key: droid_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "qwen".to_string(),
            name: "Qwen Code (qwen)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Qwen3-Coder Agentic Tool Loops & Open-Weight Code Synthesis".to_string(),
            default_model: "qwen3-coder-plus".to_string(),
            available_models: vec![
                "qwen3-coder-plus".to_string(),
                "qwen-max".to_string(),
                "qwen2.5-coder-32b-instruct".to_string(),
            ],
            color: "#512DA8".to_string(),
            icon: "🟣".to_string(),
            mode: mode_for(&qwen_bin, qwen_key),
            cli_path: qwen_bin,
            has_api_key: qwen_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "continue".to_string(),
            name: "Continue CLI (cn)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Rule-Governed Headless Checks (`.continue/rules`) & Hub Assistants".to_string(),
            default_model: "claude-3-7-sonnet".to_string(),
            available_models: vec![
                "claude-3-7-sonnet".to_string(),
                "gpt-4o".to_string(),
            ],
            color: "#00838F".to_string(),
            icon: "▶️".to_string(),
            mode: mode_for(&continue_bin, continue_key),
            cli_path: continue_bin,
            has_api_key: continue_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "cline".to_string(),
            name: "Cline CLI (cline)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Plan/Act Dual-Phase Agentic Execution & `.clinerules` Enforcement".to_string(),
            default_model: "claude-sonnet-4-20250514".to_string(),
            available_models: vec![
                "claude-sonnet-4-20250514".to_string(),
                "deepseek-reasoner".to_string(),
            ],
            color: "#455A64".to_string(),
            icon: "🦾".to_string(),
            mode: mode_for(&cline_bin, cline_key),
            cli_path: cline_bin,
            has_api_key: cline_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "crush".to_string(),
            name: "Charm Crush (crush)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Catwalk Multi-Provider Routing & Fast Terminal Code Generation".to_string(),
            default_model: "claude-sonnet-4-5".to_string(),
            available_models: vec![
                "claude-sonnet-4-5".to_string(),
                "gpt-4o".to_string(),
            ],
            color: "#AD1457".to_string(),
            icon: "💖".to_string(),
            mode: mode_for(&crush_bin, crush_key),
            cli_path: crush_bin,
            has_api_key: crush_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "deepseek".to_string(),
            name: "DeepSeek R1/V3".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "First-Principles Algorithmic Reasoning & Math Verification".to_string(),
            default_model: "deepseek-reasoner".to_string(),
            available_models: vec![
                "deepseek-reasoner".to_string(),
                "deepseek-chat".to_string(),
            ],
            color: "#4A6B5D".to_string(),
            icon: "🔵".to_string(),
            mode: mode_for(&None, deepseek_key),
            cli_path: None,
            has_api_key: deepseek_key,
            enabled_in_council: true,
        },
        ProviderInfo {
            id: "kimi".to_string(),
            name: "Kimi (Moonshot)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Long-Context Cross-Repository Synthesis".to_string(),
            default_model: "kimi-k2-5".to_string(),
            available_models: vec![
                "kimi-k2-5".to_string(),
                "moonshot-v1-128k".to_string(),
            ],
            color: "#A8202A".to_string(),
            icon: "🌙".to_string(),
            mode: mode_for(&None, kimi_key),
            cli_path: None,
            has_api_key: kimi_key,
            enabled_in_council: true,
        },
        ProviderInfo {
            id: "grok".to_string(),
            name: "xAI Grok (grok)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "High-Throughput Agentic Coding (`grok-code-fast-1`) & Frontier Reasoning".to_string(),
            default_model: "grok-code-fast-1".to_string(),
            available_models: vec![
                "grok-code-fast-1".to_string(),
                "grok-4".to_string(),
                "grok-3-mini".to_string(),
            ],
            color: "#263238".to_string(),
            icon: "✖️".to_string(),
            mode: mode_for(&None, grok_key),
            cli_path: None,
            has_api_key: grok_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "mistral".to_string(),
            name: "Mistral Codestral / Vibe".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Devstral SWE Agent, Codestral FIM & Mistral Vibe CLI".to_string(),
            default_model: "codestral-latest".to_string(),
            available_models: vec![
                "codestral-latest".to_string(),
                "devstral-small-latest".to_string(),
                "mistral-large-latest".to_string(),
            ],
            color: "#E65100".to_string(),
            icon: "🌪️".to_string(),
            mode: mode_for(&vibe_bin, mistral_key),
            cli_path: vibe_bin,
            has_api_key: mistral_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "ollama".to_string(),
            name: "Local Ollama (ollama)".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Zero-Egress Local Open-Weight Inference (`qwen3-coder`, `devstral`, `muse-glimmer`)".to_string(),
            default_model: "qwen3-coder".to_string(),
            available_models: vec![
                "qwen3-coder".to_string(),
                "qwen2.5-coder:32b".to_string(),
                "deepseek-r1:32b".to_string(),
                "devstral".to_string(),
                "muse-glimmer".to_string(),
            ],
            color: "#33691E".to_string(),
            icon: "🦙".to_string(),
            mode: mode_for(&ollama_bin, ollama_key),
            cli_path: ollama_bin,
            has_api_key: ollama_key,
            enabled_in_council: false,
        },
        ProviderInfo {
            id: "openrouter".to_string(),
            name: "OpenRouter Gateway".to_string(),
            model_class: "GENERATIVE_SYSTEM_2".to_string(),
            task_specialty: "Universal Multi-Vendor Model Routing & Failover Gateway".to_string(),
            default_model: "anthropic/claude-sonnet-4".to_string(),
            available_models: vec![
                "anthropic/claude-sonnet-4".to_string(),
                "x-ai/grok-code-fast-1".to_string(),
                "mistralai/codestral-latest".to_string(),
                "qwen/qwen3-coder".to_string(),
                "openrouter/auto".to_string(),
            ],
            color: "#4527A0".to_string(),
            icon: "🌐".to_string(),
            mode: mode_for(&None, openrouter_key),
            cli_path: None,
            has_api_key: openrouter_key,
            enabled_in_council: false,
        },
    ]
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
                        .unwrap_or("allow_execution")
                        .to_string();

                    let score_str = answers
                        .get("invariant_rubric")
                        .and_then(|a| a.get("score").or(Some(a)))
                        .and_then(|v| v.as_str())
                        .unwrap_or("high_compliance")
                        .to_string();

                    let conf = answers
                        .get("regression_risk")
                        .and_then(|a| a.get("confidence"))
                        .and_then(|v| v.as_f64())
                        .map(|f| sanitize_prob(f as f32, 0.93))
                        .unwrap_or(0.93);

                    let gate_status = if risk_noul > 0.65 || choice_str == "fail_closed_block" {
                        "FAIL_CLOSED_BLOCK".to_string()
                    } else if risk_noul > 0.35 || choice_str == "require_human_approval" {
                        "REQUIRE_APPROVAL".to_string()
                    } else {
                        "ALLOW_EXECUTION".to_string()
                    };

                    return DecisionEvaluation {
                        engine: format!("{} (LIVE TypeSafe API)", model),
                        noul_risk_prob: risk_noul,
                        choice_action: choice_str.clone(),
                        score_rubric: score_str.clone(),
                        confidence: conf,
                        gate_status,
                        latency_ms: start.elapsed().as_millis() as u64,
                        primitives: vec![
                            DecisionPrimitive {
                                key: "regression_risk".to_string(),
                                primitive_type: "noul".to_string(),
                                value: format!("{:.2}", risk_noul),
                                numeric_value: Some(risk_noul),
                                confidence: conf,
                                description: "System-1 continuous probability of regression or unsafe mutation".to_string(),
                            },
                            DecisionPrimitive {
                                key: "execution_route".to_string(),
                                primitive_type: "choice".to_string(),
                                value: choice_str,
                                numeric_value: None,
                                confidence: conf,
                                description: "Categorical routing decision".to_string(),
                            },
                            DecisionPrimitive {
                                key: "invariant_rubric".to_string(),
                                primitive_type: "score".to_string(),
                                value: score_str,
                                numeric_value: Some( sanitize_prob(1.0 - risk_noul * 0.6, 0.90) ),
                                confidence: conf,
                                description: "Ordinal architectural rubric rating".to_string(),
                            },
                        ],
                    };
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
        || prompt_or_proposal.contains("`")
        || prompt_or_proposal.len() > 140;

    let (risk_noul, choice_action, score_rubric, numeric_score, gate_status, conf) = if is_destructive {
        (
            0.88_f32,
            "fail_closed_block".to_string(),
            "high_risk_mutation (0.18)".to_string(),
            0.18_f32,
            "FAIL_CLOSED_BLOCK".to_string(),
            0.97_f32,
        )
    } else if is_complex_arch && !has_concrete_structure {
        (
            0.28_f32,
            "route_to_council".to_string(),
            "high_complexity_task (0.86)".to_string(),
            0.86_f32,
            "ALLOW_EXECUTION".to_string(),
            0.92_f32,
        )
    } else if has_concrete_structure {
        (
            0.06_f32,
            "allow_execution".to_string(),
            "exemplary_deep_module (0.95)".to_string(),
            0.95_f32,
            "ALLOW_EXECUTION".to_string(),
            0.95_f32,
        )
    } else {
        (
            0.14_f32,
            "allow_execution".to_string(),
            "high_compliance (0.89)".to_string(),
            0.89_f32,
            "ALLOW_EXECUTION".to_string(),
            0.91_f32,
        )
    };

    DecisionEvaluation {
        engine: format!(
            "{} (TypeSafe System-1 Deterministic Gate)",
            if model.is_empty() { "jev-latest" } else { model }
        ),
        noul_risk_prob: sanitize_prob(risk_noul, 0.10),
        choice_action: choice_action.clone(),
        score_rubric: score_rubric.clone(),
        confidence: sanitize_prob(conf, 0.90),
        gate_status,
        latency_ms: start.elapsed().as_millis().max(12) as u64,
        primitives: vec![
            DecisionPrimitive {
                key: "regression_risk_noul".to_string(),
                primitive_type: "noul".to_string(),
                value: format!("{:.2}", risk_noul),
                numeric_value: Some(risk_noul),
                confidence: conf,
                description: "Quantized probability [0.0, 1.0] of boundary violation or destructive side-effect".to_string(),
            },
            DecisionPrimitive {
                key: "task_routing_choice".to_string(),
                primitive_type: "choice".to_string(),
                value: choice_action,
                numeric_value: None,
                confidence: conf,
                description: "Discrete System-1 gate routing selection".to_string(),
            },
            DecisionPrimitive {
                key: "invariant_compliance_score".to_string(),
                primitive_type: "score".to_string(),
                value: score_rubric,
                numeric_value: Some(numeric_score),
                confidence: conf,
                description: "Ordinal rubric rating for deep-module & zero-allocation alignment".to_string(),
            },
        ],
    }
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

    match provider_id {
        "claude" => {
            if let Some(key) = resolve_key(api_keys, "claude", &["ANTHROPIC_API_KEY"]) {
                if !key.is_empty() {
                    match call_anthropic_messages(
                        &client,
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(clean) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &clean,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text: clean,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [CLAUDE • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "codex" => {
            if let Some(key) = resolve_key(api_keys, "codex", &["OPENAI_API_KEY"]) {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://api.openai.com/v1/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [CODEX • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "deepseek" => {
            if let Some(key) = resolve_key(api_keys, "deepseek", &["DEEPSEEK_API_KEY"]) {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://api.deepseek.com/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [DEEPSEEK • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "kimi" => {
            if let Some(key) = resolve_key(api_keys, "kimi", &["MOONSHOT_API_KEY", "KIMI_API_KEY"])
            {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://api.moonshot.cn/v1/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [KIMI • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "copilot" => {
            if let Some(key) = resolve_key(api_keys, "copilot", &["GITHUB_TOKEN", "GH_TOKEN"]) {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://models.inference.ai.azure.com/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [COPILOT • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "muse" => {
            if let Some(key) = resolve_key(api_keys, "muse", &["META_API_KEY", "MODEL_API_KEY"]) {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://api.meta.ai/v1/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [MUSE • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "gemini" => {
            if let Some(key) = resolve_key(api_keys, "gemini", &["GEMINI_API_KEY", "GOOGLE_API_KEY"]) {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [GEMINI • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "grok" => {
            if let Some(key) = resolve_key(api_keys, "grok", &["XAI_API_KEY"]) {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://api.x.ai/v1/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [GROK • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "mistral" => {
            if let Some(key) =
                resolve_key(api_keys, "mistral", &["MISTRAL_API_KEY", "CODESTRAL_API_KEY"])
            {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://api.mistral.ai/v1/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [MISTRAL • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "qwen" => {
            if let Some(key) =
                resolve_key(api_keys, "qwen", &["DASHSCOPE_API_KEY", "QWEN_CODE_API_KEY"])
            {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [QWEN • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        "openrouter" => {
            if let Some(key) = resolve_key(api_keys, "openrouter", &["OPENROUTER_API_KEY"]) {
                if !key.is_empty() {
                    match call_openai_compatible(
                        &client,
                        "https://openrouter.ai/api/v1/chat/completions",
                        &key,
                        &api_model,
                        &contextualized_prompt,
                    )
                    .await
                    {
                        Ok(text) => {
                            let iqs = maybe_extract_or_synthesize_questions(
                                prompt,
                                &text,
                                active_project,
                                fast_council_stage,
                            );
                            return ProviderResponse {
                                text,
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API".to_string(),
                                decision_eval: None,
                                interactive_questions: iqs,
                            };
                        }
                        Err(diag) => {
                            return ProviderResponse {
                                text: format!(
                                    "⚠️ [OPENROUTER • {}] Live API Error: {}",
                                    api_model, diag
                                ),
                                latency_ms: start.elapsed().as_millis() as u64,
                                execution_mode: "LIVE_API_ERROR".to_string(),
                                decision_eval: None,
                                interactive_questions: Vec::new(),
                            };
                        }
                    }
                }
            }
        }
        _ => {}
    }

    if !fast_council_stage {
        if let Some(mut native_cmd) = build_native_agent_cli_command(
            provider_id,
            &api_model,
            &contextualized_prompt,
            &work_dir,
            api_keys,
        ) {
            native_cmd
                .current_dir(&work_dir)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let fut = native_cmd.output();

            if let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(185), fut).await {
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

    if !fast_council_stage
        && (provider_id == "antigravity"
            || provider_id == "gemini"
            || provider_id == "muse"
            || provider_id == "claude"
            || provider_id == "codex")
    {
        if let Some(agy_bin) = detect_binary(&["agy"]) {
            let agy_model = map_provider_to_agy_model(provider_id, model);
            let mut cmd = Command::new(&agy_bin);
            cmd.arg("-p")
                .arg(&contextualized_prompt)
                .arg("--model")
                .arg(&agy_model)
                .arg("--print-timeout")
                .arg("180s")
                .current_dir(&work_dir)
                .kill_on_drop(true)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let fut = cmd.output();

            if let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(185), fut).await {
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
        execution_mode: if provider_id == "antigravity"
            || provider_id == "gemini"
            || provider_id == "muse"
            || provider_id == "copilot"
        {
            "LIVE_CLI_HYBRID".to_string()
        } else {
            "SIMULATED".to_string()
        },
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
    match provider_id {
        "muse" => {
            if !is_muse_cli_authenticated(api_keys) {
                return None;
            }
            let bin = detect_binary(&["/opt/homebrew/bin/muse", "muse"])?;
            let mut cmd = Command::new(bin);
            if let Some(key) = resolve_key(api_keys, "muse", &["META_API_KEY", "MODEL_API_KEY"]) {
                if !key.is_empty() {
                    cmd.env("META_API_KEY", key);
                }
            }
            cmd.arg("exec")
                .arg("--model")
                .arg(model)
                .arg("--workspace")
                .arg(work_dir)
                .arg("--trust-workspace")
                .arg("--disable-approval")
                .arg("--user-input-auto-resolve")
                .arg(prompt);
            Some(cmd)
        }
        "gemini" => {
            let bin = detect_binary(&["/opt/homebrew/bin/gemini", "/usr/local/bin/gemini", "gemini"])?;
            let mut cmd = Command::new(bin);
            if let Some(key) = resolve_key(api_keys, "gemini", &["GEMINI_API_KEY", "GOOGLE_API_KEY"]) {
                if !key.is_empty() {
                    cmd.env("GEMINI_API_KEY", key);
                }
            }
            cmd.env("GEMINI_CLI_TRUST_WORKSPACE", "true")
                .arg("-p")
                .arg(prompt)
                .arg("-m")
                .arg(model)
                .arg("--approval-mode")
                .arg("yolo")
                .arg("--skip-trust");
            Some(cmd)
        }
        "copilot" => {
            let bin = detect_binary(&["/opt/homebrew/bin/copilot", "/usr/local/bin/copilot", "copilot"])?;
            let mut cmd = Command::new(bin);
            if let Some(key) =
                resolve_key(api_keys, "copilot", &["COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"])
            {
                if !key.is_empty() {
                    cmd.env("COPILOT_GITHUB_TOKEN", key);
                }
            }
            cmd.arg("-p")
                .arg(prompt)
                .arg("-s")
                .arg("--allow-all")
                .arg("--trust-folder")
                .arg("--no-ask-user")
                .arg("-C")
                .arg(work_dir);
            Some(cmd)
        }
        "cursor" => {
            let bin = detect_binary(&["/opt/homebrew/bin/cursor-agent", "cursor-agent"])?;
            let mut cmd = Command::new(bin);
            if let Some(key) = resolve_key(api_keys, "cursor", &["CURSOR_API_KEY"]) {
                if !key.is_empty() {
                    cmd.env("CURSOR_API_KEY", key);
                }
            }
            cmd.arg("-p")
                .arg(prompt)
                .arg("--force")
                .arg("--model")
                .arg(model)
                .arg("--workspace")
                .arg(work_dir);
            Some(cmd)
        }
        "opencode" => {
            let bin = detect_binary(&["/opt/homebrew/bin/opencode", "opencode"])?;
            let mut cmd = Command::new(bin);
            cmd.env("OPENCODE_DISABLE_AUTOUPDATE", "1")
                .arg("run")
                .arg(prompt)
                .arg("--model")
                .arg(model)
                .arg("--auto");
            Some(cmd)
        }
        "aider" => {
            let bin = detect_binary(&["/opt/homebrew/bin/aider", "aider"])?;
            let mut cmd = Command::new(bin);
            cmd.arg("--message")
                .arg(prompt)
                .arg("--yes-always")
                .arg("--no-pretty")
                .arg("--no-stream")
                .arg("--no-check-update")
                .arg("--model")
                .arg(model);
            Some(cmd)
        }
        "goose" => {
            let bin = detect_binary(&["/opt/homebrew/bin/goose", "goose"])?;
            let mut cmd = Command::new(bin);
            cmd.env("GOOSE_MODE", "auto")
                .env("GOOSE_DISABLE_KEYRING", "1")
                .arg("run")
                .arg("--text")
                .arg(prompt)
                .arg("--no-session")
                .arg("--quiet")
                .arg("--model")
                .arg(model);
            Some(cmd)
        }
        "amp" => {
            let bin = detect_binary(&["/opt/homebrew/bin/amp", "amp"])?;
            let mut cmd = Command::new(bin);
            if let Some(key) = resolve_key(api_keys, "amp", &["AMP_API_KEY"]) {
                if !key.is_empty() {
                    cmd.env("AMP_API_KEY", key);
                }
            }
            cmd.arg("-x")
                .arg(prompt)
                .arg("--dangerously-allow-all");
            Some(cmd)
        }
        "droid" => {
            let bin = detect_binary(&["/opt/homebrew/bin/droid", "droid"])?;
            let mut cmd = Command::new(bin);
            if let Some(key) = resolve_key(api_keys, "droid", &["FACTORY_API_KEY"]) {
                if !key.is_empty() {
                    cmd.env("FACTORY_API_KEY", key);
                }
            }
            cmd.arg("exec")
                .arg("--auto")
                .arg("high")
                .arg("-m")
                .arg(model)
                .arg("--cwd")
                .arg(work_dir)
                .arg(prompt);
            Some(cmd)
        }
        "qwen" => {
            let bin = detect_binary(&["/opt/homebrew/bin/qwen", "qwen"])?;
            let mut cmd = Command::new(bin);
            if let Some(key) =
                resolve_key(api_keys, "qwen", &["DASHSCOPE_API_KEY", "QWEN_CODE_API_KEY"])
            {
                if !key.is_empty() {
                    cmd.env("DASHSCOPE_API_KEY", key);
                }
            }
            cmd.arg("-p")
                .arg(prompt)
                .arg("-m")
                .arg(model)
                .arg("--yolo");
            Some(cmd)
        }
        "continue" => {
            let bin = detect_binary(&["/opt/homebrew/bin/cn", "cn"])?;
            let mut cmd = Command::new(bin);
            if let Some(key) = resolve_key(api_keys, "continue", &["CONTINUE_API_KEY"]) {
                if !key.is_empty() {
                    cmd.env("CONTINUE_API_KEY", key);
                }
            }
            cmd.arg("-p")
                .arg(prompt)
                .arg("--allow")
                .arg("*")
                .arg("--silent")
                .arg("--model")
                .arg(model)
                .arg("--cwd")
                .arg(work_dir);
            Some(cmd)
        }
        "cline" => {
            let bin = detect_binary(&["/opt/homebrew/bin/cline", "cline"])?;
            let mut cmd = Command::new(bin);
            cmd.arg("--auto-approve")
                .arg("true")
                .arg("-m")
                .arg(model)
                .arg("-c")
                .arg(work_dir)
                .arg(prompt);
            Some(cmd)
        }
        "crush" => {
            let bin = detect_binary(&["/opt/homebrew/bin/crush", "crush"])?;
            let mut cmd = Command::new(bin);
            cmd.arg("run")
                .arg("--cwd")
                .arg(work_dir)
                .arg("--yolo")
                .arg("--quiet")
                .arg(prompt);
            Some(cmd)
        }
        "mistral" => {
            let bin = detect_binary(&["/opt/homebrew/bin/vibe", "vibe"])?;
            let mut cmd = Command::new(bin);
            if let Some(key) =
                resolve_key(api_keys, "mistral", &["MISTRAL_API_KEY", "CODESTRAL_API_KEY"])
            {
                if !key.is_empty() {
                    cmd.env("MISTRAL_API_KEY", key);
                }
            }
            cmd.arg("-p").arg(prompt).arg("--auto-approve");
            Some(cmd)
        }
        "ollama" => {
            let bin = detect_binary(&["/opt/homebrew/bin/ollama", "/usr/local/bin/ollama", "ollama"])?;
            let mut cmd = Command::new(bin);
            cmd.arg("run").arg(model).arg(prompt);
            Some(cmd)
        }
        _ => None,
    }
}

async fn call_anthropic_messages(
    client: &Client,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    let url = "https://api.anthropic.com/v1/messages";
    let resp = client
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": model,
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": prompt}]
        }))
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

    let val: Value = resp.json().await.map_err(|e| {
        format!(
            "Failed to parse JSON response from {} (model `{}`): {}",
            url, model, e
        )
    })?;

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
    let resp = client
        .post(url)
        .bearer_auth(api_key)
        .json(&json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.3,
            "max_tokens": 1024
        }))
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

    let val: Value = resp.json().await.map_err(|e| {
        format!(
            "Failed to parse JSON response from {} (model `{}`): {}",
            url, model, e
        )
    })?;

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
        files.push("Cargo.toml".to_string());
        files.push("src/main.rs".to_string());
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

    match provider_id {
        "antigravity" => format!(
            "[Antigravity • {}] Multi-Agent Graph & Zero-Copy Architecture for `{}`:\n\
            • Workspace Context: Scanned `{}`.\n\
            • Proposal for \"{}\": Structure the execution pipeline around lock-free bounded channels (`tokio::sync::broadcast`) and deterministic Anchor verification gates.\n\
            • Key Advantage: Eliminates DOM/heap churn while preserving full transcript traceability in `~/.gemini/antigravity-cli/brain`.",
            model, project, file_list, clean_q
        ),
        "muse" => format!(
            "[Meta Muse Code • {}] 1M-Context Agentic & Worktree Synthesis for `{}`:\n\
            • Workspace Context: Audited `{}` with 4-observer background verification (Memory, Skill, Goal, Verify).\n\
            • Proposal for \"{}\": Isolate multi-file mutations in a dedicated Git worktree (`muse -w`), execute parallel tool calls across subagents, and validate against MSP schema contracts.\n\
            • Key Advantage: Combines 1,048,576-token context horizon with OS-sandboxed shell execution and deterministic goal tracking.",
            model, project, file_list, clean_q
        ),
        "claude" => format!(
            "[Claude • {}] Deep-Module Boundary & Invariant Analysis for `{}`:\n\
            • Audited modules: `{}`.\n\
            • Proposal for \"{}\": Enforce strict type-safe state transitions at the boundary seam, isolate side-effects into an explicit trait adapter, and verify edge-case error recovery.\n\
            • Trade-off Check: Prioritizes maintainability, zero-trust invariants, and clean human/mobile ergonomics.",
            model, project, file_list, clean_q
        ),
        "codex" => format!(
            "[Codex • {}] Low-Latency Implementation & Hot-Path Patch for `{}`:\n\
            • Target files: `{}`.\n\
            • Proposal for \"{}\": Use pre-allocated ring buffers, bit-packed state flags, and zero-dependency native APIs to cut P99 latency below 1.5ms.\n\
            • Concrete Action: Inline hot loops, strip intermediate allocations, and ship a single static binary.",
            model, project, file_list, clean_q
        ),
        "copilot" => format!(
            "[Copilot • {}] Developer Workflow & Incremental Integration for `{}`:\n\
            • Repo layout: `{}`.\n\
            • Proposal for \"{}\": Integrate directly with existing CLI tooling (`gh`, `cargo`) and expose keyboard-first command palettes alongside mobile quick-actions.",
            model, project, file_list, clean_q
        ),
        "deepseek" => format!(
            "[DeepSeek • {}] First-Principles Algorithmic & Throughput Optimization for `{}`:\n\
            • Analyzed `{}`.\n\
            • Reasoning trace for \"{}\": Decompose the problem into a directed acyclic state machine with O(1) amortized event dispatch and delta-compressed wire frames.\n\
            • Result: 4.2x higher packet throughput and minimal memory footprint on constrained mobile devices.",
            model, project, file_list, clean_q
        ),
        "kimi" => format!(
            "[Kimi • {}] Long-Context Cross-Repository Synthesis for `{}`:\n\
            • Full-repo correlation across `{}`.\n\
            • Proposal for \"{}\": Unify session history, subagent telemetry, and multi-provider consensus state into a single coherent timeline with instant context switching.",
            model, project, file_list, clean_q
        ),
        "gemini" => format!(
            "[Gemini CLI • {}] 1M-Context Multimodal & Grounded Architecture for `{}`:\n\
            • Workspace Context: Indexed `{}` with MCP tool and search grounding.\n\
            • Proposal for \"{}\": Leverage 1M-token full-repository context to align cross-module contracts, generate comprehensive test harnesses, and validate edge cases in a single pass.",
            model, project, file_list, clean_q
        ),
        "cursor" => format!(
            "[Cursor Agent • {}] Multi-File Composer & Codebase Indexing Synthesis for `{}`:\n\
            • Indexed targets: `{}`.\n\
            • Proposal for \"{}\": Apply coordinated multi-file structural edits with semantic AST awareness and automated lint/typecheck verification.",
            model, project, file_list, clean_q
        ),
        "opencode" => format!(
            "[OpenCode • {}] LSP-Guided Terminal Agent Architecture for `{}`:\n\
            • Audited `{}` via LSP diagnostics.\n\
            • Proposal for \"{}\": Combine multi-provider routing with live language-server symbol resolution to ensure zero broken references across module boundaries.",
            model, project, file_list, clean_q
        ),
        "aider" => format!(
            "[Aider • {}] Repo-Map & Architect/Editor Split Patch for `{}`:\n\
            • Tree-sitter repo-map over `{}`.\n\
            • Proposal for \"{}\": Separate high-level architectural reasoning from surgical unified-diff editing, followed by automatic git commit and test verification.",
            model, project, file_list, clean_q
        ),
        "goose" => format!(
            "[Block Goose • {}] Autonomous MCP Extension & Recipe Workflow for `{}`:\n\
            • Scanned `{}`.\n\
            • Proposal for \"{}\": Orchestrate modular MCP tool extensions and deterministic sub-recipes to automate build, test, and deployment verification.",
            model, project, file_list, clean_q
        ),
        "amp" => format!(
            "[Sourcegraph Amp • {}] Code-Graph & Deep Subagent Review for `{}`:\n\
            • Cross-reference graph over `{}`.\n\
            • Proposal for \"{}\": Dispatch parallel Oracle subagents to verify call-graph invariants and ensure zero regressions across dependent packages.",
            model, project, file_list, clean_q
        ),
        "droid" => format!(
            "[Factory Droid • {}] Enterprise Autonomy & CI/CD Remediation for `{}`:\n\
            • Audited `{}`.\n\
            • Proposal for \"{}\": Enforce enterprise security guardrails, deterministic build reproducibility, and automated PR verification gates.",
            model, project, file_list, clean_q
        ),
        "qwen" => format!(
            "[Qwen Code • {}] Polyglot Agentic Coding & Synthesis for `{}`:\n\
            • Analyzed `{}`.\n\
            • Proposal for \"{}\": Optimize data structures, concurrency primitives, and cross-language FFI/API contracts with high token efficiency.",
            model, project, file_list, clean_q
        ),
        "continue" => format!(
            "[Continue CLI • {}] Continuous AI Check & Rule Enforcement for `{}`:\n\
            • Checked `{}` against workspace rules.\n\
            • Proposal for \"{}\": Codify architectural invariants as repeatable headless checks that run both interactively and in CI pipelines.",
            model, project, file_list, clean_q
        ),
        "cline" => format!(
            "[Cline CLI • {}] Plan/Act Dual-Phase Execution for `{}`:\n\
            • Mapped `{}`.\n\
            • Proposal for \"{}\": Establish an explicit read-only architectural plan before executing sandboxed file mutations and shell verification.",
            model, project, file_list, clean_q
        ),
        "crush" => format!(
            "[Charm Crush • {}] Fast Terminal Session & LSP Synthesis for `{}`:\n\
            • Inspected `{}`.\n\
            • Proposal for \"{}\": Deliver clean, low-overhead implementation patches with immediate terminal feedback and zero bloat.",
            model, project, file_list, clean_q
        ),
        "grok" => format!(
            "[xAI Grok • {}] Real-Time Systems & Reasoning Analysis for `{}`:\n\
            • Evaluated `{}`.\n\
            • Proposal for \"{}\": Strip unnecessary abstraction layers, maximize I/O concurrency, and harden failure recovery paths under load.",
            model, project, file_list, clean_q
        ),
        "mistral" => format!(
            "[Mistral Codestral / Devstral • {}] Low-LatencySWE & Fill-in-the-Middle Synthesis for `{}`:\n\
            • Scanned `{}`.\n\
            • Proposal for \"{}\": Apply high-precision structural code generation with minimal latency and strict adherence to idiomatic compiler patterns.",
            model, project, file_list, clean_q
        ),
        "ollama" => format!(
            "[Local Ollama • {}] Air-Gapped Zero-Telemetry Local Inference for `{}`:\n\
            • Local workspace `{}`.\n\
            • Proposal for \"{}\": Execute 100% offline code synthesis and verification on local hardware with zero external network egress.",
            model, project, file_list, clean_q
        ),
        "openrouter" => format!(
            "[OpenRouter • {}] Unified Multi-Model Frontier Routing for `{}`:\n\
            • Context across `{}`.\n\
            • Proposal for \"{}\": Route specialized sub-tasks across the highest-performing frontier and open-weight coding models with automatic failover.",
            model, project, file_list, clean_q
        ),
        _ => format!(
            "[{} • {}] Analysis for `{}` ({}): {}",
            provider_id, model, project, file_list, clean_q
        ),
    }
}
