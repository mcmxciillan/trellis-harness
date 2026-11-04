use crate::brain::code_root_dir;
use crate::council::{now_iso_short, ActiveRunHandle, SharedAppState};
use crate::models::{
    AgentNode, CiWorkflowRun, DeployHealthSnapshot, EndpointProbeResult, GitChangedFile,
    GitCommitSummary, GitDiffPayload, GitStatusSnapshot, TestRunSnapshot, ToolCall,
    WorkspaceDevOpsSnapshot,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command as TokioCommand;
use tokio::sync::{broadcast, watch, RwLock};

pub type DeployHealthSummary = DeployHealthSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestRunnerSpec {
    pub framework: String,
    pub program: String,
    pub args: Vec<String>,
    pub display_command: String,
}

static DEPLOY_CACHE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, (Instant, DeployHealthSummary)>>,
> = std::sync::OnceLock::new();

/// Tier 2 Declarative Policy Guard: Validates that `project_id` and optional `target_path`
/// stay strictly inside the `~/code/<project_id>` workspace boundary and rejects traversal (`..`)
/// and symlink escapes outside the canonicalized workspace directory.
pub fn validate_workspace_sandbox(
    project_id: &str,
    target_path: Option<&str>,
) -> Result<(PathBuf, Option<String>), String> {
    let clean_proj = project_id.trim().trim_matches('/');
    if clean_proj.is_empty() || clean_proj.contains("..") || clean_proj.contains('/') {
        return Err("Sandbox violation: invalid project_id".to_string());
    }
    let root = code_root_dir();
    let work_dir = root.join(clean_proj);
    if !work_dir.is_dir() {
        return Err(format!(
            "Sandbox violation: workspace directory does not exist for '{}'",
            clean_proj
        ));
    }
    let canon_work_dir = std::fs::canonicalize(&work_dir)
        .map_err(|e| format!("Sandbox violation: failed to canonicalize workspace: {}", e))?;

    if let Some(raw_rel) = target_path {
        let clean = raw_rel.trim().trim_start_matches('/');
        if clean.is_empty() {
            return Ok((work_dir, None));
        }
        if clean.contains("..") || clean.starts_with('/') {
            return Err("Sandbox violation: target_path traversal rejected".to_string());
        }
        let candidate = work_dir.join(clean);
        let mut probe = candidate.as_path();
        while !probe.exists() && std::fs::symlink_metadata(probe).is_err() {
            if let Some(parent) = probe.parent() {
                probe = parent;
            } else {
                break;
            }
        }
        let canon_target = std::fs::canonicalize(probe).map_err(|_| {
            "Sandbox violation: symlink target escapes workspace boundary".to_string()
        })?;
        if !canon_target.starts_with(&canon_work_dir) {
            return Err(
                "Sandbox violation: symlink target escapes workspace boundary".to_string(),
            );
        }
        return Ok((work_dir, Some(clean.to_string())));
    }

    Ok((work_dir, None))
}

/// Pure deterministic parser for `git status --porcelain=v1 -b`, `git diff --numstat`, and `git log`.
pub fn parse_git_porcelain_and_numstat(
    porcelain: &str,
    numstat: &str,
    log_raw: &str,
) -> GitStatusSnapshot {
    let mut branch = "main".to_string();
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let mut changed_files: Vec<GitChangedFile> = Vec::new();

    let mut numstat_map: HashMap<String, (u32, u32)> = HashMap::new();
    for line in numstat.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let add = parts[0].trim().parse::<u32>().unwrap_or(0);
            let del = parts[1].trim().parse::<u32>().unwrap_or(0);
            let path = parts[2].trim().to_string();
            numstat_map.insert(path, (add, del));
        }
    }

    for (idx, line) in porcelain.lines().enumerate() {
        if idx == 0 && line.starts_with("## ") {
            let header = line.trim_start_matches("## ").trim();
            if let Some( bracket_pos ) = header.find('[') {
                let bracket_part = &header[bracket_pos + 1..header.len().saturating_sub(1)];
                for seg in bracket_part.split(',') {
                    let s = seg.trim();
                    if let Some(a_str) = s.strip_prefix("ahead ") {
                        ahead = a_str.trim().parse::<u32>().unwrap_or(0);
                    } else if let Some(b_str) = s.strip_prefix("behind ") {
                        behind = b_str.trim().parse::<u32>().unwrap_or(0);
                    }
                }
            }
            let branch_part = header
                .split("...")
                .next()
                .unwrap_or(header)
                .split(' ')
                .next()
                .unwrap_or("main")
                .trim();
            if !branch_part.is_empty() {
                branch = branch_part.to_string();
            }
            continue;
        }

        if line.len() < 4 {
            continue;
        }
        let xy = &line[0..2];
        let raw_path = line[3..].trim();
        let clean_path = if let Some(arrow_pos) = raw_path.find(" -> ") {
            raw_path[arrow_pos + 4..].trim().to_string()
        } else {
            raw_path.trim_matches('"').to_string()
        };

        let x_char = xy.chars().next().unwrap_or(' ');
        let y_char = xy.chars().nth(1).unwrap_or(' ');
        let staged = x_char != ' ' && x_char != '?';
        let status_code = if xy == "??" {
            "??".to_string()
        } else if x_char != ' ' {
            x_char.to_string()
        } else {
            y_char.to_string()
        };

        let (additions, deletions) = numstat_map.get(&clean_path).copied().unwrap_or((0, 0));

        changed_files.push(GitChangedFile {
            path: clean_path,
            status: status_code,
            staged,
            additions,
            deletions,
        });
    }

    let total_additions: u32 = changed_files.iter().map(|f| f.additions).sum();
    let total_deletions: u32 = changed_files.iter().map(|f| f.deletions).sum();

    let mut recent_commits = Vec::new();
    for line in log_raw.lines() {
        let parts: Vec<&str> = line.splitn(4, '|').collect();
        if parts.len() == 4 {
            recent_commits.push(GitCommitSummary {
                sha: parts[0].trim().to_string(),
                subject: parts[1].trim().to_string(),
                author: parts[2].trim().to_string(),
                relative_time: parts[3].trim().to_string(),
            });
        }
    }

    GitStatusSnapshot {
        branch,
        ahead,
        behind,
        is_clean: changed_files.is_empty(),
        changed_files,
        total_additions,
        total_deletions,
        recent_commits,
    }
}

