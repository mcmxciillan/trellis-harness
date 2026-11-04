use crate::models::{
    AgentNode, ArtifactItem, ChatMessage, DecisionEvaluation, DecisionPrimitive, ExecutionScope,
    FileEntry, FileTreeInfo, InteractivePrompt, InteractiveQuestionItem, OpenedFile, SessionItem,
    SessionProvisionResult, SkillItem, TokenMetrics, ToolCall, ToolCatalogItem,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn brain_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("EXO_BRAIN_DIR") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    home_dir().join(".gemini").join("antigravity-cli").join("brain")
}

pub fn code_root_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("EXO_CODE_ROOT") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    home_dir().join("code")
}

pub fn atomic_write_string(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp_path = path.with_file_name(format!("{}.tmp.{}", file_name, std::process::id()));
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, path)
}

pub fn clean_ansi_and_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if ('@'..='~').contains(&nc) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }

    // Extract inside <USER_REQUEST> if present
    if let (Some(start), Some(end)) = (out.find("<USER_REQUEST>"), out.find("</USER_REQUEST>")) {
        if start + 14 <= end {
            return out[start + 14..end].trim().to_string();
        }
    }
    out.trim().to_string()
}

pub fn sanitize_code_path(requested: &str) -> PathBuf {
    let root = code_root_dir();
    let candidate = if requested.is_empty() || requested == "." {
        root.clone()
    } else {
        let p = Path::new(requested);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    };

    let canonical_root = fs::canonicalize(&root).unwrap_or(root.clone());
    let canonical_target = fs::canonicalize(&candidate).unwrap_or(candidate);

    if canonical_target.starts_with(&canonical_root) {
        canonical_target
    } else {
        canonical_root
    }
}

pub fn list_directory_contents(target_path: &str) -> FileTreeInfo {
    let root = fs::canonicalize(code_root_dir()).unwrap_or_else(|_| code_root_dir());
    let safe_path = sanitize_code_path(target_path);

    let rel_path = safe_path
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let can_navigate_up = safe_path != root && !rel_path.is_empty();
    let mut entries = Vec::new();

    if let Ok(read_dir) = fs::read_dir(&safe_path) {
        let mut raw_entries: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
        raw_entries.sort_by_key(|e| e.file_name());

        for entry in raw_entries {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && name != ".git" {
                continue;
            }
            let full = entry.path();
            let is_dir = full.is_dir();

            let is_project = if is_dir {
                [".git", "Cargo.toml", "package.json", "pyproject.toml", "go.mod"]
                    .iter()
                    .any(|m| full.join(m).exists())
            } else {
                false
            };

            let size_bytes = if !is_dir {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            } else {
                0
            };

            let entry_rel = full
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| name.clone());

            entries.push(FileEntry {
                name,
                rel_path: entry_rel,
                is_dir,
                is_project,
                size_bytes,
            });
        }
    }

    FileTreeInfo {
        root_dir: root.to_string_lossy().to_string(),
        current_rel_path: rel_path,
        can_navigate_up,
        entries,
    }
}

pub fn classify_file_kind(file_name: &str) -> String {
    let lower = file_name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "md" | "markdown" | "mdx" => "markdown".to_string(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => "image".to_string(),
        "json" | "jsonl" => "json".to_string(),
        "rs" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "css" | "html" | "htm" | "toml"
        | "yaml" | "yml" | "sh" | "zsh" | "bash" | "py" | "go" | "sql" | "c" | "cpp" | "h" => {
            "code".to_string()
        }
        _ => "text".to_string(),
    }
}

pub fn resolve_sandboxed_workspace_path(project_slug: &str, rel_path: &str) -> Option<PathBuf> {
    if rel_path.contains('\0') {
        return None;
    }
    let slug = validate_project_slug(project_slug).ok()?;
    let trimmed = rel_path.trim().trim_start_matches("file://");
    let rel_p = Path::new(trimmed);
    if rel_p.is_absolute() {
        return None;
    }
    for comp in rel_p.components() {
        if matches!(
            comp,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        ) {
            return None;
        }
    }

    let code_root = fs::canonicalize(code_root_dir()).ok()?;
    let proj_root = fs::canonicalize(code_root.join(&slug)).ok()?;
    if !proj_root.starts_with(&code_root) || !proj_root.is_dir() {
        return None;
    }

    if trimmed.is_empty() || trimmed == "." {
        return Some(proj_root);
    }

    let candidate = proj_root.join(rel_p);
    if fs::symlink_metadata(&candidate).is_ok() {
        let canon = fs::canonicalize(&candidate).ok()?;
        if canon.starts_with(&proj_root) {
            Some(canon)
        } else {
            None
        }
    } else {
        let parent = candidate.parent()?;
        let file_name = candidate.file_name()?;
        let canon_parent = fs::canonicalize(parent).ok()?;
        if canon_parent.starts_with(&proj_root) {
            Some(canon_parent.join(file_name))
        } else {
            None
        }
    }
}

pub fn resolve_sandboxed_file_path(
    raw_path: &str,
    active_session_id: &str,
    active_project: &str,
) -> Option<PathBuf> {
    if raw_path.contains('\0') {
        return None;
    }
    let trimmed = raw_path
        .trim()
        .trim_start_matches("file://")
        .split('#')
        .next()
        .unwrap_or("")
        .replace("%20", " ");
    if trimmed.is_empty() {
        return None;
    }

    let code_root = fs::canonicalize(code_root_dir()).unwrap_or_else(|_| code_root_dir());
    let brain_root = fs::canonicalize(brain_dir()).unwrap_or_else(|_| brain_dir());

    let is_allowed = |cand: &Path| -> Option<PathBuf> {
        let canon = fs::canonicalize(cand).ok()?;
        if canon.is_file() && (canon.starts_with(&code_root) || canon.starts_with(&brain_root)) {
            Some(canon)
        } else {
            None
        }
    };

    let p = Path::new(&trimmed);
    if p.is_absolute() {
        if let Some(found) = is_allowed(p) {
            return Some(found);
        }
    }

    // 1. Check relative to ~/code
    if let Some(found) = is_allowed(&code_root.join(&trimmed)) {
        return Some(found);
    }

    // 2. Check relative to active project inside ~/code/<active_project>
    if !active_project.is_empty() {
        if let Some(found) = is_allowed(&code_root.join(active_project).join(&trimmed)) {
            return Some(found);
        }
    }

    // 3. Check inside active session brain directory (~/.gemini/antigravity-cli/brain/<session_id>)
    if !active_session_id.is_empty() {
        let s_dir = brain_dir().join(active_session_id);
        if let Some(found) = is_allowed(&s_dir.join(&trimmed)) {
            return Some(found);
        }
        if !trimmed.ends_with(".md") {
            if let Some(found) = is_allowed(&s_dir.join(format!("{}.md", trimmed))) {
                return Some(found);
            }
        }
    }

    // 4. If bare filename, search top-level files of active project and session brain dir
    if !trimmed.contains('/') {
        if !active_session_id.is_empty() {
            for art in get_session_artifacts(active_session_id) {
                if art.name.eq_ignore_ascii_case(&trimmed)
                    || Path::new(&art.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().eq_ignore_ascii_case(&trimmed))
                        .unwrap_or(false)
                {
                    if let Some(found) = is_allowed(Path::new(&art.path)) {
                        return Some(found);
                    }
                }
            }
        }
    }

    None
}

pub fn read_bounded_file(
    rel_file_path: &str,
    active_session_id: &str,
    active_project: &str,
) -> Option<OpenedFile> {
    let code_root = fs::canonicalize(code_root_dir()).unwrap_or_else(|_| code_root_dir());
    let brain_root = fs::canonicalize(brain_dir()).unwrap_or_else(|_| brain_dir());
    let safe_path = resolve_sandboxed_file_path(rel_file_path, active_session_id, active_project)?;

    let name = safe_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    let rel_path = if let Ok(rel) = safe_path.strip_prefix(&code_root) {
        rel.to_string_lossy().to_string()
    } else if let Ok(rel) = safe_path.strip_prefix(&brain_root) {
        format!("~/.gemini/antigravity-cli/brain/{}", rel.to_string_lossy())
    } else {
        safe_path.to_string_lossy().to_string()
    };

    let abs_path = safe_path.to_string_lossy().to_string();
    let file_kind = classify_file_kind(&name);
    let size_bytes = fs::metadata(&safe_path).map(|m| m.len()).unwrap_or(0);

    let content = if file_kind == "image" {
        format!("/raw-file?path={}", abs_path)
    } else {
        match fs::read(&safe_path) {
            Ok(bytes) => {
                let slice = if bytes.len() > 250_000 {
                    &bytes[..250_000]
                } else {
                    &bytes[..]
                };
                String::from_utf8_lossy(slice).to_string()
            }
            Err(e) => format!("Error reading file: {}", e),
        }
    };

    Some(OpenedFile {
        name,
        rel_path,
        abs_path,
        file_kind,
        size_bytes,
        content,
    })
}

pub fn format_epoch_secs(secs: u64) -> String {
    // Simple UTC/local human-readable timestamp without heavy timezone crates
    let days = secs / 86400;
    let rem = secs % 86400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;

    // Civil date from days since 1970-01-01 (Howard Hinnant algorithm)
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, m, d, hours, mins)
}

pub fn extract_project_slug_from_text(text: &str) -> Option<String> {
    let mut slice = text;
    while let Some(pos) = slice.find("/code/") {
        let after = &slice[pos + "/code/".len()..];
        let slug: String = after
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_' || *ch == '.')
            .collect();
        if !slug.is_empty() && slug != "." && slug != ".." {
            return Some(slug);
        }
        slice = after;
    }
    None
}

pub fn infer_session_project_id(session_id: &str) -> String {
    if session_id.is_empty() || session_id.contains("..") || session_id.contains('/') {
        return "exo-suit".to_string();
    }
    let s_path = brain_dir().join(session_id);
    if let Ok(saved) = fs::read_to_string(s_path.join(".session_project.txt")) {
        if let Ok(valid) = validate_project_slug(saved.trim()) {
            return valid;
        }
    }

    let code_root = code_root_dir();
    for log_name in ["transcript.jsonl", "transcript_full.jsonl"] {
        let tp = s_path.join(".system_generated").join("logs").join(log_name);
        if let Ok(content) = fs::read_to_string(&tp) {
            for line in content.lines() {
                if let Some(slug) = extract_project_slug_from_text(line) {
                    if let Ok(valid) = validate_project_slug(&slug) {
                        if code_root.join(&valid).is_dir() {
                            let _ = save_session_project_id(session_id, &valid);
                            return valid;
                        }
                    }
                }
            }
        }
    }

    "exo-suit".to_string()
}

