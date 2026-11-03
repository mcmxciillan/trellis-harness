use exo_suit::brain::{
    brain_dir, code_root_dir, extract_project_slug_from_text, infer_session_project_id,
    provision_workspace_and_session_in_roots, rename_session, resolve_execution_scope,
    save_session_project_id, validate_project_slug,
};
use std::fs;

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
    let test_sid = format!("test-seam2-session-{}", std::process::id());
    let s_dir = brain_dir().join(&test_sid);
    fs::create_dir_all(&s_dir).unwrap();

    // Save project binding & verify inference
    assert!(save_session_project_id(&test_sid, "exo-suit"));
    assert_eq!(infer_session_project_id(&test_sid), "exo-suit");

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

    let _ = fs::remove_dir_all(&s_dir);
}