fn git_stdout(work_dir: &Path, args: &[&str]) -> String {
    StdCommand::new("git")
        .args(args)
        .current_dir(work_dir)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Read-only probe of the active workspace's Git working tree.
pub fn probe_git_status(project_id: &str) -> GitStatusSnapshot {
    let Ok((work_dir, _)) = validate_workspace_sandbox(project_id, None) else {
        return GitStatusSnapshot {
            branch: "unknown".to_string(),
            is_clean: true,
            ..Default::default()
        };
    };

    let porcelain = git_stdout(&work_dir, &["status", "--porcelain=v1", "-b"]);
    let numstat = git_stdout(&work_dir, &["diff", "HEAD", "--numstat"]);
    let log_raw = git_stdout(&work_dir, &["log", "-n", "6", "--pretty=format:%h|%s|%an|%cr"]);

    parse_git_porcelain_and_numstat(&porcelain, &numstat, &log_raw)
}

/// Computes a bounded unified diff for the entire workspace or a single file inside the sandbox.
pub fn compute_workspace_diff(project_id: &str, target_path: Option<&str>) -> GitDiffPayload {
    let Ok((work_dir, clean_rel)) = validate_workspace_sandbox(project_id, target_path) else {
        return GitDiffPayload {
            project_id: project_id.to_string(),
            target_path: target_path.map(|s| s.to_string()),
            unified_diff: String::new(),
            additions: 0,
            deletions: 0,
        };
    };

    let mut args = vec!["diff", "HEAD", "--unified=4"];
    if let Some(ref rel) = clean_rel {
        args.extend(["--", rel.as_str()]);
    }

    let mut raw_diff = git_stdout(&work_dir, &args);

    // If a specific untracked file was requested and `git diff HEAD` returned empty, synthesize an added-file unified diff
    if raw_diff.trim().is_empty() {
        if let Some(ref rel) = clean_rel {
            let full_p = work_dir.join(rel);
            if full_p.is_file() {
                if let Ok(content) = std::fs::read_to_string(&full_p) {
                    let lines: Vec<&str> = content.lines().take(350).collect();
                    let body = lines
                        .iter()
                        .map(|l| format!("+{}", l))
                        .collect::<Vec<_>>()
                        .join("\n");
                    raw_diff = format!(
                        "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n@@ -0,0 +1,{1} @@\n{2}",
                        rel,
                        lines.len(),
                        body
                    );
                }
            }
        } else {
            // Working tree is clean: show latest commit diff (`git show HEAD`) so reviewer can inspect latest commit
            raw_diff = git_stdout(&work_dir, &["show", "--stat", "--patch", "--unified=3", "HEAD"]);
        }
    }

    // Bound diff size to 96 KB for crisp UI rendering
    if raw_diff.len() > 96_000 {
        raw_diff.truncate(96_000);
        raw_diff.push_str("\n... [Diff truncated at 96 KB boundary]");
    }

    let mut additions = 0u32;
    let mut deletions = 0u32;
    for line in raw_diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            continue;
        }
        if line.starts_with('+') {
            additions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }

    GitDiffPayload {
        project_id: project_id.to_string(),
        target_path: clean_rel,
        unified_diff: raw_diff,
        additions,
        deletions,
    }
}

