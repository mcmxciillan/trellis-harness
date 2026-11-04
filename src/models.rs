use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub model_class: String, // "GENERATIVE_SYSTEM_2" | "DECISION_SYSTEM_1"
    pub task_specialty: String,
    pub default_model: String,
    pub available_models: Vec<String>,
    pub color: String,
    pub icon: String,
    pub mode: String, // "LIVE_CLI", "LIVE_API", "SIMULATED"
    pub cli_path: Option<String>,
    pub has_api_key: bool,
    pub enabled_in_council: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionPrimitive {
    pub key: String,
    pub primitive_type: String, // "noul" | "choice" | "score"
    pub value: String,
    pub numeric_value: Option<f32>,
    pub confidence: f32,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionEvaluation {
    pub engine: String, // "jev-latest (TypeSafe System-1)"
    pub noul_risk_prob: f32,
    pub choice_action: String,
    pub score_rubric: String,
    pub confidence: f32,
    pub gate_status: String, // "ALLOW_EXECUTION" | "REQUIRE_APPROVAL" | "FAIL_CLOSED_BLOCK"
    pub latency_ms: u64,
    pub primitives: Vec<DecisionPrimitive>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenMetrics {
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub summary: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InteractiveQuestionItem {
    pub index: usize,
    pub question: String,
    pub options: Vec<String>,
    pub is_multi_select: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InteractivePrompt {
    pub prompt_id: String,
    pub step_index: u32,
    pub status: String, // "Pending" | "Answered"
    pub questions: Vec<InteractiveQuestionItem>,
    pub answers: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub step_index: u32,
    pub role: String,     // "user" | "assistant" | "council" | "decision"
    pub sender: String,   // e.g. "You", "Antigravity", "Jev (System-1)", "AI Council"
    pub provider: String, // "user", "antigravity", "jev", "claude", etc.
    pub model: String,
    pub timestamp: String,
    pub content: String,
    pub thinking: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub interactive_prompt: Option<InteractivePrompt>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentNode {
    pub agent_id: String,
    pub parent_id: String,
    #[serde(default)]
    pub node_kind: String, // "agent" | "subagent" | "task" | "gate"
    pub provider: String,
    pub model: String,
    pub role: String,
    pub status: String, // "Running", "Completed", "AwaitingToolApproval", "AwaitingUserInput", "Negotiating", "Voting", "Winner", "DecisionGate", "Errored", "Idle"
    pub execution_step: u32,
    pub prompt: String,
    pub thinking: String,
    pub context: String,
    pub recent_log: String,
    pub tokens: TokenMetrics,
    pub tool_calls: Vec<ToolCall>,
    pub outputs: Vec<String>,
    pub x: f32,
    pub y: f32,
    pub council_score: Option<f32>,
    pub is_council_winner: bool,
    pub decision_eval: Option<DecisionEvaluation>,
    #[serde(default)]
    pub interactive_prompt: Option<InteractivePrompt>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CouncilProposal {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    pub agent_id: String,
    pub initial_draft: String,
    pub revised_draft: Option<String>,
    pub critique_given: Option<String>,
    pub latency_ms: u64,
    pub execution_mode: String,
    pub jev_noul_safety: Option<f32>,
    pub jev_choice_verdict: Option<String>,
    pub jev_rubric_score: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RankedChoiceVote {
    pub voter_provider: String,
    pub voter_class: String, // "GENERATIVE_PEER" | "DECISION_GATE (Jev)"
    pub first_choice: String,
    pub second_choice: String,
    pub confidence: f32,
    pub rationale: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CouncilScoreEntry {
    pub provider_id: String,
    pub provider_name: String,
    pub borda_points: f32,
    pub jev_gate_bonus: f32,
    pub weighted_score: f32,
    pub first_place_votes: u32,
    pub second_place_votes: u32,
    pub jev_verdict: String,
    pub is_winner: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CouncilSession {
    pub council_id: String,
    pub prompt: String,
    pub project_id: String,
    pub stage: String, // "STAGE_1_PROPOSALS", "STAGE_2_NEGOTIATION", "STAGE_3_VOTING", "COMPLETED"
    pub stage_number: u8,
    pub participants: Vec<String>,
    pub decision_models: Vec<String>,
    pub proposals: Vec<CouncilProposal>,
    pub votes: Vec<RankedChoiceVote>,
    pub leaderboard: Vec<CouncilScoreEntry>,
    pub winning_provider: Option<String>,
    pub synthesized_output: Option<String>,
    pub jev_gate_summary: Option<DecisionEvaluation>,
    pub started_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionItem {
    pub session_id: String,
    #[serde(default)]
    pub project_id: String,
    pub updated_at: String,
    pub prompt_preview: String,
    #[serde(default)]
    pub custom_title: Option<String>,
    pub artifact_count: u32,
    pub step_count: u32,
    pub subagent_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillItem {
    pub id: String,
    pub name: String,
    pub source: String,
    pub path: String,
    pub description: String,
    pub content_preview: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCatalogItem {
    pub name: String,
    pub server: String, // "native-harness" | "chrome-devtools" | "github" | "google-developer-knowledge"
    pub mode: String,   // "EAGER" | "LAZY"
    pub description: String,
    pub parameters_summary: String,
    pub schema_preview: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactItem {
    pub name: String,
    pub path: String,
    pub summary: String,
    pub request_feedback: bool,
    pub user_facing: bool,
    pub approved: bool,
    pub content_preview: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub rel_path: String,
    pub is_dir: bool,
    pub is_project: bool,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileTreeInfo {
    pub root_dir: String,
    pub current_rel_path: String,
    pub can_navigate_up: bool,
    pub entries: Vec<FileEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenedFile {
    pub name: String,
    pub rel_path: String,
    #[serde(default)]
    pub abs_path: String,
    #[serde(default)]
    pub file_kind: String,
    #[serde(default)]
    pub size_bytes: u64,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClientCommand {
    pub command: String,
    pub path: Option<String>,
    pub session_id: Option<String>,
    pub project_id: Option<String>,
    pub run_id: Option<String>,
    pub title: Option<String>,
    pub mode: Option<String>,
    pub description: Option<String>,
    pub init_git: Option<bool>,
    pub auto_kickoff: Option<bool>,
    pub artifact_name: Option<String>,
    pub agent_id: Option<String>,
    pub prompt: Option<String>,
    pub prompt_id: Option<String>,
    pub answers: Option<Vec<String>>,
    pub provider: Option<String>,
    pub providers: Option<Vec<String>>,
    pub model: Option<String>,
    pub api_keys: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionProvisionResult {
    pub session_id: String,
    pub project_id: String,
    pub title: String,
    pub workspace_path: String,
    pub created_new_project: bool,
    pub git_initialized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionScope {
    pub session_id: String,
    pub project_id: String,
    pub workspace_path: std::path::PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct GitChangedFile {
    pub path: String,
    pub status: String, // "M", "A", "D", "??", "R"
    pub staged: bool,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct GitCommitSummary {
    pub sha: String,
    pub subject: String,
    pub author: String,
    pub relative_time: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct GitStatusSnapshot {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub is_clean: bool,
    pub changed_files: Vec<GitChangedFile>,
    pub total_additions: u32,
    pub total_deletions: u32,
    pub recent_commits: Vec<GitCommitSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct GitDiffPayload {
    pub project_id: String,
    pub target_path: Option<String>,
    pub unified_diff: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TestRunSnapshot {
    pub project_id: String,
    pub framework: String, // "cargo" | "npm" | "pytest" | "go" | "custom"
    pub command: String,   // e.g. "cargo test"
    pub status: String,    // "Idle" | "Running" | "Passed" | "Failed"
    pub passed_count: u32,
    pub failed_count: u32,
    pub duration_ms: u64,
    pub output_excerpt: String,
    pub last_run_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CiWorkflowRun {
    pub name: String,
    pub status: String,     // "completed" | "in_progress" | "queued"
    pub conclusion: String, // "success" | "failure" | "neutral"
    pub branch: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct EndpointProbeResult {
    pub name: String,
    pub url: String,
    pub status: String, // "HEALTHY" | "DEGRADED" | "DOWN"
    pub status_code: u16,
    pub latency_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DeployHealthSnapshot {
    pub project_id: String,
    pub pr_summary: Option<String>,
    pub pr_state: Option<String>,
    pub ci_runs: Vec<CiWorkflowRun>,
    pub endpoints: Vec<EndpointProbeResult>,
    pub release_ready: bool,
    pub checked_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct WorkspaceDevOpsSnapshot {
    pub project_id: String,
    pub git: GitStatusSnapshot,
    pub tests: TestRunSnapshot,
    pub deploy_health: DeployHealthSnapshot,
}