pub fn save_session_project_id(session_id: &str, project_id: &str) -> bool {
    if session_id.is_empty()
        || session_id.contains("..")
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains('\0')
    {
        return false;
    }
    let Ok(valid_slug) = validate_project_slug(project_id) else {
        return false;
    };
    let s_path = brain_dir().join(session_id);
    if !s_path.exists() {
        return false;
    }
    atomic_write_string(&s_path.join(".session_project.txt"), &valid_slug).is_ok()
}

pub fn resolve_execution_scope(session_id: &str, requested_project: &str) -> ExecutionScope {
    let clean_session = if session_id.trim().is_empty() || session_id.contains("..") {
        "default-session".to_string()
    } else {
        session_id.trim().to_string()
    };

    let candidate_proj = if !requested_project.trim().is_empty() {
        validate_project_slug(requested_project).unwrap_or_else(|_| "exo-suit".to_string())
    } else {
        infer_session_project_id(&clean_session)
    };

    let root = fs::canonicalize(code_root_dir()).unwrap_or_else(|_| code_root_dir());
    let safe_path = sanitize_code_path(&candidate_proj);
    let resolved_proj = if safe_path == root {
        // If requested_project escaped ~/code or was root, fall back to exo-suit if it exists
        let fallback = sanitize_code_path("exo-suit");
        if fallback != root {
            return ExecutionScope {
                session_id: clean_session,
                project_id: "exo-suit".to_string(),
                workspace_path: fallback,
            };
        }
        "exo-suit".to_string()
    } else {
        safe_path
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| candidate_proj)
    };

    ExecutionScope {
        session_id: clean_session,
        project_id: resolved_proj,
        workspace_path: safe_path,
    }
}

pub fn get_past_sessions(limit: usize) -> Vec<SessionItem> {
    let bdir = brain_dir();
    let code_root = code_root_dir();
    let mut sessions = Vec::new();
    let Ok(read_dir) = fs::read_dir(&bdir) else {
        return sessions;
    };

    let mut dirs: Vec<(String, PathBuf, u64)> = Vec::new();
    for entry in read_dir.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let p = entry.path();
        if p.is_dir() {
            // Check mtime of transcript.jsonl if available, otherwise dir mtime
            let transcript_p = p.join(".system_generated").join("logs").join("transcript.jsonl");
            let mtime = fs::metadata(&transcript_p)
                .or_else(|_| entry.metadata())
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            dirs.push((name, p, mtime));
        }
    }

    dirs.sort_by(|a, b| b.2.cmp(&a.2));

    for (s_id, s_path, mtime) in dirs.into_iter().take(limit) {
        let artifact_count = fs::read_dir(&s_path)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| {
                        let fname = e.file_name().to_string_lossy().to_string();
                        e.path().is_file()
                            && !fname.starts_with('.')
                            && !fname.ends_with(".metadata.json")
                    })
                    .count() as u32
            })
            .unwrap_or(0);

        let mut first_prompt = "Interactive Agent Session".to_string();
        let mut step_count = 0u32;
        let mut subagent_count = 0u32;

        let saved_project = fs::read_to_string(s_path.join(".session_project.txt"))
            .ok()
            .and_then(|s| validate_project_slug(s.trim()).ok());
        let mut inferred_project = saved_project.clone();

        let transcript_p = s_path
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");

        if let Ok(content) = fs::read_to_string(&transcript_p) {
            for line in content.lines() {
                step_count += 1;
                if line.contains("\"invoke_subagent\"") {
                    subagent_count += 1;
                }
                if inferred_project.is_none() {
                    if let Some(slug) = extract_project_slug_from_text(line) {
                        if let Ok(valid) = validate_project_slug(&slug) {
                            if code_root.join(&valid).is_dir() {
                                inferred_project = Some(valid);
                            }
                        }
                    }
                }
                if first_prompt == "Interactive Agent Session" && line.contains("\"USER_INPUT\"") {
                    if let Ok(val) = serde_json::from_str::<Value>(line) {
                        if val.get("type").and_then(|t| t.as_str()) == Some("USER_INPUT") {
                            if let Some(raw_c) = val.get("content").and_then(|c| c.as_str()) {
                                let cleaned = clean_ansi_and_tags(raw_c);
                                if !cleaned.is_empty() {
                                    first_prompt = cleaned.chars().take(64).collect();
                                }
                            }
                        }
                    }
                }
            }
        }

        if saved_project.is_none() {
            if let Some(ref slug) = inferred_project {
                let _ = save_session_project_id(&s_id, slug);
            }
        }

        let custom_title = fs::read_to_string(s_path.join(".session_title.txt"))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());

        let project_id = inferred_project.unwrap_or_else(|| "exo-suit".to_string());

        sessions.push(SessionItem {
            session_id: s_id,
            project_id,
            updated_at: format_epoch_secs(mtime),
            prompt_preview: first_prompt,
            custom_title,
            artifact_count,
            step_count,
            subagent_count,
        });
    }

    sessions
}

pub fn rename_session(session_id: &str, new_title: &str) -> bool {
    if session_id.is_empty() || session_id.contains("..") || session_id.contains('/') {
        return false;
    }
    let s_path = brain_dir().join(session_id);
    if !s_path.exists() {
        return false;
    }
    let title_file = s_path.join(".session_title.txt");
    let trimmed = new_title.trim();
    if trimmed.is_empty() {
        let _ = fs::remove_file(&title_file);
        true
    } else {
        let bounded: String = trimmed.chars().take(80).collect();
        atomic_write_string(&title_file, &bounded).is_ok()
    }
}

fn parse_skill_frontmatter(raw: &str, fallback_name: &str) -> (String, String, String) {
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut body = raw;

    if let Some(after_open) = raw.strip_prefix("---") {
        if let Some(end_idx) = after_open.find("\n---") {
            let fm = &after_open[..end_idx];
            body = after_open[end_idx + 4..].trim_start();
            let mut in_desc_multiline = false;
            for line in fm.lines() {
                let t = line.trim();
                if let Some(n) = t.strip_prefix("name:") {
                    name = n.trim().trim_matches('"').trim_matches('\'').to_string();
                    in_desc_multiline = false;
                } else if let Some(d) = t.strip_prefix("description:") {
                    let clean_d = d.trim().trim_matches('"').trim_matches('\'');
                    if clean_d == "|" || clean_d == ">" || clean_d.is_empty() {
                        in_desc_multiline = true;
                    } else {
                        description = clean_d.to_string();
                        in_desc_multiline = true;
                    }
                } else if in_desc_multiline && (line.starts_with("  ") || line.starts_with('\t')) {
                    if !t.is_empty() {
                        if !description.is_empty() {
                            description.push(' ');
                        }
                        description.push_str(t);
                    }
                } else if t.contains(':') {
                    in_desc_multiline = false;
                }
            }
        }
    }

    if description.is_empty() {
        description = body
            .lines()
            .find(|l| {
                let tr = l.trim();
                !tr.is_empty() && !tr.starts_with('#')
            })
            .unwrap_or("Specialized agent skill and workflow instructions.")
            .trim()
            .chars()
            .take(220)
            .collect();
    } else {
        description = description.chars().take(260).collect();
    }

    let preview: String = raw.chars().take(4200).collect();
    (name, description, preview)
}

fn scan_skills_dir(dir: &Path, source_label: &str, out: &mut Vec<SkillItem>, seen: &mut HashSet<String>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let skill_md = p.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let full_path = skill_md.to_string_lossy().to_string();
        if !seen.insert(full_path.clone()) {
            continue;
        }
        let folder_name = entry.file_name().to_string_lossy().to_string();
        let Ok(raw) = fs::read_to_string(&skill_md) else {
            continue;
        };
        let (name, description, content_preview) = parse_skill_frontmatter(&raw, &folder_name);
        out.push(SkillItem {
            id: format!("{}:{}", source_label, name),
            name,
            source: source_label.to_string(),
            path: full_path,
            description,
            content_preview,
        });
    }
}

pub fn get_installed_skills() -> Vec<SkillItem> {
    let home = home_dir();
    let gemini = home.join(".gemini");
    let mut skills = Vec::new();
    let mut seen = HashSet::new();

    scan_skills_dir(&gemini.join("skills"), "user-skills", &mut skills, &mut seen);
    scan_skills_dir(&gemini.join("config").join("skills"), "config-skills", &mut skills, &mut seen);
    scan_skills_dir(
        &gemini.join("antigravity-cli").join("skills"),
        "agy-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &gemini.join("antigravity-cli").join("builtin").join("skills"),
        "builtin-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".config").join("muse").join("skills"),
        "muse-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".agents").join("skills"),
        "agents-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".claude").join("skills"),
        "claude-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".codex").join("skills"),
        "codex-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".copilot").join("skills"),
        "copilot-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".cursor").join("skills"),
        "cursor-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".config").join("opencode").join("skills"),
        "opencode-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".config").join("goose").join("skills"),
        "goose-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".config").join("amp").join("skills"),
        "amp-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".factory").join("skills"),
        "droid-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".qwen").join("skills"),
        "qwen-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".continue").join("skills"),
        "continue-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".cline").join("skills"),
        "cline-skills",
        &mut skills,
        &mut seen,
    );
    scan_skills_dir(
        &home.join(".config").join("crush").join("skills"),
        "crush-skills",
        &mut skills,
        &mut seen,
    );

    // Also scan plugin skills under ~/.gemini/config/plugins/*/skills
    let plugins_dir = gemini.join("config").join("plugins");
    if let Ok(rd) = fs::read_dir(&plugins_dir) {
        let mut p_entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        p_entries.sort_by_key(|e| e.file_name());
        for pe in p_entries {
            let p_name = pe.file_name().to_string_lossy().to_string();
            let p_skills = pe.path().join("skills");
            if p_skills.is_dir() {
                scan_skills_dir(&p_skills, &p_name, &mut skills, &mut seen);
            }
        }
    }

    skills.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    skills
}