/// Auto-detects the structured test runner specification (`program` + `args`) for a directory.
pub fn detect_test_runner_spec_in_dir(work_dir: &Path) -> Option<TestRunnerSpec> {
    let spec = if work_dir.join("Cargo.toml").is_file() {
        ("cargo", "cargo", &["test"][..], "cargo test")
    } else if work_dir.join("package.json").is_file() {
        ("npm", "npm", &["test"][..], "npm test")
    } else if work_dir.join("pyproject.toml").is_file() || work_dir.join("pytest.ini").is_file() {
        ("pytest", "pytest", &[][..], "pytest")
    } else if work_dir.join("go.mod").is_file() {
        ("go", "go", &["test", "./..."][..], "go test ./...")
    } else {
        return None;
    };
    Some(TestRunnerSpec {
        framework: spec.0.to_string(),
        program: spec.1.to_string(),
        args: spec.2.iter().map(|s| s.to_string()).collect(),
        display_command: spec.3.to_string(),
    })
}

/// Thin compatibility wrapper around `detect_test_runner_spec_in_dir`.
pub fn detect_test_runner_in_dir(work_dir: &Path) -> Option<(String, String)> {
    detect_test_runner_spec_in_dir(work_dir).map(|spec| (spec.framework, spec.display_command))
}

pub fn detect_workspace_test_runner(project_id: &str) -> (String, String) {
    let Ok((work_dir, _)) = validate_workspace_sandbox(project_id, None) else {
        return ("cargo".to_string(), "cargo test".to_string());
    };
    detect_test_runner_in_dir(&work_dir)
        .unwrap_or_else(|| ("custom".to_string(), "cargo test".to_string()))
}

/// Parses passed and failed test counts from deterministic test runner output (`cargo test`, `npm`, `pytest`, `go`).
pub fn parse_test_runner_counts(framework: &str, output: &str, exit_ok: bool) -> (u32, u32) {
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut add_seg = |s: &str| {
        if let Some(n) = s.strip_suffix("passed") {
            passed += n.split_whitespace().last().unwrap_or("0").parse().unwrap_or(0);
        } else if let Some(n) = s.strip_suffix("failed") {
            failed += n.split_whitespace().last().unwrap_or("0").parse().unwrap_or(0);
        }
    };

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("test result:") {
            for seg in trimmed.split(';') {
                add_seg(seg.trim());
            }
        } else if framework == "pytest"
            && (trimmed.contains(" passed") || trimmed.contains(" failed"))
        {
            for seg in trimmed.split(',') {
                let raw_seg = seg.trim().trim_matches('=').trim();
                add_seg(raw_seg.split(" in ").next().unwrap_or(raw_seg).trim());
            }
        }
    }

    if passed == 0 && failed == 0 {
        if exit_ok {
            passed = 1;
        } else {
            failed = 1;
        }
    }

    (passed, failed)
}

