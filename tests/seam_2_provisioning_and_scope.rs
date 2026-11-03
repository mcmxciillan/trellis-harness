use exo_suit::brain::{
    brain_dir, code_root_dir, extract_project_slug_from_text, get_past_sessions,
    infer_session_project_id, provision_workspace_and_session_in_roots, rename_session,
    resolve_execution_scope, resolve_sandboxed_workspace_path, save_session_project_id,
    validate_project_slug,
};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct TempBrainGuard {
    _lock: MutexGuard<'static, ()>,
    brain_root: PathBuf,
}

impl TempBrainGuard {
    fn new(label: &str) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let brain_root = std::env::temp_dir().join(format!(
            "exo_suit_seam2_{}_{}",
            label,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&brain_root);
        fs::create_dir_all(&brain_root).unwrap();
        std::env::set_var("EXO_BRAIN_DIR", &brain_root);
        Self {
            _lock: lock,
            brain_root,
        }
    }
}

impl Drop for TempBrainGuard {
    fn drop(&mut self) {
        std::env::remove_var("EXO_BRAIN_DIR");
        let _ = fs::remove_dir_all(&self.brain_root);
    }
}

#[test]
fn seam2_validate_project_slug_enforces_strict_sandbox_naming() {
    assert_eq!(validate_project_slug("exo-suit").unwrap(), "exo-suit");
    assert_eq!(
        validate_project_slug("  pulse_engine_v2  ").unwrap(),
        "pulse_engine_v2"
    );

    // Rejects empty, path traversal, slashes, leading dots/dashes, and >64 chars
    assert!(validate_project_slug("").is_err());
    assert!(validate_project_slug("..").is_err());
    assert!(validate_project_slug("../secret").is_err());
    assert!(validate_project_slug("sub/dir").is_err());
    assert!(validate_project_slug(".env").is_err());
    assert!(validate_project_slug("-invalid").is_err());
    let long_slug = "a".repeat(65);
    assert!(validate_project_slug(&long_slug).is_err());
}

