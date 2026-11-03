use exo_suit::brain::code_root_dir;
use exo_suit::devops::{
    build_workspace_devops_snapshot, compute_workspace_diff, detect_test_runner_in_dir,
    detect_test_runner_spec_in_dir, parse_git_porcelain_and_numstat, parse_test_runner_counts,
    probe_deploy_and_health, validate_workspace_sandbox, TestRunnerSpec,
};
use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("exo-suit-{}-{}-{}", prefix, std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn seam4_validate_workspace_sandbox_enforces_strict_boundary() {
    assert!(validate_workspace_sandbox("", None).is_err());
    assert!(validate_workspace_sandbox("../etc", None).is_err());
    assert!(validate_workspace_sandbox("exo-suit/sub", None).is_err());
    assert!(validate_workspace_sandbox("non-existent-workspace-999999", None).is_err());

    // Rejects target_path traversal
    assert!(validate_workspace_sandbox("exo-suit", Some("../outside.rs")).is_err());
    assert!(validate_workspace_sandbox("exo-suit", Some("src/../../outside.rs")).is_err());

    // Accepts valid workspace and strips leading slash on relative target_path
    let (work_dir, rel) =
        validate_workspace_sandbox("exo-suit", Some("/src/devops.rs")).expect("valid sandbox path");
    assert!(work_dir.ends_with("exo-suit"));
    assert_eq!(rel.as_deref(), Some("src/devops.rs"));
}

#[test]
fn seam4_validate_workspace_sandbox_rejects_symlink_escape() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_proj_id = format!(".tmp-sandbox-symlink-{}-{}", std::process::id(), nanos);
    let work_dir = code_root_dir().join(&temp_proj_id);
    fs::create_dir_all(&work_dir).expect("create temporary workspace in code root");

    let outside_dir = temp_dir("outside-boundary");
    let outside_file = outside_dir.join("secret.txt");
    fs::write(&outside_file, "classified").unwrap();

    let inside_file = work_dir.join("safe.txt");
    fs::write(&inside_file, "safe").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("/etc/hosts", work_dir.join("hosts_link")).expect("create /etc/hosts symlink");
        symlink(&outside_file, work_dir.join("outside_link"))
            .expect("create outside temp file symlink");
        symlink(&outside_dir, work_dir.join("outside_dir_link"))
            .expect("create outside temp dir symlink");
        symlink(&inside_file, work_dir.join("inside_link"))
            .expect("create internal workspace symlink");
    }

    let err_hosts = validate_workspace_sandbox(&temp_proj_id, Some("hosts_link"))
        .expect_err("symlink to /etc/hosts must be rejected");
    assert_eq!(
        err_hosts,
        "Sandbox violation: symlink target escapes workspace boundary"
    );

    let err_outside = validate_workspace_sandbox(&temp_proj_id, Some("outside_link"))
        .expect_err("symlink to parent temp dir file must be rejected");
    assert_eq!(
        err_outside,
        "Sandbox violation: symlink target escapes workspace boundary"
    );

    let err_nested =
        validate_workspace_sandbox(&temp_proj_id, Some("outside_dir_link/nonexistent.txt"))
            .expect_err("symlink parent directory pointing outside workspace must be rejected");
    assert_eq!(
        err_nested,
        "Sandbox violation: symlink target escapes workspace boundary"
    );

    let ok_inside = validate_workspace_sandbox(&temp_proj_id, Some("inside_link"))
        .expect("internal symlink staying inside workspace should succeed");
    assert_eq!(ok_inside.1.as_deref(), Some("inside_link"));

    let _ = fs::remove_dir_all(&work_dir);
    let _ = fs::remove_dir_all(&outside_dir);
}

#[test]
fn seam4_parse_git_porcelain_handles_renames_staged_untracked_and_commit_history() {
    let porcelain = "\
## main...origin/main [ahead 3, behind 2]
M  src/lib.rs
 M src/main.rs
R  src/old_name.rs -> src/new_name.rs
?? tests/new_test.rs
";
    let numstat = "\
45\t5\tsrc/lib.rs
12\t8\tsrc/main.rs
80\t2\tsrc/new_name.rs
";
    let log_raw = "\
112233a|feat: extract library crate target|Corey McMillan|5 minutes ago
445566b|test: add seam integration suite|Corey McMillan|12 minutes ago
";

    let snap = parse_git_porcelain_and_numstat(porcelain, numstat, log_raw);
    assert_eq!(snap.branch, "main");
    assert_eq!(snap.ahead, 3);
    assert_eq!(snap.behind, 2);
    assert!(!snap.is_clean);
    assert_eq!(snap.changed_files.len(), 4);

    // 1. Staged modification (`M `)
    assert_eq!(snap.changed_files[0].path, "src/lib.rs");
    assert_eq!(snap.changed_files[0].status, "M");
    assert!(snap.changed_files[0].staged);
    assert_eq!(snap.changed_files[0].additions, 45);
    assert_eq!(snap.changed_files[0].deletions, 5);

    // 2. Unstaged modification (` M`)
    assert_eq!(snap.changed_files[1].path, "src/main.rs");
    assert_eq!(snap.changed_files[1].status, "M");
    assert!(!snap.changed_files[1].staged);

    // 3. Staged rename (`R  old -> new`) resolves destination path and numstat
    assert_eq!(snap.changed_files[2].path, "src/new_name.rs");
    assert_eq!(snap.changed_files[2].status, "R");
    assert!(snap.changed_files[2].staged);
    assert_eq!(snap.changed_files[2].additions, 80);

    // 4. Untracked (`??`)
    assert_eq!(snap.changed_files[3].path, "tests/new_test.rs");
    assert_eq!(snap.changed_files[3].status, "??");
    assert!(!snap.changed_files[3].staged);

    assert_eq!(snap.total_additions, 45 + 12 + 80);
    assert_eq!(snap.total_deletions, 5 + 8 + 2);
    assert_eq!(snap.recent_commits.len(), 2);
    assert_eq!(snap.recent_commits[0].sha, "112233a");
    assert_eq!(
        snap.recent_commits[0].subject,
        "feat: extract library crate target"
    );
}