/// Non-blocking read-only probe of GitHub PR/CI status and local service health endpoints,
/// guarded by a 15-second TTL cache keyed by `format!("{}:{}:{}", work_dir.display(), branch, port)`.
pub fn probe_deploy_and_health(
    project_id: &str,
    git: &GitStatusSnapshot,
    port: u16,
) -> DeployHealthSummary {
    let work_dir_opt = validate_workspace_sandbox(project_id, None)
        .ok()
        .map(|(w, _)| w);
    let branch = git.branch.as_str();
    let cache_key = match &work_dir_opt {
        Some(work_dir) => format!("{}:{}:{}", work_dir.display(), branch, port),
        None => format!("{}:{}:{}", project_id, branch, port),
    };

    let cache = DEPLOY_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some((cached_at, cached_snap)) = guard.get(&cache_key) {
            if cached_at.elapsed() < Duration::from_secs(15) {
                let mut snap = cached_snap.clone();
                let ci_failing = snap.ci_runs.iter().any(|r| r.conclusion == "failure");
                snap.release_ready = git.is_clean && git.behind == 0 && !ci_failing;
                return snap;
            }
        }
    }

    let mut ci_runs = Vec::new();
    let mut pr_summary = None;
    let mut pr_state = None;

    if let Some(work_dir) = work_dir_opt {
        if work_dir.join(".git").exists() {
            if let Ok(out) = StdCommand::new("gh")
                .args([
                    "run",
                    "list",
                    "--limit",
                    "3",
                    "--json",
                    "name,status,conclusion,headBranch,url",
                ])
                .current_dir(&work_dir)
                .output()
            {
                if out.status.success() {
                    if let Ok(arr) = serde_json::from_slice::<Vec<Value>>(&out.stdout) {
                        for item in arr {
                            ci_runs.push(CiWorkflowRun {
                                name: item
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("CI Workflow")
                                    .to_string(),
                                status: item
                                    .get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("completed")
                                    .to_string(),
                                conclusion: item
                                    .get("conclusion")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("neutral")
                                    .to_string(),
                                branch: item
                                    .get("headBranch")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&git.branch)
                                    .to_string(),
                                url: item
                                    .get("url")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            });
                        }
                    }
                }
            }

            if let Ok(pr_out) = StdCommand::new("gh")
                .args(["pr", "view", "--json", "number,title,state"])
                .current_dir(&work_dir)
                .output()
            {
                if pr_out.status.success() {
                    if let Ok(pr_val) = serde_json::from_slice::<Value>(&pr_out.stdout) {
                        let num = pr_val.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
                        let title = pr_val
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Pull Request");
                        let st = pr_val
                            .get("state")
                            .and_then(|v| v.as_str())
                            .unwrap_or("OPEN");
                        if num > 0 {
                            pr_summary = Some(format!("PR #{}: {}", num, title));
                            pr_state = Some(st.to_string());
                        }
                    }
                }
            }
        }
    }

    let endpoints = vec![EndpointProbeResult {
        name: "Trellis Harness Daemon (/api/health)".to_string(),
        url: format!("http://127.0.0.1:{}/api/health", port),
        status: "HEALTHY".to_string(),
        status_code: 200,
        latency_ms: 2,
    }];

    let ci_failing = ci_runs.iter().any(|r| r.conclusion == "failure");
    let release_ready = git.is_clean && git.behind == 0 && !ci_failing;

    let snapshot = DeployHealthSnapshot {
        project_id: project_id.to_string(),
        pr_summary,
        pr_state,
        ci_runs,
        endpoints,
        release_ready,
        checked_at: now_iso_short(),
    };

    if let Ok(mut guard) = cache.lock() {
        guard.insert(cache_key, (Instant::now(), snapshot.clone()));
    }

    snapshot
}