#[test]
fn seam2_provision_greenfield_project_and_existing_repo_sessions() {
    let tmp_dir =
        std::env::temp_dir().join(format!("exo_suit_seam2_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    let code_root = tmp_dir.join("code");
    let brain_root = tmp_dir.join("brain");
    fs::create_dir_all(&code_root).unwrap();
    fs::create_dir_all(&brain_root).unwrap();

    // 1. Attempting "existing_repo" on a non-existent folder fails cleanly without creating files
    let missing_err = provision_workspace_and_session_in_roots(
        &code_root,
        &brain_root,
        "existing_repo",
        "nonexistent-repo",
        Some("Should Fail"),
        None,
        false,
    );
    assert!(missing_err.is_err());
    assert!(!code_root.join("nonexistent-repo").exists());

    // 2. Provisioning a brand-new greenfield project creates directory, README.md, .gitignore, git repo, and session
    let created = provision_workspace_and_session_in_roots(
        &code_root,
        &brain_root,
        "new_project",
        "atlas-core",
        Some("Atlas Core Architecture"),
        Some("High-throughput distributed state machine."),
        true,
    )
    .expect("new_project provisioning should succeed");

    assert_eq!(created.project_id, "atlas-core");
    assert_eq!(created.title, "Atlas Core Architecture");
    assert!(created.created_new_project);
    assert!(created.git_initialized);

    let repo_path = code_root.join("atlas-core");
    assert!(repo_path.join("README.md").is_file());
    assert!(
        fs::read_to_string(repo_path.join("README.md"))
            .unwrap()
            .contains("High-throughput distributed state machine.")
    );
    assert!(repo_path.join(".gitignore").is_file());
    assert!(repo_path.join(".git").is_dir());

    let sess_path = brain_root.join(&created.session_id);
    assert_eq!(
        fs::read_to_string(sess_path.join(".session_project.txt")).unwrap(),
        "atlas-core"
    );
    assert_eq!(
        fs::read_to_string(sess_path.join(".session_title.txt")).unwrap(),
        "Atlas Core Architecture"
    );
    assert!(sess_path
        .join(".system_generated/logs/transcript.jsonl")
        .is_file());

    // 3. Provisioning an "existing_repo" session in the now-existing `atlas-core` repo succeeds
    let second_sess = provision_workspace_and_session_in_roots(
        &code_root,
        &brain_root,
        "existing_repo",
        "atlas-core",
        None, // default title fallback
        None,
        false,
    )
    .expect("existing_repo provisioning should succeed");

    assert_eq!(second_sess.project_id, "atlas-core");
    assert_eq!(second_sess.title, "atlas-core — Workspace Session");
    assert!(!second_sess.created_new_project);
    assert!(second_sess.git_initialized);
    assert_ne!(created.session_id, second_sess.session_id);

    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn seam2_session_project_binding_renaming_and_execution_scope_resolution() {
    let _guard = TempBrainGuard::new("scope");
    let test_sid = format!("test-seam2-session-{}", std::process::id());
    let s_dir = brain_dir().join(&test_sid);
    fs::create_dir_all(&s_dir).unwrap();

    // Save project binding & verify inference + strict slug validation
    assert!(save_session_project_id(&test_sid, "exo-suit"));
    assert_eq!(infer_session_project_id(&test_sid), "exo-suit");
    assert!(!save_session_project_id(&test_sid, "../escape"));
    assert!(!save_session_project_id(&test_sid, ".hidden"));
    assert!(!save_session_project_id(&test_sid, "-leading-dash"));
    assert!(!save_session_project_id(&test_sid, "nested/slug"));

    // Rename session & verify persistence
    assert!(rename_session(&test_sid, "Custom Renamed Title"));
    assert_eq!(
        fs::read_to_string(s_dir.join(".session_title.txt")).unwrap(),
        "Custom Renamed Title"
    );

    // Clearing title removes `.session_title.txt`
    assert!(rename_session(&test_sid, "   "));
    assert!(!s_dir.join(".session_title.txt").exists());

    // Extract slug from text and resolve execution scope
    let sample_path = format!("Reading {}/demo-app/package.json", code_root_dir().display());
    assert_eq!(
        extract_project_slug_from_text(&sample_path),
        Some("demo-app".to_string())
    );
    let scope = resolve_execution_scope(&test_sid, "");
    assert_eq!(scope.session_id, test_sid);
    assert_eq!(scope.project_id, "exo-suit");
    assert!(scope.workspace_path.ends_with("exo-suit"));

    // Traversal attempt falls back safely to exo-suit
    let traversal_scope = resolve_execution_scope(&test_sid, "../../../etc");
    assert_eq!(traversal_scope.project_id, "exo-suit");

    // Verify resolve_sandboxed_workspace_path allows valid workspace paths and blocks traversal/symlinks
    assert!(resolve_sandboxed_workspace_path("exo-suit", "Cargo.toml").is_some());
    assert!(resolve_sandboxed_workspace_path("exo-suit", "src/new_uncreated_file.rs").is_some());
    assert!(resolve_sandboxed_workspace_path("exo-suit", "../outside.txt").is_none());
    assert!(resolve_sandboxed_workspace_path("../etc", "passwd").is_none());
    assert!(resolve_sandboxed_workspace_path("exo-suit", "/etc/passwd").is_none());

    // Verify get_past_sessions infers and caches .session_project.txt in a single pass
    let uncached_sid = "uncached-session-1";
    let uncached_logs = brain_dir()
        .join(uncached_sid)
        .join(".system_generated")
        .join("logs");
    fs::create_dir_all(&uncached_logs).unwrap();
    fs::write(
        uncached_logs.join("transcript.jsonl"),
        format!(
            "{{\"step_index\":1,\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"content\":\"Inspect {}/exo-suit/Cargo.toml\"}}\n",
            code_root_dir().display()
        ),
    )
    .unwrap();
    let past = get_past_sessions(10);
    let found = past
        .iter()
        .find(|s| s.session_id == uncached_sid)
        .expect("uncached session should appear in get_past_sessions");
    assert_eq!(found.project_id, "exo-suit");
    assert_eq!(
        fs::read_to_string(brain_dir().join(uncached_sid).join(".session_project.txt")).unwrap(),
        "exo-suit"
    );
}