pub fn get_tool_catalog() -> Vec<ToolCatalogItem> {
    let mut tools = vec![
        ToolCatalogItem {
            name: "read_file".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "Read the contents of a text, image, pdf, video, or audio file (up to 800 lines or 46KB per read).".to_string(),
            parameters_summary: "Path (required), StartLine, EndLine, ByteOffset".to_string(),
            schema_preview: "{\n  \"name\": \"read_file\",\n  \"required\": [\"Path\"],\n  \"properties\": {\n    \"Path\": { \"type\": \"STRING\", \"description\": \"Absolute path of the file to read\" },\n    \"StartLine\": { \"type\": \"INTEGER\" },\n    \"EndLine\": { \"type\": \"INTEGER\" },\n    \"ByteOffset\": { \"type\": \"INTEGER\" }\n  }\n}".to_string(),
        },
        ToolCatalogItem {
            name: "edit_file".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "Modify an existing file with exact contiguous target-to-replacement matching.".to_string(),
            parameters_summary: "Path (required), Target (required), Replacement (required), ArtifactMetadata".to_string(),
            schema_preview: "{\n  \"name\": \"edit_file\",\n  \"required\": [\"Path\", \"Target\", \"Replacement\"],\n  \"properties\": {\n    \"Path\": { \"type\": \"STRING\" },\n    \"Target\": { \"type\": \"STRING\" },\n    \"Replacement\": { \"type\": \"STRING\" }\n  }\n}".to_string(),
        },
        ToolCatalogItem {
            name: "write_file".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "Create a new file or overwrite/append to an existing file or artifact.".to_string(),
            parameters_summary: "Path (required), Content (required), Overwrite, Append, ArtifactMetadata".to_string(),
            schema_preview: "{\n  \"name\": \"write_file\",\n  \"required\": [\"Path\", \"Content\"],\n  \"properties\": {\n    \"Path\": { \"type\": \"STRING\" },\n    \"Content\": { \"type\": \"STRING\" },\n    \"Overwrite\": { \"type\": \"BOOLEAN\" },\n    \"Append\": { \"type\": \"BOOLEAN\" }\n  }\n}".to_string(),
        },
        ToolCatalogItem {
            name: "run_command".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "Execute a command in the workspace zsh shell synchronously or as a background daemon task.".to_string(),
            parameters_summary: "CommandLine (required), WaitMsBeforeAsync (required), toolSummary (required), IsDaemon".to_string(),
            schema_preview: "{\n  \"name\": \"run_command\",\n  \"required\": [\"CommandLine\", \"WaitMsBeforeAsync\", \"toolSummary\"],\n  \"properties\": {\n    \"CommandLine\": { \"type\": \"STRING\" },\n    \"WaitMsBeforeAsync\": { \"type\": \"INTEGER\" },\n    \"toolSummary\": { \"type\": \"STRING\" },\n    \"IsDaemon\": { \"type\": \"BOOLEAN\" }\n  }\n}".to_string(),
        },
        ToolCatalogItem {
            name: "invoke_subagent".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "Launch an autonomous subagent in its own conversation context with isolated or shared workspace.".to_string(),
            parameters_summary: "TypeName (required), Role (required), Prompt (required), Model, Workspace".to_string(),
            schema_preview: "{\n  \"name\": \"invoke_subagent\",\n  \"required\": [\"TypeName\", \"Role\", \"Prompt\"],\n  \"properties\": {\n    \"TypeName\": { \"type\": \"STRING\" },\n    \"Role\": { \"type\": \"STRING\" },\n    \"Prompt\": { \"type\": \"STRING\" },\n    \"Model\": { \"enum\": [\"inherit\", \"flash_lite\", \"flash\", \"pro\"] }\n  }\n}".to_string(),
        },
        ToolCatalogItem {
            name: "define_subagent".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "Define a specialized subagent archetype with custom system prompt and tool permissions.".to_string(),
            parameters_summary: "Name (required), Description (required), SystemPrompt (required), EnableWriteTools, EnableMCPTools".to_string(),
            schema_preview: "{\n  \"name\": \"define_subagent\",\n  \"required\": [\"Name\", \"Description\", \"SystemPrompt\"]\n}".to_string(),
        },
        ToolCatalogItem {
            name: "manage_task".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "List, inspect status/logs, send input to, or terminate background tasks.".to_string(),
            parameters_summary: "Action (list | status | send_input | kill), TaskId, Input".to_string(),
            schema_preview: "{\n  \"name\": \"manage_task\",\n  \"required\": [\"Action\"],\n  \"properties\": {\n    \"Action\": { \"enum\": [\"list\", \"kill\", \"status\", \"send_input\"] },\n    \"TaskId\": { \"type\": \"STRING\" }\n  }\n}".to_string(),
        },
        ToolCatalogItem {
            name: "call_mcp_tool".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "Invoke any lazy-loaded MCP server tool after inspecting its JSON schema.".to_string(),
            parameters_summary: "ServerName (required), ToolName (required), Arguments (required)".to_string(),
            schema_preview: "{\n  \"name\": \"call_mcp_tool\",\n  \"required\": [\"ServerName\", \"ToolName\", \"Arguments\"]\n}".to_string(),
        },
        ToolCatalogItem {
            name: "search_web".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "Perform a web search for a query and return a synthesized summary with URL citations.".to_string(),
            parameters_summary: "query (required), domain (optional)".to_string(),
            schema_preview: "{\n  \"name\": \"search_web\",\n  \"required\": [\"query\"]\n}".to_string(),
        },
        ToolCatalogItem {
            name: "ask_question".to_string(),
            server: "native-harness".to_string(),
            mode: "EAGER".to_string(),
            description: "Present interactive single-select or multi-select clarification questions directly to the user in the harness.".to_string(),
            parameters_summary: "questions (required: array of {question, options, is_multi_select})".to_string(),
            schema_preview: "{\n  \"name\": \"ask_question\",\n  \"required\": [\"questions\"],\n  \"properties\": {\n    \"questions\": {\n      \"type\": \"ARRAY\",\n      \"items\": {\n        \"properties\": {\n          \"question\": { \"type\": \"STRING\" },\n          \"options\": { \"type\": \"ARRAY\", \"items\": { \"type\": \"STRING\" } },\n          \"is_multi_select\": { \"type\": \"BOOLEAN\" }\n        }\n      }\n    }\n  }\n}".to_string(),
        },
    ];

    // Dynamically scan ~/.gemini/antigravity-cli/mcp/<server>/*.json for MCP tool schemas
    let mcp_root = home_dir().join(".gemini").join("antigravity-cli").join("mcp");
    if let Ok(rd) = fs::read_dir(&mcp_root) {
        let mut servers: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        servers.sort_by_key(|e| e.file_name());
        for s_entry in servers {
            let s_path = s_entry.path();
            if !s_path.is_dir() {
                continue;
            }
            let server_name = s_entry.file_name().to_string_lossy().to_string();
            let Ok(files) = fs::read_dir(&s_path) else {
                continue;
            };
            let mut f_entries: Vec<_> = files.filter_map(|e| e.ok()).collect();
            f_entries.sort_by_key(|e| e.file_name());

            for f_entry in f_entries {
                let fp = f_entry.path();
                let fname = f_entry.file_name().to_string_lossy().to_string();
                if !fname.ends_with(".json") {
                    continue;
                }
                let tool_name = fname.trim_end_matches(".json").to_string();
                let Ok(raw_json) = fs::read_to_string(&fp) else {
                    continue;
                };
                let mut desc = format!("MCP tool `{}` on server `{}`", tool_name, server_name);
                let mut params_vec = Vec::new();

                if let Ok(val) = serde_json::from_str::<Value>(&raw_json) {
                    if let Some(d) = val.get("description").and_then(|v| v.as_str()) {
                        desc = d.trim().chars().take(240).collect();
                    }
                    let schema_obj = val
                        .get("parameters")
                        .or_else(|| val.get("inputSchema"))
                        .unwrap_or(&val);
                    let req_set: HashSet<String> = schema_obj
                        .get("required")
                        .and_then(|r| r.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();

                    if let Some(props) = schema_obj.get("properties").and_then(|p| p.as_object()) {
                        for k in props.keys() {
                            if req_set.contains(k) {
                                params_vec.push(format!("{}*", k));
                            } else {
                                params_vec.push(k.clone());
                            }
                        }
                    }
                }

                let parameters_summary = if params_vec.is_empty() {
                    "No parameters".to_string()
                } else {
                    params_vec.join(", ")
                };

                tools.push(ToolCatalogItem {
                    name: tool_name,
                    server: server_name.clone(),
                    mode: "LAZY".to_string(),
                    description: desc,
                    parameters_summary,
                    schema_preview: raw_json.chars().take(3200).collect(),
                });
            }
        }
    }

    tools
}

pub fn get_session_artifacts(session_id: &str) -> Vec<ArtifactItem> {
    let mut artifacts = Vec::new();
    let s_path = brain_dir().join(session_id);
    if !s_path.exists() {
        return artifacts;
    }

    if let Ok(rd) = fs::read_dir(&s_path) {
        for entry in rd.filter_map(|e| e.ok()) {
            let full_f = entry.path();
            let fname = entry.file_name().to_string_lossy().to_string();
            if full_f.is_file() && !fname.starts_with('.') && !fname.ends_with(".metadata.json") {
                let meta_f = s_path.join(format!("{}.metadata.json", fname));
                let mut summary = format!("Artifact file {}", fname);
                let mut request_feedback = false;
                let mut user_facing = true;
                let mut approved = false;

                if let Ok(meta_str) = fs::read_to_string(&meta_f) {
                    if let Ok(mdata) = serde_json::from_str::<Value>(&meta_str) {
                        if let Some(s) = mdata.get("summary").and_then(|v| v.as_str()) {
                            summary = s.to_string();
                        }
                        request_feedback = mdata
                            .get("requestFeedback")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        user_facing = mdata
                            .get("userFacing")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        approved = mdata
                            .get("approved")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                    }
                }

                let preview = fs::read_to_string(&full_f)
                    .map(|s| s.chars().take(64_000).collect::<String>())
                    .unwrap_or_else(|_| "[Binary or non-UTF8 artifact]".to_string());

                artifacts.push(ArtifactItem {
                    name: fname,
                    path: full_f.to_string_lossy().to_string(),
                    summary,
                    request_feedback,
                    user_facing,
                    approved,
                    content_preview: preview,
                });
            }
        }
    }

    artifacts
}

pub fn approve_artifact_metadata(session_id: &str, artifact_name: &str) -> bool {
    let meta_p = brain_dir()
        .join(session_id)
        .join(format!("{}.metadata.json", artifact_name));
    if let Ok(content) = fs::read_to_string(&meta_p) {
        if let Ok(mut val) = serde_json::from_str::<Value>(&content) {
            if let Some(obj) = val.as_object_mut() {
                obj.insert("approved".to_string(), Value::Bool(true));
                obj.insert("requestFeedback".to_string(), Value::Bool(false));
                if let Ok(serialized) = serde_json::to_string_pretty(&val) {
                    return atomic_write_string(&meta_p, &serialized).is_ok();
                }
            }
        }
    }
    false
}

pub fn load_session_interactive_answers(session_id: &str) -> HashMap<String, Vec<String>> {
    if session_id.is_empty() || session_id.contains("..") || session_id.contains('/') {
        return HashMap::new();
    }
    let p = brain_dir().join(session_id).join(".interactive_answers.json");
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, Vec<String>>>(&s).ok())
        .unwrap_or_default()
}