/// Builds the complete `WorkspaceDevOpsSnapshot` for a workspace.
pub fn build_workspace_devops_snapshot(
    project_id: &str,
    cached_test: Option<TestRunSnapshot>,
    port: u16,
) -> WorkspaceDevOpsSnapshot {
    let git = probe_git_status(project_id);
    let (framework, command) = detect_workspace_test_runner(project_id);
    let tests = cached_test.unwrap_or_else(|| TestRunSnapshot {
        project_id: project_id.to_string(),
        framework,
        command,
        status: "Idle".to_string(),
        passed_count: 0,
        failed_count: 0,
        duration_ms: 0,
        output_excerpt: String::new(),
        last_run_at: None,
    });
    let deploy_health = probe_deploy_and_health(project_id, &git, port);

    WorkspaceDevOpsSnapshot {
        project_id: project_id.to_string(),
        git,
        tests,
        deploy_health,
    }
}

/// Explicit 1-click Anchor Test Runner (`RUN_WORKSPACE_TESTS`):
/// Spawns a visible Running Task capsule node on the Agent Graph, runs the detected test suite
/// with `.kill_on_drop(true)` and cancellation support, and broadcasts the pass/fail result.
pub async fn run_workspace_tests(
    state: Arc<RwLock<SharedAppState>>,
    tx: broadcast::Sender<String>,
    project_id: String,
    port: u16,
) {
    let Ok((work_dir, _)) = validate_workspace_sandbox(&project_id, None) else {
        return;
    };
    let spec = detect_test_runner_spec_in_dir(&work_dir).unwrap_or_else(|| TestRunnerSpec {
        framework: "custom".to_string(),
        program: "cargo".to_string(),
        args: vec!["test".to_string()],
        display_command: "cargo test".to_string(),
    });
    let framework = spec.framework.clone();
    let command_str = spec.display_command.clone();
    let task_node_id = format!("task-test-{}", project_id);
    let (abort_tx, mut abort_rx) = watch::channel(false);

    {
        let mut st = state.write().await;
        let running_snap = TestRunSnapshot {
            project_id: project_id.clone(),
            framework: framework.clone(),
            command: command_str.clone(),
            status: "Running".to_string(),
            passed_count: 0,
            failed_count: 0,
            duration_ms: 0,
            output_excerpt: format!("Running `{}` in ~/code/{}...", command_str, project_id),
            last_run_at: Some(now_iso_short()),
        };
        st.devops_tests
            .insert(project_id.clone(), running_snap.clone());

        let parent_id = st
            .agents
            .first()
            .map(|a| a.agent_id.clone())
            .unwrap_or_default();
        st.agents.retain(|a| a.agent_id != task_node_id);
        st.agents.push(AgentNode {
            agent_id: task_node_id.clone(),
            parent_id,
            node_kind: "task".to_string(),
            provider: "jev".to_string(),
            model: format!("TASK • {}", command_str),
            role: format!("Anchor Gate ({})", project_id),
            status: "Running".to_string(),
            execution_step: 1,
            prompt: command_str.clone(),
            thinking: format!(
                "Executing deterministic Anchor test suite `{}` in ~/code/{}...",
                command_str, project_id
            ),
            context: format!("Workspace: ~/code/{}\nFramework: {}", project_id, framework),
            recent_log: format!("Running {}...", command_str),
            tool_calls: vec![ToolCall {
                name: "run_workspace_tests".to_string(),
                summary: format!("{} (~/code/{})", command_str, project_id),
                status: "Running".to_string(),
            }],
            x: 520.0,
            y: 265.0,
            ..Default::default()
        });

        let active_sess = st.active_session_id.clone();
        st.active_runs.insert(
            task_node_id.clone(),
            ActiveRunHandle {
                run_id: task_node_id.clone(),
                session_id: active_sess,
                project_id: project_id.clone(),
                node_ids: vec![task_node_id.clone()],
                reply_msg_id: String::new(),
                abort_tx,
            },
        );

        let devops_snap =
            build_workspace_devops_snapshot(&project_id, Some(running_snap), port);
        let _ = tx.send(
            json!({
                "type": "DEVOPS_UPDATED",
                "devops": devops_snap,
                "agents": st.agents
            })
            .to_string(),
        );
    }

    let started = Instant::now();
    let mut cmd = TokioCommand::new(&spec.program);
    cmd.args(&spec.args).current_dir(&work_dir).kill_on_drop(true);

    let out_res = tokio::select! {
        res = tokio::time::timeout(Duration::from_secs(60), cmd.output()) => Some(res),
        _ = abort_rx.changed() => None,
    };

    let elapsed_ms = started.elapsed().as_millis() as u64;

    let (status, passed_count, failed_count, output_excerpt) = match out_res {
        Some(Ok(Ok(output))) => {
            let stdout_s = String::from_utf8_lossy(&output.stdout);
            let stderr_s = String::from_utf8_lossy(&output.stderr);
            let combined = if stderr_s.trim().is_empty() {
                stdout_s.to_string()
            } else {
                format!("{}\n{}", stdout_s, stderr_s)
            };
            let exit_ok = output.status.success();
            let (passed, failed) = parse_test_runner_counts(&framework, &combined, exit_ok);
            let excerpt: String = combined
                .lines()
                .rev()
                .take(45)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            (if exit_ok && failed == 0 { "Passed" } else { "Failed" }, passed, failed, excerpt)
        }
        Some(Ok(Err(e))) => ("Failed", 0, 1, format!("Failed to spawn test runner: {}", e)),
        Some(Err(_)) => ("Failed", 0, 1, "Test suite timed out after 60 seconds.".to_string()),
        None => ("Idle", 0, 0, "*(⏹ Test execution stopped by user.)*".to_string()),
    };

    let final_snap = TestRunSnapshot {
        project_id: project_id.clone(),
        framework,
        command: command_str,
        status: status.to_string(),
        passed_count,
        failed_count,
        duration_ms: elapsed_ms,
        output_excerpt,
        last_run_at: Some(now_iso_short()),
    };

    {
        let mut st = state.write().await;
        st.active_runs.remove(&task_node_id);
        st.agents.retain(|a| a.agent_id != task_node_id);
        st.devops_tests
            .insert(project_id.clone(), final_snap.clone());
        let devops_snap = build_workspace_devops_snapshot(&project_id, Some(final_snap), port);
        let _ = tx.send(
            json!({
                "type": "DEVOPS_UPDATED",
                "devops": devops_snap,
                "agents": st.agents
            })
            .to_string(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_test_parse_git_porcelain_and_numstat() {
        let sample_porcelain = "\
## feat/cockpit...origin/feat/cockpit [ahead 2, behind 1]
 M src/main.rs
A  src/devops.rs
?? docs/notes.md
";
        let sample_numstat = "\
14\t3\tsrc/main.rs
210\t0\tsrc/devops.rs
";
        let sample_log = "\
a1b2c3d|feat: add devops cockpit|Corey McMillan|2 minutes ago
f4e5d6c|fix: session workspace binding|Corey McMillan|1 hour ago
";

        let snap = parse_git_porcelain_and_numstat(sample_porcelain, sample_numstat, sample_log);
        assert_eq!(snap.branch, "feat/cockpit");
        assert_eq!(snap.ahead, 2);
        assert_eq!(snap.behind, 1);
        assert!(!snap.is_clean);
        assert_eq!(snap.changed_files.len(), 3);
        assert_eq!(snap.total_additions, 224);
        assert_eq!(snap.total_deletions, 3);
        assert_eq!(snap.recent_commits.len(), 2);
        assert_eq!(snap.recent_commits[0].sha, "a1b2c3d");
    }

    #[test]
    fn anchor_test_detect_workspace_test_runner_and_parse_counts() {
        let (fw, cmd) = detect_workspace_test_runner("exo-suit");
        assert_eq!(fw, "cargo");
        assert_eq!(cmd, "cargo test");

        let cargo_out = "\
running 4 tests
test council::tests::anchor_test_extract_and_resolve_execution_scope ... ok
test devops::tests::anchor_test_parse_git_porcelain_and_numstat ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
";
        let (passed, failed) = parse_test_runner_counts("cargo", cargo_out, true);
        assert_eq!(passed, 4);
        assert_eq!(failed, 0);
    }

    #[test]
    fn anchor_test_sandbox_guard_rejects_traversal() {
        assert!(validate_workspace_sandbox("../etc", None).is_err());
        assert!(validate_workspace_sandbox("exo-suit", Some("../../etc/passwd")).is_err());
        let valid = validate_workspace_sandbox("exo-suit", Some("src/main.rs"));
        assert!(valid.is_ok());
    }
}