#[test]
fn seam4_detect_test_runner_in_dir_supports_polyglot_manifests() {
    let cargo_dir = temp_dir("runner-cargo");
    fs::write(cargo_dir.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    assert_eq!(
        detect_test_runner_in_dir(&cargo_dir),
        Some(("cargo".to_string(), "cargo test".to_string()))
    );
    assert_eq!(
        detect_test_runner_spec_in_dir(&cargo_dir),
        Some(TestRunnerSpec {
            framework: "cargo".to_string(),
            program: "cargo".to_string(),
            args: vec!["test".to_string()],
            display_command: "cargo test".to_string(),
        })
    );

    let npm_dir = temp_dir("runner-npm");
    fs::write(npm_dir.join("package.json"), "{}").unwrap();
    assert_eq!(
        detect_test_runner_in_dir(&npm_dir),
        Some(("npm".to_string(), "npm test".to_string()))
    );
    assert_eq!(
        detect_test_runner_spec_in_dir(&npm_dir),
        Some(TestRunnerSpec {
            framework: "npm".to_string(),
            program: "npm".to_string(),
            args: vec!["test".to_string()],
            display_command: "npm test".to_string(),
        })
    );

    let py_dir = temp_dir("runner-pytest");
    fs::write(py_dir.join("pyproject.toml"), "[tool.pytest]\n").unwrap();
    assert_eq!(
        detect_test_runner_in_dir(&py_dir),
        Some(("pytest".to_string(), "pytest".to_string()))
    );
    assert_eq!(
        detect_test_runner_spec_in_dir(&py_dir),
        Some(TestRunnerSpec {
            framework: "pytest".to_string(),
            program: "pytest".to_string(),
            args: vec![],
            display_command: "pytest".to_string(),
        })
    );

    let go_dir = temp_dir("runner-go");
    fs::write(go_dir.join("go.mod"), "module example.com/demo\n").unwrap();
    assert_eq!(
        detect_test_runner_in_dir(&go_dir),
        Some(("go".to_string(), "go test ./...".to_string()))
    );
    assert_eq!(
        detect_test_runner_spec_in_dir(&go_dir),
        Some(TestRunnerSpec {
            framework: "go".to_string(),
            program: "go".to_string(),
            args: vec!["test".to_string(), "./...".to_string()],
            display_command: "go test ./...".to_string(),
        })
    );

    let empty_dir = temp_dir("runner-empty");
    assert_eq!(detect_test_runner_spec_in_dir(&empty_dir), None);
    assert_eq!(detect_test_runner_in_dir(&empty_dir), None);

    let _ = fs::remove_dir_all(&cargo_dir);
    let _ = fs::remove_dir_all(&npm_dir);
    let _ = fs::remove_dir_all(&py_dir);
    let _ = fs::remove_dir_all(&go_dir);
    let _ = fs::remove_dir_all(&empty_dir);
}

#[test]
fn seam4_parse_test_runner_counts_aggregates_multi_binary_cargo_and_pytest_outputs() {
    let multi_cargo = "\
     Running unittests src/lib.rs
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running tests/seam_1_brain_and_transcript.rs
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running tests/seam_2_provisioning_and_scope.rs
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
";
    let (passed, failed) = parse_test_runner_counts("cargo", multi_cargo, false);
    assert_eq!(passed, 15);
    assert_eq!(failed, 1);

    let pytest_out = "=== 12 passed, 3 failed in 1.28s ===";
    let (py_pass, py_fail) = parse_test_runner_counts("pytest", pytest_out, false);
    assert_eq!(py_pass, 12);
    assert_eq!(py_fail, 3);
}

#[test]
fn seam4_compute_workspace_diff_synthesizes_untracked_file_and_builds_snapshot() {
    // `src/lib.rs` is in `exo-suit`
    let diff = compute_workspace_diff("exo-suit", Some("src/lib.rs"));
    assert_eq!(diff.project_id, "exo-suit");
    assert_eq!(diff.target_path.as_deref(), Some("src/lib.rs"));
    assert!(
        diff.unified_diff.contains("+++ b/src/lib.rs"),
        "Unified diff should include header for src/lib.rs"
    );
    assert!(diff.additions > 0);

    let devops = build_workspace_devops_snapshot("exo-suit", None, 9000);
    assert_eq!(devops.project_id, "exo-suit");
    assert_eq!(devops.tests.framework, "cargo");
    assert_eq!(devops.tests.command, "cargo test");
    assert!(!devops.deploy_health.endpoints.is_empty());
    assert_eq!(devops.deploy_health.endpoints[0].status_code, 200);

    // Verify 15-second TTL deploy health cache returns cached DeployHealthSummary in <1ms
    let started = Instant::now();
    let cached_deploy = probe_deploy_and_health("exo-suit", &devops.git, 9000);
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_millis() < 1,
        "Cached probe_deploy_and_health should complete in <1ms, took {:?}",
        elapsed
    );
    assert_eq!(cached_deploy, devops.deploy_health);
}