pub fn save_session_interactive_answer(
    session_id: &str,
    prompt_id: &str,
    answers: &[String],
) -> bool {
    if session_id.is_empty() || session_id.contains("..") || session_id.contains('/') {
        return false;
    }
    let s_dir = brain_dir().join(session_id);
    if !s_dir.exists() {
        return false;
    }
    let mut map = load_session_interactive_answers(session_id);
    map.insert(prompt_id.to_string(), answers.to_vec());
    let p = s_dir.join(".interactive_answers.json");
    if let Ok(serialized) = serde_json::to_string_pretty(&map) {
        return atomic_write_string(&p, &serialized).is_ok();
    }
    false
}

pub fn reconcile_interactive_prompt(
    step_index: u32,
    questions: Vec<InteractiveQuestionItem>,
    subsequent_steps: &[Value],
    saved_answers: &HashMap<String, Vec<String>>,
) -> InteractivePrompt {
    let prompt_id = format!("ask-step-{}", step_index);
    let mut cli_answers = Vec::new();

    for next_val in subsequent_steps.iter().take(6) {
        let next_type = next_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if next_type == "GENERIC" {
            if let Some(nc) = next_val.get("content").and_then(|v| v.as_str()) {
                let extracted = extract_generic_step_answers(nc);
                if !extracted.is_empty() {
                    cli_answers = extracted;
                    break;
                }
            }
        } else if next_type == "PLANNER_RESPONSE" || next_type == "USER_INPUT" {
            break;
        }
    }

    let final_answers = if !cli_answers.is_empty() {
        cli_answers
    } else if let Some(saved) = saved_answers.get(&prompt_id) {
        saved.clone()
    } else {
        Vec::new()
    };

    let has_subsequent_user_input = subsequent_steps.iter().any(|v| {
        if v.get("type").and_then(|t| t.as_str()) != Some("USER_INPUT") {
            return false;
        }
        if let Some(c) = v.get("content").and_then(|c| c.as_str()) {
            if c.contains("<SYSTEM_MESSAGE>") && !c.contains("<USER_REQUEST>") {
                return false;
            }
        }
        true
    });

    let is_answered = !final_answers.is_empty() || has_subsequent_user_input;
    InteractivePrompt {
        prompt_id,
        step_index,
        status: if is_answered {
            "Answered".to_string()
        } else {
            "Pending".to_string()
        },
        questions,
        answers: final_answers,
    }
}

pub fn parse_ask_question_args(args: Option<&Value>) -> Vec<InteractiveQuestionItem> {
    let Some(args_obj) = args else {
        return Vec::new();
    };
    let Some(q_val) = args_obj.get("questions") else {
        return Vec::new();
    };

    let parsed_arr: Option<Vec<Value>> = match q_val {
        Value::Array(arr) => Some(arr.clone()),
        Value::String(s) => {
            let trimmed = s.trim();
            serde_json::from_str::<Vec<Value>>(trimmed)
                .ok()
                .or_else(|| {
                    // Sometimes double-encoded or wrapped in quotes
                    serde_json::from_str::<String>(trimmed)
                        .ok()
                        .and_then(|inner| serde_json::from_str::<Vec<Value>>(&inner).ok())
                })
        }
        _ => None,
    };

    let Some(arr) = parsed_arr else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for (idx, item) in arr.into_iter().enumerate() {
        let question = item
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if question.is_empty() {
            continue;
        }
        let is_multi_select = item
            .get("is_multi_select")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let options = item
            .get("options")
            .and_then(|v| v.as_array())
            .map(|opts| {
                opts.iter()
                    .filter_map(|o| o.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();

        items.push(InteractiveQuestionItem {
            index: idx + 1,
            question,
            options,
            is_multi_select,
        });
    }
    items
}

fn extract_generic_step_answers(content: &str) -> Vec<String> {
    let mut answers = Vec::new();
    let mut past_header = false;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("Created At:") || t.starts_with("Completed At:") {
            past_header = true;
            continue;
        }
        if t.starts_with("A1:")
            || t.starts_with("A2:")
            || t.starts_with("A3:")
            || t.starts_with("A4:")
            || t.starts_with("A5:")
            || (past_header && !t.starts_with("Showing lines") && !t.starts_with("Command exited"))
        {
            answers.push(t.to_string());
        }
    }
    answers
}

fn load_transcript_values(session_id: &str) -> Vec<Value> {
    let logs_dir = brain_dir()
        .join(session_id)
        .join(".system_generated")
        .join("logs");
    let transcript_p = logs_dir.join("transcript.jsonl");
    let Ok(raw) = fs::read_to_string(&transcript_p) else {
        return Vec::new();
    };

    let raw_lines: Vec<&str> = raw.lines().collect();
    let needs_full = raw_lines.iter().any(|l| {
        l.contains("\"truncated_fields\"")
            && (l.contains("\"ask_question\"")
                || l.contains("Tool is running as a background task")
                || l.contains("Created the following subagent")
                || l.contains("background task(s) currently running")
                || l.contains("\"invoke_subagent\"")
                || l.contains("\"run_command\""))
    });

    let full_lines: Vec<String> = if needs_full {
        fs::read_to_string(logs_dir.join("transcript_full.jsonl"))
            .map(|s| s.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut out = Vec::with_capacity(raw_lines.len());
    for (idx, line) in raw_lines.into_iter().enumerate() {
        let use_line = if line.contains("\"truncated_fields\"")
            && (line.contains("\"ask_question\"")
                || line.contains("Tool is running as a background task")
                || line.contains("Created the following subagent")
                || line.contains("background task(s) currently running")
                || line.contains("\"invoke_subagent\"")
                || line.contains("\"run_command\""))
            && idx < full_lines.len()
        {
            full_lines[idx].as_str()
        } else {
            line
        };
        if let Ok(val) = serde_json::from_str::<Value>(use_line) {
            out.push(val);
        }
    }
    out
}

/// Parses a session's `transcript.jsonl` into a chronological conversation thread of user and
/// assistant turns, folding intermediate tool-call / thinking steps into cohesive assistant replies
/// and surfacing interactive CLI questions (`ask_question`) as structured interactive prompts.
pub fn get_session_chat_thread(session_id: &str) -> Vec<ChatMessage> {
    let mut thread: Vec<ChatMessage> = Vec::new();
    let steps = load_transcript_values(session_id);
    if steps.is_empty() {
        return thread;
    }

    let saved_answers = load_session_interactive_answers(session_id);

    let mut pending_thinking: Option<String> = None;
    let mut pending_tools: Vec<ToolCall> = Vec::new();
    let mut pending_content = String::new();
    let mut pending_step: u32 = 0;
    let mut pending_ts = String::new();
    let mut turn_anchor_step: u32 = 0;
    let mut has_open_turn = false;

    for (idx, val) in steps.iter().enumerate() {
        let step_index = val
            .get("step_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let step_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let created_at = val
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(19)
            .collect::<String>()
            .replace('T', " ");

        if step_type == "USER_INPUT" {
            let Some(raw_c) = val.get("content").and_then(|v| v.as_str()) else {
                continue;
            };
            // Skip pure system/task wakeup messages that don't contain a user request
            if raw_c.contains("<SYSTEM_MESSAGE>") && !raw_c.contains("<USER_REQUEST>") {
                continue;
            }
            let cleaned = clean_ansi_and_tags(raw_c);
            if cleaned.is_empty() || cleaned.starts_with("<CONTEXT_SUMMARY>") {
                continue;
            }

            // Flush any previous assistant turn before starting a new user turn
            if !pending_content.is_empty() || pending_thinking.is_some() || !pending_tools.is_empty()
            {
                if pending_tools.len() > 45 {
                    pending_tools = pending_tools.split_off(pending_tools.len() - 45);
                }
                thread.push(ChatMessage {
                    id: format!("msg-turn-{}", turn_anchor_step),
                    step_index: pending_step,
                    role: "assistant".to_string(),
                    sender: "Antigravity".to_string(),
                    provider: "antigravity".to_string(),
                    model: "gemini-4-argon".to_string(),
                    timestamp: pending_ts.clone(),
                    content: std::mem::take(&mut pending_content),
                    thinking: pending_thinking.take(),
                    tool_calls: std::mem::take(&mut pending_tools),
                    streaming: false,
                    interactive_prompt: None,
                });
            }

            thread.push(ChatMessage {
                id: format!("msg-{}", step_index),
                step_index,
                role: "user".to_string(),
                sender: "You".to_string(),
                provider: "user".to_string(),
                model: String::new(),
                timestamp: created_at.clone(),
                content: cleaned,
                thinking: None,
                tool_calls: Vec::new(),
                streaming: false,
                interactive_prompt: None,
            });

            turn_anchor_step = step_index + 1;
            pending_step = step_index + 1;
            pending_ts = created_at;
            has_open_turn = true;
        } else if step_type == "PLANNER_RESPONSE" {
            if turn_anchor_step == 0 {
                turn_anchor_step = step_index;
            }
            pending_step = step_index;
            has_open_turn = true;
            if !created_at.is_empty() {
                pending_ts = created_at.clone();
            }

            if let Some(th) = val.get("thinking").and_then(|v| v.as_str()) {
                let t = th.trim();
                if !t.is_empty() {
                    pending_thinking = Some(t.chars().take(900).collect());
                }
            }

            if let Some(c) = val.get("content").and_then(|v| v.as_str()) {
                let trimmed = c.trim();
                if !trimmed.is_empty() {
                    pending_content = trimmed.chars().take(8000).collect();
                }
            }

            let mut step_interactive_questions: Vec<InteractiveQuestionItem> = Vec::new();

            if let Some(tcs) = val.get("tool_calls").and_then(|v| v.as_array()) {
                if !tcs.is_empty() {
                    for tc in tcs {
                        let name = tc
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        let args = tc.get("args");

                        if name == "ask_question" {
                            let parsed_qs = parse_ask_question_args(args);
                            if !parsed_qs.is_empty() {
                                step_interactive_questions = parsed_qs;
                            }
                        }

                        let raw_summary = if name == "ask_question" {
                            step_interactive_questions
                                .first()
                                .map(|q| q.question.as_str())
                                .unwrap_or("Interactive clarification question")
                        } else {
                            args.and_then(|a| {
                                a.get("toolSummary")
                                    .or_else(|| a.get("Path"))
                                    .or_else(|| a.get("CommandLine"))
                                    .or_else(|| a.get("Role"))
                                    .or_else(|| a.get("query"))
                            })
                            .and_then(|v| v.as_str())
                            .unwrap_or("Executing operation")
                            .trim_matches('"')
                        };

                        let code_root_prefix = format!("{}/", code_root_dir().to_string_lossy());
                        let exo_prefix = format!("{}exo-suit/", code_root_prefix);
                        let agy_prefix = format!(
                            "{}/.gemini/antigravity-cli/",
                            home_dir().to_string_lossy()
                        );
                        let clean_summary = raw_summary
                            .replace(&exo_prefix, "")
                            .replace(&code_root_prefix, "")
                            .replace(&agy_prefix, "~/.gemini/")
                            .chars()
                            .take(78)
                            .collect::<String>();

                        pending_tools.push(ToolCall {
                            name,
                            summary: clean_summary,
                            status: "Completed".to_string(),
                        });
                    }
                }
            }

            // If this PLANNER_RESPONSE presented interactive questions via `ask_question`,
            // surface it immediately as a dedicated interactive message card in the chat thread!
            if !step_interactive_questions.is_empty() {
                let reconciled = reconcile_interactive_prompt(
                    step_index,
                    step_interactive_questions,
                    &steps[idx + 1..],
                    &saved_answers,
                );

                if pending_tools.len() > 45 {
                    pending_tools = pending_tools.split_off(pending_tools.len() - 45);
                }

                thread.push(ChatMessage {
                    id: format!("msg-ask-{}", step_index),
                    step_index,
                    role: "assistant".to_string(),
                    sender: "Antigravity".to_string(),
                    provider: "antigravity".to_string(),
                    model: "gemini-4-argon".to_string(),
                    timestamp: pending_ts.clone(),
                    content: std::mem::take(&mut pending_content),
                    thinking: pending_thinking.take(),
                    tool_calls: std::mem::take(&mut pending_tools),
                    streaming: false,
                    interactive_prompt: Some(reconciled),
                });

                turn_anchor_step = step_index + 1;
                pending_step = step_index + 1;
                has_open_turn = false;
            }
        }
    }

    // Emit the final/active assistant turn at the end of the transcript
    if has_open_turn
        || !pending_content.is_empty()
        || pending_thinking.is_some()
        || !pending_tools.is_empty()
    {
        if pending_tools.len() > 45 {
            pending_tools = pending_tools.split_off(pending_tools.len() - 45);
        }
        // The turn is actively streaming/working only if final content hasn't arrived yet
        let is_streaming = pending_content.is_empty();
        if is_streaming {
            if let Some(last_tc) = pending_tools.last_mut() {
                last_tc.status = "Running".to_string();
            }
        }
        thread.push(ChatMessage {
            id: format!("msg-turn-{}", turn_anchor_step),
            step_index: pending_step,
            role: "assistant".to_string(),
            sender: "Antigravity".to_string(),
            provider: "antigravity".to_string(),
            model: "gemini-4-argon".to_string(),
            timestamp: pending_ts,
            content: pending_content,
            thinking: pending_thinking.take(),
            tool_calls: std::mem::take(&mut pending_tools),
            streaming: is_streaming,
            interactive_prompt: None,
        });
    }

    // Keep the most recent 60 messages to preserve the sub-20MB memory footprint
    if thread.len() > 60 {
        thread = thread.split_off(thread.len() - 60);
    }

    thread
}

/// Parses a real session's `transcript.jsonl` and recursively follows spawned `invoke_subagent`
/// conversationIds to build the live multi-agent execution tree.
pub fn build_live_session_graph(session_id: &str, active_project: &str) -> Vec<AgentNode> {
    let mut nodes = Vec::new();
    let mut visited = HashSet::new();

    parse_session_node_recursive(
        session_id,
        "",
        "Antigravity Lead Agent",
        active_project,
        460.0,
        140.0,
        0,
        &mut nodes,
        &mut visited,
    );

    if nodes.is_empty() {
        return fallback_project_graph(active_project);
    }

    nodes
}

#[derive(Clone, Debug)]
struct RunningTaskInfo {
    task_id: String,
    short_id: String,
    summary: String,
    command: String,
    is_daemon: bool,
    step_index: u32,
}

#[allow(clippy::too_many_arguments)]
fn parse_session_node_recursive(
    session_id: &str,
    parent_id: &str,
    default_role: &str,
    active_project: &str,
    x: f32,
    y: f32,
    depth: usize,
    nodes: &mut Vec<AgentNode>,
    visited: &mut HashSet<String>,
) {
    if depth > 3 || visited.contains(session_id) {
        return;
    }
    visited.insert(session_id.to_string());

    let steps = load_transcript_values(session_id);
    if steps.is_empty() {
        return;
    }

    let saved_answers = load_session_interactive_answers(session_id);

    let mut prompt = String::new();
    let mut thinking = String::new();
    let mut recent_log = String::new();
    let mut status = "Completed".to_string();
    let mut step_count = 0u32;
    let mut tokens = TokenMetrics::default();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut outputs: Vec<String> = Vec::new();
    let mut child_subagents: Vec<(String, String, String)> = Vec::new(); // (conv_id, role, prompt)
    let mut completed_subagents: HashSet<String> = HashSet::new();
    let mut pending_subagent_meta: Vec<(String, String)> = Vec::new();
    let mut pending_cmd_meta: Vec<(String, String, bool)> = Vec::new();
    let mut running_tasks: Vec<RunningTaskInfo> = Vec::new();
    let mut latest_interactive_prompt: Option<InteractivePrompt> = None;

    let total_lines = steps.len();

    for (idx, val) in steps.iter().enumerate() {
        step_count = val
            .get("step_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(step_count as u64 + 1) as u32;

        let step_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let step_status = val.get("status").and_then(|v| v.as_str()).unwrap_or("DONE");

        if let Some(in_tok) = val.get("input_tokens").and_then(|v| v.as_u64()) {
            tokens.input_tokens += in_tok;
        }
        if let Some(cache_tok) = val.get("cache_read_tokens").and_then(|v| v.as_u64()) {
            tokens.cache_read_tokens += cache_tok;
        }
        if let Some(out_tok) = val.get("output_tokens").and_then(|v| v.as_u64()) {
            tokens.output_tokens += out_tok;
        }

        if step_type == "USER_INPUT" {
            if let Some(c) = val.get("content").and_then(|v| v.as_str()) {
                let cleaned = clean_ansi_and_tags(c);
                if !cleaned.is_empty() {
                    prompt = cleaned;
                }
            }
        } else if step_type == "PLANNER_RESPONSE" {
            if let Some(th) = val.get("thinking").and_then(|v| v.as_str()) {
                let trimmed = th.trim();
                if !trimmed.is_empty() {
                    thinking = trimmed.chars().take(600).collect();
                }
            }

            if let Some(tcs) = val.get("tool_calls").and_then(|v| v.as_array()) {
                pending_cmd_meta.clear();
                for tc in tcs {
                    let name = tc
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool")
                        .to_string();
                    let args = tc.get("args");

                    if name == "invoke_subagent" {
                        let role_arg = args
                            .and_then(|a| a.get("Role"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("Subagent")
                            .trim_matches('"')
                            .to_string();
                        let prompt_arg = args
                            .and_then(|a| a.get("Prompt"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("Delegated subagent workflow")
                            .trim_matches('"')
                            .chars()
                            .take(240)
                            .collect::<String>();
                        pending_subagent_meta.push((role_arg, prompt_arg));
                    } else if name == "run_command" {
                        let sum_arg = args
                            .and_then(|a| a.get("toolSummary"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("Background Task")
                            .trim_matches('"')
                            .to_string();
                        let cmd_arg = args
                            .and_then(|a| a.get("CommandLine"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim_matches('"')
                            .to_string();
                        let is_daemon = args
                            .and_then(|a| a.get("IsDaemon"))
                            .and_then(|v| {
                                v.as_bool()
                                    .or_else(|| v.as_str().map(|s| s.trim_matches('"') == "true"))
                            })
                            .unwrap_or(false);
                        pending_cmd_meta.push((sum_arg, cmd_arg, is_daemon));
                    } else if name == "manage_task" {
                        let action = args
                            .and_then(|a| a.get("Action"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim_matches('"');
                        if action == "kill" {
                            if let Some(tid) = args
                                .and_then(|a| a.get("TaskId"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.trim_matches('"'))
                            {
                                running_tasks.retain(|rt| rt.task_id != tid && !tid.ends_with(&rt.short_id));
                            }
                        }
                    } else if name == "manage_subagents" {
                        let action = args
                            .and_then(|a| a.get("Action"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim_matches('"');
                        if action == "kill_all" {
                            for (cid, _, _) in &child_subagents {
                                completed_subagents.insert(cid.clone());
                            }
                        }
                    }

                    if name == "ask_question" {
                        let parsed_qs = parse_ask_question_args(args);
                        if !parsed_qs.is_empty() {
                            latest_interactive_prompt = Some(reconcile_interactive_prompt(
                                step_count,
                                parsed_qs,
                                &steps[idx + 1..],
                                &saved_answers,
                            ));
                        }
                    }

                    let summary = args
                        .and_then(|a| {
                            a.get("toolSummary")
                                .or_else(|| a.get("Path"))
                                .or_else(|| a.get("CommandLine"))
                                .or_else(|| a.get("Role"))
                        })
                        .and_then(|v| v.as_str())
                        .unwrap_or("Executed tool call")
                        .trim_matches('"')
                        .chars()
                        .take(80)
                        .collect::<String>();

                    recent_log = format!("{} → {}", name, summary);

                    let is_last = idx + 1 >= total_lines;
                    let tc_status = if is_last && step_status != "DONE" {
                        "Running".to_string()
                    } else {
                        "Completed".to_string()
                    };

                    tool_calls.push(ToolCall {
                        name: name.clone(),
                        summary: summary.clone(),
                        status: tc_status,
                    });
                }
            }

            if let Some(c) = val.get("content").and_then(|v| v.as_str()) {
                let trimmed = c.trim();
                if !trimmed.is_empty() {
                    recent_log = trimmed.lines().next().unwrap_or("").chars().take(90).collect();
                    outputs.push(trimmed.chars().take(2400).collect());
                }
            }
        } else if step_type == "GENERIC" {
            if let Some(c) = val.get("content").and_then(|v| v.as_str()) {
                // 1. Check if this step created subagents
                if c.contains("Created the following subagent:") && c.contains("\"conversationId\"") {
                    for cap_line in c.lines() {
                        if let Some(pos) = cap_line.find("\"conversationId\"") {
                            let rest = &cap_line[pos + 16..];
                            let conv_id: String = rest
                                .chars()
                                .filter(|ch| ch.is_ascii_hexdigit() || *ch == '-')
                                .collect();
                            if conv_id.len() >= 32 {
                                let (sub_role, sub_prompt) = if !pending_subagent_meta.is_empty() {
                                    pending_subagent_meta.remove(0)
                                } else {
                                    (
                                        format!("Subagent {}", child_subagents.len() + 1),
                                        "Delegated subagent workflow".to_string(),
                                    )
                                };
                                completed_subagents.remove(&conv_id);
                                if !child_subagents.iter().any(|(id, _, _)| id == &conv_id) {
                                    child_subagents.push((conv_id, sub_role, sub_prompt));
                                }
                            }
                        }
                    }
                }

                // 2. Check if this step launched one or more background tasks
                let marker = "Tool is running as a background task with task id:";
                let mut task_search = c;
                while let Some(pos) = task_search.find(marker) {
                    let after = &task_search[pos + marker.len()..];
                    task_search = after;
                    let task_id: String = after
                        .trim_start()
                        .chars()
                        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '/')
                        .collect();
                    if task_id.contains("/task-") {
                        let short_tid = task_id
                            .rsplit('/')
                            .next()
                            .unwrap_or(&task_id)
                            .to_string();
                        let (sum_meta, cmd_meta, is_daemon) = if !pending_cmd_meta.is_empty() {
                            pending_cmd_meta.remove(0)
                        } else {
                            let desc = c
                                .replace("\\n", "\n")
                                .lines()
                                .find_map(|l| l.strip_prefix("Task Description:"))
                                .unwrap_or("Background Command")
                                .trim()
                                .to_string();
                            let daemon_guess = desc.contains("exo-suit") || desc.contains("daemon");
                            (desc.clone(), desc, daemon_guess)
                        };
                        // Remove any duplicate task_id or superseded daemon with the same command/summary
                        running_tasks.retain(|rt| {
                            rt.task_id != task_id
                                && !(is_daemon
                                    && rt.is_daemon
                                    && (rt.command == cmd_meta || rt.summary == sum_meta))
                        });
                        running_tasks.push(RunningTaskInfo {
                            task_id,
                            short_id: short_tid,
                            summary: sum_meta,
                            command: cmd_meta,
                            is_daemon,
                            step_index: step_count,
                        });
                    }
                }

                // 3. Check if a task was cancelled, finished, or listed via manage_task output
                let unescaped_c = c.replace("\\n", "\n").replace("\\\"", "\"");
                if unescaped_c.contains("No background tasks are currently running") {
                    running_tasks.clear();
                } else if unescaped_c.contains("background task(s) currently running:") {
                    let mut active_ids = HashSet::new();
                    let mut search_slice = unescaped_c.as_str();
                    while let Some(p) = search_slice.find("\"taskId\":\"") {
                        let rest = &search_slice[p + "\"taskId\":\"".len()..];
                        let tid: String = rest
                            .chars()
                            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '/')
                            .collect();
                        if !tid.is_empty() {
                            active_ids.insert(tid);
                        }
                        search_slice = rest;
                    }
                    if !active_ids.is_empty() {
                        running_tasks.retain(|rt| active_ids.contains(&rt.task_id));
                    }
                }
                if unescaped_c.contains("Status: DONE") || unescaped_c.contains("cancelled.") {
                    for line in unescaped_c.lines() {
                        if let Some(tid) = line.strip_prefix("Task: ") {
                            let clean_tid = tid.trim();
                            running_tasks.retain(|rt| rt.task_id != clean_tid);
                        } else if line.contains("Task \"") && line.contains("\" cancelled") {
                            let parts: Vec<&str> = line.split('"').collect();
                            if parts.len() >= 2 {
                                let clean_tid = parts[1];
                                running_tasks.retain(|rt| rt.task_id != clean_tid);
                            }
                        }
                    }
                }
            }
        } else if step_type == "SYSTEM_MESSAGE" {
            if let Some(c) = val.get("content").and_then(|v| v.as_str()) {
                // Check for task completion or subagent completion notifications:
                // e.g. `sender=eae8f37c-.../task-922 ... Task id "..." finished`
                // or `sender=a0538fe0-77e1-4dda-b0c7-5ac816b80550`
                for token in c.split_whitespace() {
                    if let Some(sender) = token.strip_prefix("sender=") {
                        let clean_sender = sender.trim_matches(|ch: char| {
                            !ch.is_ascii_alphanumeric() && ch != '-' && ch != '/'
                        });
                        if clean_sender.contains("/task-") {
                            running_tasks.retain(|rt| rt.task_id != clean_sender);
                        } else if clean_sender.len() >= 32 {
                            completed_subagents.insert(clean_sender.to_string());
                        }
                    }
                }
            }
        }
    }

    // Determine live status from latest step, active tasks, or pending interactive question
    let latest_step_has_tools = steps
        .last()
        .and_then(|v| v.get("tool_calls"))
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);

    if let Some(ref ip) = latest_interactive_prompt {
        if ip.status == "Pending" {
            status = "AwaitingUserInput".to_string();
            let q_len = ip.questions.len();
            recent_log = format!(
                "❓ Awaiting user input ({} question{})",
                q_len,
                if q_len == 1 { "" } else { "s" }
            );
        }
    } else if latest_step_has_tools || (!tool_calls.is_empty() && outputs.is_empty()) {
        status = "Running".to_string();
    }
    if recent_log.is_empty() {
        recent_log = if !prompt.is_empty() {
            format!("Prompt: {}", prompt.chars().take(60).collect::<String>())
        } else {
            "Session ready".to_string()
        };
    }

    let short_id = if session_id.len() > 8 {
        format!("agy-{}", &session_id[..8])
    } else {
        session_id.to_string()
    };

    // Keep last 12 tool calls & last 5 outputs for minimal memory footprint
    if tool_calls.len() > 12 {
        tool_calls = tool_calls.split_off(tool_calls.len() - 12);
    }
    if outputs.len() > 5 {
        outputs = outputs.split_off(outputs.len() - 5);
    }

    nodes.push(AgentNode {
        agent_id: short_id.clone(),
        parent_id: parent_id.to_string(),
        node_kind: if depth == 0 {
            "agent".to_string()
        } else {
            "subagent".to_string()
        },
        provider: "antigravity".to_string(),
        model: "gemini-4-argon".to_string(),
        role: default_role.to_string(),
        status,
        execution_step: step_count,
        prompt: if prompt.is_empty() {
            format!("Workspace orchestration in ~/code/{}", active_project)
        } else {
            prompt
        },
        thinking,
        context: format!(
            "Session: {}\nWorkspace: ~/code/{}\nTokens: {} in / {} cached / {} out",
            session_id,
            active_project,
            tokens.input_tokens,
            tokens.cache_read_tokens,
            tokens.output_tokens
        ),
        recent_log,
        tokens,
        tool_calls: tool_calls.clone(),
        outputs,
        x,
        y,
        council_score: None,
        is_council_winner: false,
        decision_eval: None,
        interactive_prompt: latest_interactive_prompt,
    });

    // Filter to ONLY currently running subagents (subagents that have not yet returned a completion message)
    let active_subagents: Vec<(String, String, String)> = child_subagents
        .into_iter()
        .filter(|(sub_id, _, _)| {
            if completed_subagents.contains(sub_id) {
                return false;
            }
            // Also verify from the subagent's own transcript whether it already sent its completion message back
            let sub_steps = load_transcript_values(sub_id);
            let sent_back = sub_steps.iter().any(|sv| {
                sv.get("tool_calls")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter().any(|tc| {
                            tc.get("name").and_then(|n| n.as_str()) == Some("send_message")
                        })
                    })
                    .unwrap_or(false)
            });
            !sent_back
        })
        .collect();

    if !active_subagents.is_empty() {
        let count = active_subagents.len() as f32;
        let spacing = 260.0;
        let start_x = x - ((count - 1.0) * spacing) / 2.0;

        for (i, (sub_id, sub_role, _sub_prompt)) in active_subagents.into_iter().enumerate() {
            let child_x = start_x + (i as f32) * spacing;
            let child_y = y + 195.0;
            parse_session_node_recursive(
                &sub_id,
                &short_id,
                &sub_role,
                active_project,
                child_x,
                child_y,
                depth + 1,
                nodes,
                visited,
            );
        }
    }

    // Represent currently running tasks (background commands, daemons, or active tool executions)
    // as visually distinct Task nodes (`node_kind: "task"`) on the graph!
    if depth == 0 {
        let mut task_items: Vec<RunningTaskInfo> = running_tasks;

        // If the latest step in the transcript is actively executing a synchronous tool call,
        // also surface it as a live transient task capsule while it runs
        if let Some(last_step) = steps.last() {
            let l_type = last_step.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let l_status = last_step.get("status").and_then(|v| v.as_str()).unwrap_or("DONE");
            if l_type == "PLANNER_RESPONSE" && l_status != "DONE" {
                if let Some(tcs) = last_step.get("tool_calls").and_then(|v| v.as_array()) {
                    for (t_idx, tc) in tcs.iter().enumerate() {
                        let t_name = tc
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        if t_name != "invoke_subagent" && t_name != "ask_question" {
                            let args = tc.get("args");
                            let t_sum = args
                                .and_then(|a| {
                                    a.get("toolSummary")
                                        .or_else(|| a.get("Path"))
                                        .or_else(|| a.get("CommandLine"))
                                })
                                .and_then(|v| v.as_str())
                                .unwrap_or("Executing tool")
                                .trim_matches('"')
                                .to_string();
                            task_items.push(RunningTaskInfo {
                                task_id: format!("{}/live-tool-{}", session_id, t_idx + 1),
                                short_id: format!("tool-{}", t_name),
                                summary: format!("{}: {}", t_name, t_sum),
                                command: t_sum,
                                is_daemon: false,
                                step_index: step_count,
                            });
                        }
                    }
                }
            }
        }

        if !task_items.is_empty() {
            let count = task_items.len() as f32;
            let spacing = 245.0;
            let start_x = x - ((count - 1.0) * spacing) / 2.0;
            let task_y = y + 185.0;

            for (i, rt) in task_items.into_iter().enumerate() {
                let task_node_id = format!("{}-{}", short_id, rt.short_id);
                let kind_badge = if rt.is_daemon { "DAEMON" } else { "TASK" };
                nodes.push(AgentNode {
                    agent_id: task_node_id,
                    parent_id: short_id.clone(),
                    node_kind: "task".to_string(),
                    provider: "antigravity".to_string(),
                    model: format!("{} • {}", kind_badge, rt.short_id),
                    role: rt.summary.chars().take(34).collect(),
                    status: "Running".to_string(),
                    execution_step: rt.step_index,
                    prompt: rt.command.clone(),
                    thinking: if rt.is_daemon {
                        "Long-running background daemon process monitored by Trellis harness."
                            .to_string()
                    } else {
                        "Active background task executing in workspace shell.".to_string()
                    },
                    context: format!(
                        "Task ID: {}\nMode: {}\nCommand: {}",
                        rt.task_id,
                        if rt.is_daemon {
                            "Background Daemon (IsDaemon=true)"
                        } else {
                            "Asynchronous Task"
                        },
                        rt.command
                    ),
                    recent_log: rt.command.chars().take(80).collect(),
                    tokens: TokenMetrics::default(),
                    tool_calls: vec![ToolCall {
                        name: if rt.is_daemon {
                            "daemon_process".to_string()
                        } else {
                            "run_command".to_string()
                        },
                        summary: rt.summary,
                        status: "Running".to_string(),
                    }],
                    outputs: vec![],
                    x: start_x + (i as f32) * spacing,
                    y: task_y,
                    council_score: None,
                    is_council_winner: false,
                    decision_eval: None,
                    interactive_prompt: None,
                });
            }
        }
    }
}

pub fn fallback_project_graph(project_name: &str) -> Vec<AgentNode> {
    let p_clean = Path::new(project_name)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "exo-suit".to_string());

    vec![
        AgentNode {
            agent_id: format!("{}-orchestrator", p_clean),
            parent_id: String::new(),
            node_kind: "agent".to_string(),
            provider: "antigravity".to_string(),
            model: "gemini-4-argon".to_string(),
            role: format!("{} Lead Orchestrator", p_clean),
            status: "Running".to_string(),
            execution_step: 12,
            prompt: format!("Orchestrate multi-provider workflow for {}", p_clean),
            thinking: "Evaluating workspace dependency graph and subagent readiness.".to_string(),
            context: format!("Workspace: ~/code/{}", p_clean),
            recent_log: format!("Monitoring ~/code/{}...", p_clean),
            tokens: TokenMetrics {
                input_tokens: 14200,
                cache_read_tokens: 9800,
                output_tokens: 640,
            },
            tool_calls: vec![ToolCall {
                name: "list_dir".to_string(),
                summary: format!("Scanned ~/code/{}", p_clean),
                status: "Completed".to_string(),
            }],
            outputs: vec![format!("Workspace {} ready", p_clean)],
            x: 460.0,
            y: 140.0,
            council_score: None,
            is_council_winner: false,
            decision_eval: None,
            interactive_prompt: None,
        },
        AgentNode {
            agent_id: format!("{}-jev-gate", p_clean),
            parent_id: format!("{}-orchestrator", p_clean),
            node_kind: "gate".to_string(),
            provider: "jev".to_string(),
            model: "jev-latest".to_string(),
            role: "🎯 Jev System-1 Gate".to_string(),
            status: "DecisionGate".to_string(),
            execution_step: 4,
            prompt: format!("Evaluate System-1 decision primitives (Noul, Choice, Score) for {}", p_clean),
            thinking: "Sub-200ms TypeSafe AI System-1 evaluation over quantized state.".to_string(),
            context: format!("Model Class: DECISION_SYSTEM_1\nSLA: <200ms • Fail-Closed Safety\nWorkspace: ~/code/{}", p_clean),
            recent_log: "NOUL: 0.92 • CHOICE: allow_execution • SCORE: high".to_string(),
            tokens: TokenMetrics {
                input_tokens: 420,
                cache_read_tokens: 380,
                output_tokens: 48,
            },
            tool_calls: vec![ToolCall {
                name: "jev_evaluate_primitives".to_string(),
                summary: "Evaluated {noul, choice, score} in 18ms".to_string(),
                status: "Completed".to_string(),
            }],
            outputs: vec![
                "NOUL (Safety Confidence): 0.92\nCHOICE (Routing Action): allow_execution\nSCORE (Invariant Rubric): high_compliance (0.94)".to_string(),
            ],
            x: 220.0,
            y: 340.0,
            council_score: Some(9.4),
            is_council_winner: false,
            decision_eval: Some(DecisionEvaluation {
                engine: "jev-latest (TypeSafe System-1)".to_string(),
                noul_risk_prob: 0.08,
                choice_action: "allow_execution".to_string(),
                score_rubric: "high_compliance".to_string(),
                confidence: 0.94,
                gate_status: "ALLOW_EXECUTION".to_string(),
                latency_ms: 18,
                primitives: vec![
                    DecisionPrimitive {
                        key: "regression_risk_noul".to_string(),
                        primitive_type: "noul".to_string(),
                        value: "0.08".to_string(),
                        numeric_value: Some(0.08),
                        confidence: 0.95,
                        description: "Probability of invariant regression or unsafe side-effect".to_string(),
                    },
                    DecisionPrimitive {
                        key: "execution_route_choice".to_string(),
                        primitive_type: "choice".to_string(),
                        value: "allow_execution".to_string(),
                        numeric_value: None,
                        confidence: 0.94,
                        description: "Discrete System-1 gate routing decision".to_string(),
                    },
                    DecisionPrimitive {
                        key: "architecture_alignment_score".to_string(),
                        primitive_type: "score".to_string(),
                        value: "high_compliance (0.94)".to_string(),
                        numeric_value: Some(0.94),
                        confidence: 0.93,
                        description: "Ordinal rubric score against project boundary rules".to_string(),
                    },
                ],
            }),
            interactive_prompt: None,
        },
        AgentNode {
            agent_id: format!("{}-verifier", p_clean),
            parent_id: format!("{}-orchestrator", p_clean),
            node_kind: "subagent".to_string(),
            provider: "claude".to_string(),
            model: "claude-4-sonnet".to_string(),
            role: "Adversarial Verifier".to_string(),
            status: "AwaitingToolApproval".to_string(),
            execution_step: 6,
            prompt: "Run zero-trust verification gate on candidate changes.".to_string(),
            thinking: "Checking boundary invariants and memory allocations.".to_string(),
            context: format!("Gate: Zero-Trust Disproof in ~/code/{}", p_clean),
            recent_log: "Pending approval: cargo test --release".to_string(),
            tokens: TokenMetrics {
                input_tokens: 6100,
                cache_read_tokens: 4000,
                output_tokens: 310,
            },
            tool_calls: vec![ToolCall {
                name: "run_command".to_string(),
                summary: "cargo test --release".to_string(),
                status: "Pending".to_string(),
            }],
            outputs: vec![],
            x: 460.0,
            y: 340.0,
            council_score: None,
            is_council_winner: false,
            decision_eval: None,
            interactive_prompt: None,
        },
        AgentNode {
            agent_id: format!("{}-worker", p_clean),
            parent_id: format!("{}-orchestrator", p_clean),
            node_kind: "task".to_string(),
            provider: "codex".to_string(),
            model: "TASK • task-1".to_string(),
            role: "cargo test --release".to_string(),
            status: "Running".to_string(),
            execution_step: 9,
            prompt: "Optimize hot-path allocations and binary framing.".to_string(),
            thinking: "Replaced dynamic heap allocations with bounded ring buffers.".to_string(),
            context: format!("Path: ~/code/{}/src", p_clean),
            recent_log: "Zero-allocation pass verified.".to_string(),
            tokens: TokenMetrics {
                input_tokens: 4800,
                cache_read_tokens: 2100,
                output_tokens: 520,
            },
            tool_calls: vec![],
            outputs: vec!["All benchmarks passed (<1ms frame time)".to_string()],
            x: 700.0,
            y: 340.0,
            council_score: None,
            is_council_winner: false,
            decision_eval: None,
            interactive_prompt: None,
        },
    ]
}

/// Declarative sandbox guard for project slugs inside `~/code/<slug>`.
/// Rejects empty strings, `.` / `..`, path separators, leading dots, or characters outside `[a-zA-Z0-9_-]`.
pub fn validate_project_slug(raw_slug: &str) -> Result<String, String> {
    let trimmed = raw_slug.trim();
    if trimmed.is_empty() {
        return Err("Project slug cannot be empty.".to_string());
    }
    if trimmed.len() > 64 {
        return Err("Project slug must be 64 characters or fewer.".to_string());
    }
    if trimmed.starts_with('.') || trimmed.starts_with('-') {
        return Err("Project slug must start with an alphanumeric character.".to_string());
    }
    if trimmed.contains("..") || trimmed.contains('/') || trimmed.contains('\\') {
        return Err("Project slug cannot contain path separators or '..'.".to_string());
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "Project slug may only contain letters, numbers, hyphens (-), and underscores (_)."
                .to_string(),
        );
    }
    Ok(trimmed.to_string())
}

/// Lists all valid top-level project directory names in `~/code/*`.
pub fn list_workspace_projects() -> Vec<String> {
    let root = code_root_dir();
    let mut projects = Vec::new();
    if let Ok(rd) = fs::read_dir(&root) {
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') && validate_project_slug(&name).is_ok() {
                projects.push(name);
            }
        }
    }
    projects.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    projects
}

fn generate_new_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let now_nanos = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1_700_000_000_000_000_000);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let p1 = (now_nanos >> 32) as u32;
    let p2 = ((now_nanos >> 16) & 0xFFFF) as u16;
    let p3 = (0x4000 | (now_nanos & 0x0FFF)) as u16;
    let p4 = (0x8000 | (seq & 0x3FFF)) as u16;
    let p5 = (now_nanos ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15)) & 0xFFFF_FFFF_FFFF;
    format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", p1, p2, p3, p4, p5)
}

/// Core deep-module implementation for provisioning either:
/// - `"existing_repo"`: A fresh session bound to an existing directory in `code_root/<slug>`
/// - `"new_project"`: A brand-new greenfield project directory in `code_root/<slug>` (with optional `git init -b main` + `.gitignore` + `README.md`) plus a bound session in `brain_root/<session_id>`.
pub fn provision_workspace_and_session_in_roots(
    code_root: &Path,
    brain_root: &Path,
    mode: &str,
    raw_project_slug: &str,
    raw_title: Option<&str>,
    raw_description: Option<&str>,
    init_git: bool,
) -> Result<SessionProvisionResult, String> {
    let slug = validate_project_slug(raw_project_slug)?;
    let workspace_dir = code_root.join(&slug);
    let is_new_project_mode = mode == "new_project";

    let mut created_new_project = false;
    let mut git_initialized = false;

    if is_new_project_mode {
        if !workspace_dir.exists() {
            fs::create_dir_all(&workspace_dir)
                .map_err(|e| format!("Failed to create ~/code/{}: {}", slug, e))?;
            created_new_project = true;
        }

        let desc_clean = raw_description
            .map(|d| d.trim())
            .filter(|d| !d.is_empty())
            .unwrap_or("Greenfield project initialized via Trellis AI Harness.");

        let readme_path = workspace_dir.join("README.md");
        if !readme_path.exists() {
            let readme_body = format!(
                "# {}\n\n{}\n\n## Architecture & Development\nManaged with the Trellis AI Harness (`exo-suit`).\n",
                slug, desc_clean
            );
            let _ = fs::write(&readme_path, readme_body);
        }

        let gitignore_path = workspace_dir.join(".gitignore");
        if !gitignore_path.exists() {
            let gitignore_body = "/target/\n/node_modules/\n/.next/\n/dist/\n.DS_Store\n.env\n.env.local\n*.log\n";
            let _ = fs::write(&gitignore_path, gitignore_body);
        }

        if init_git && !workspace_dir.join(".git").exists() {
            let init_ok = std::process::Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(&workspace_dir)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if init_ok {
                let _ = std::process::Command::new("git")
                    .args(["add", "."])
                    .current_dir(&workspace_dir)
                    .output();
                let _ = std::process::Command::new("git")
                    .args([
                        "commit",
                        "-m",
                        &format!("chore: initialize {} greenfield workspace", slug),
                    ])
                    .current_dir(&workspace_dir)
                    .output();
                git_initialized = true;
            }
        } else if workspace_dir.join(".git").exists() {
            git_initialized = true;
        }
    } else if !workspace_dir.is_dir() {
        return Err(format!(
            "Workspace directory ~/code/{} does not exist. Switch to 'New Greenfield Project' to create it.",
            slug
        ));
    } else {
        git_initialized = workspace_dir.join(".git").exists();
    }

    // Generate & persist the new session inside `brain_root/<session_id>`
    let session_id = generate_new_session_id();
    let session_dir = brain_root.join(&session_id);
    let logs_dir = session_dir.join(".system_generated").join("logs");
    fs::create_dir_all(&logs_dir)
        .map_err(|e| format!("Failed to create session brain directory: {}", e))?;

    let final_title = raw_title
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .unwrap_or_else(|| {
            if is_new_project_mode {
                format!("{} — Greenfield Architecture", slug)
            } else {
                format!("{} — Workspace Session", slug)
            }
        });

    atomic_write_string(&session_dir.join(".session_project.txt"), &slug)
        .map_err(|e| format!("Failed to persist .session_project.txt: {}", e))?;
    atomic_write_string(&session_dir.join(".session_title.txt"), &final_title)
        .map_err(|e| format!("Failed to persist .session_title.txt: {}", e))?;

    let now_secs = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_730_000_000);
    let ts_str = format_epoch_secs(now_secs);

    // Seed an initial SYSTEM step in transcript.jsonl so `get_past_sessions` indexes and orders it at the top immediately
    let seed_step = serde_json::json!({
        "step_index": 0,
        "source": "SYSTEM",
        "type": "SESSION_INITIALIZED",
        "status": "DONE",
        "created_at": ts_str,
        "content": format!("Initialized session '{}' bound to {}", final_title, workspace_dir.display())
    });
    fs::write(
        logs_dir.join("transcript.jsonl"),
        format!("{}\n", seed_step),
    )
    .map_err(|e| format!("Failed to seed transcript.jsonl: {}", e))?;

    Ok(SessionProvisionResult {
        session_id,
        project_id: slug,
        title: final_title,
        workspace_path: workspace_dir.to_string_lossy().to_string(),
        created_new_project,
        git_initialized,
    })
}

/// Public entry point using the host's `~/code` and `~/.gemini/antigravity-cli/brain` roots.
pub fn provision_workspace_and_session(
    mode: &str,
    raw_project_slug: &str,
    raw_title: Option<&str>,
    raw_description: Option<&str>,
    init_git: bool,
) -> Result<SessionProvisionResult, String> {
    provision_workspace_and_session_in_roots(
        &code_root_dir(),
        &brain_dir(),
        mode,
        raw_project_slug,
        raw_title,
        raw_description,
        init_git,
    )
}

pub fn load_session_agy_conversation_id(session_id: &str) -> Option<String> {
    if session_id.is_empty() || session_id.contains("..") || session_id.contains('/') {
        return None;
    }
    let map_file = brain_dir().join(session_id).join(".agy_conversation_id.txt");
    if let Ok(raw) = fs::read_to_string(&map_file) {
        let trimmed = raw.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    None
}

pub fn save_session_agy_conversation_id(session_id: &str, agy_conv_id: &str) {
    if session_id.is_empty()
        || session_id.contains("..")
        || session_id.contains('/')
        || agy_conv_id.trim().is_empty()
    {
        return;
    }
    let s_dir = brain_dir().join(session_id);
    if s_dir.is_dir() {
        let _ = atomic_write_string(&s_dir.join(".agy_conversation_id.txt"), agy_conv_id.trim());
    }
}

pub fn append_turn_to_session_transcript(
    session_id: &str,
    prompt: &str,
    reply_text: &str,
    tool_calls: &[ToolCall],
    thinking: Option<&str>,
    tokens: &TokenMetrics,
) {
    if session_id.is_empty() || session_id.contains("..") || session_id.contains('/') {
        return;
    }
    if load_session_agy_conversation_id(session_id).as_deref() == Some(session_id) {
        return;
    }
    let logs_dir = brain_dir()
        .join(session_id)
        .join(".system_generated")
        .join("logs");
    if fs::create_dir_all(&logs_dir).is_err() {
        return;
    }
    let transcript_path = logs_dir.join("transcript.jsonl");
    let existing = load_transcript_values(session_id);
    let trimmed_reply = reply_text.trim();
    if !trimmed_reply.is_empty() {
        if let Some(last_planner) = existing
            .iter()
            .rev()
            .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("PLANNER_RESPONSE"))
        {
            if last_planner
                .get("content")
                .and_then(|c| c.as_str())
                .map(|c| c.trim() == trimmed_reply)
                .unwrap_or(false)
            {
                return;
            }
        }
    }
    let next_step = existing
        .iter()
        .filter_map(|v| v.get("step_index").and_then(|s| s.as_u64()))
        .max()
        .map(|m| m as u32 + 1)
        .unwrap_or(1);

    let now_secs = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_730_000_000);
    let ts_str = format_epoch_secs(now_secs);

    let user_step = serde_json::json!({
        "step_index": next_step,
        "source": "USER_EXPLICIT",
        "type": "USER_INPUT",
        "status": "DONE",
        "created_at": ts_str,
        "content": format!("<USER_REQUEST>\n{}\n</USER_REQUEST>", prompt)
    });

    let serialized_tcs: Vec<Value> = tool_calls
        .iter()
        .map(|tc| {
            serde_json::json!({
                "name": tc.name,
                "args": {
                    "toolSummary": tc.summary
                }
            })
        })
        .collect();

    let planner_step = serde_json::json!({
        "step_index": next_step + 1,
        "source": "MODEL",
        "type": "PLANNER_RESPONSE",
        "status": "DONE",
        "created_at": ts_str,
        "thinking": thinking.unwrap_or(""),
        "content": reply_text,
        "input_tokens": tokens.input_tokens,
        "cache_read_tokens": tokens.cache_read_tokens,
        "output_tokens": tokens.output_tokens,
        "tool_calls": serialized_tcs
    });

    use std::io::Write;
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript_path)
    {
        let _ = writeln!(file, "{}", user_step);
        let _ = writeln!(file, "{}", planner_step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_test_validate_project_slug_rejects_traversal_and_invalid_chars() {
        assert_eq!(
            validate_project_slug("pulse-engine").unwrap(),
            "pulse-engine"
        );
        assert_eq!(validate_project_slug("exo_suit_2").unwrap(), "exo_suit_2");

        assert!(validate_project_slug("").is_err());
        assert!(validate_project_slug("   ").is_err());
        assert!(validate_project_slug("..").is_err());
        assert!(validate_project_slug("../etc").is_err());
        assert!(validate_project_slug("foo/bar").is_err());
        assert!(validate_project_slug(".hidden").is_err());
        assert!(validate_project_slug("-dashstart").is_err());
        assert!(validate_project_slug("space name").is_err());
    }

    #[test]
    fn anchor_test_provision_new_session_and_greenfield_project() {
        let tmp_base = std::env::temp_dir().join(format!(
            "trellis_provision_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_base);
        let code_root = tmp_base.join("code");
        let brain_root = tmp_base.join("brain");
        fs::create_dir_all(&code_root).unwrap();
        fs::create_dir_all(&brain_root).unwrap();

        // 1. Provision a brand-new greenfield project + session
        let res = provision_workspace_and_session_in_roots(
            &code_root,
            &brain_root,
            "new_project",
            "nova-ledger",
            Some("Nova Ledger Day-1 Spec"),
            Some("Zero-copy financial ledger engine."),
            true,
        )
        .expect("Greenfield project provisioning should succeed");

        assert_eq!(res.project_id, "nova-ledger");
        assert_eq!(res.title, "Nova Ledger Day-1 Spec");
        assert!(res.created_new_project);
        assert!(res.git_initialized);

        let proj_dir = code_root.join("nova-ledger");
        assert!(proj_dir.is_dir());
        assert!(proj_dir.join("README.md").is_file());
        assert!(proj_dir.join(".gitignore").is_file());
        assert!(proj_dir.join(".git").is_dir());

        let sess_dir = brain_root.join(&res.session_id);
        assert_eq!(
            fs::read_to_string(sess_dir.join(".session_project.txt")).unwrap(),
            "nova-ledger"
        );
        assert_eq!(
            fs::read_to_string(sess_dir.join(".session_title.txt")).unwrap(),
            "Nova Ledger Day-1 Spec"
        );
        assert!(sess_dir
            .join(".system_generated/logs/transcript.jsonl")
            .is_file());

        // 2. Provision a second session in the now-existing project
        let res2 = provision_workspace_and_session_in_roots(
            &code_root,
            &brain_root,
            "existing_repo",
            "nova-ledger",
            Some("Nova Bugfix Thread"),
            None,
            false,
        )
        .expect("Existing repo session provisioning should succeed");

        assert_eq!(res2.project_id, "nova-ledger");
        assert!(!res2.created_new_project);
        assert_ne!(res.session_id, res2.session_id);

        let _ = fs::remove_dir_all(&tmp_base);
    }

    #[test]
    fn anchor_test_reconcile_interactive_prompt_and_atomic_write() {
        let tmp_base = std::env::temp_dir().join(format!(
            "trellis_brain_unit_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_base);

        // 1. Atomic write string
        let fpath = tmp_base.join("sub").join(".session_project.txt");
        atomic_write_string(&fpath, "exo-suit").expect("atomic write should succeed");
        assert_eq!(fs::read_to_string(&fpath).unwrap(), "exo-suit");

        // 2. reconcile_interactive_prompt does not falsely mark Answered on non-USER_INPUT steps
        let qs = vec![InteractiveQuestionItem {
            index: 1,
            question: "Pick a mode".to_string(),
            options: vec!["Fast".to_string(), "Thorough".to_string()],
            is_multi_select: false,
        }];
        let saved = HashMap::new();
        let non_user_steps = vec![serde_json::json!({
            "step_index": 5,
            "type": "GENERIC",
            "status": "DONE",
            "content": "Command exited with code 0"
        })];
        let prompt_pending = reconcile_interactive_prompt(4, qs.clone(), &non_user_steps, &saved);
        assert_eq!(prompt_pending.status, "Pending");

        let user_steps = vec![serde_json::json!({
            "step_index": 5,
            "type": "USER_INPUT",
            "status": "DONE",
            "content": "<USER_REQUEST>Use Thorough mode</USER_REQUEST>"
        })];
        let prompt_answered = reconcile_interactive_prompt(4, qs, &user_steps, &saved);
        assert_eq!(prompt_answered.status, "Answered");

        let _ = fs::remove_dir_all(&tmp_base);
    }
}

