//! `DockerCodexRunner` (issue #74) — the production adapter that
//! implements all four Codex phase-runner traits (Planning,
//! Implementation, Review, Merge) backed by a `SandboxLauncher`.
//!
//! One instance is constructed per phase per Plan Run. The struct holds
//! the launcher, the resolved runtime image tag, the codex auth + config
//! bind-mount paths, and the project path that is always read-only mount
//! for the Planning phase. Per-Assignment phases (Implementation,
//! Review, Merge) take the worktree path from the `AssignmentContext`
//! passed per call so the same instance serves every assignment in a
//! Plan Run.
//!
//! See ADR-0041 for mount, label, env, and resource semantics.

use std::path::PathBuf;
use std::sync::Arc;

use crate::plan_run::{
    AssignmentContext, ImplementationPhaseRunner, MergePhaseRunner, PlanRunPhaseError,
    PlanningContext, PlanningPhaseRunner, ReviewPhaseRunner,
};
use crate::sandbox::{
    MISE_CACHE_VOLUME, SandboxLaunchSpec, SandboxLauncher, SandboxMount, SandboxPhase,
};

/// Single struct that fills any of the four Codex phase-runner slots in
/// `PlanRunDeps`. Production callers construct one per phase; the
/// `SandboxPhase` decides labels, mount semantics, and which
/// `PlanRunPhaseError` variant runner failures map to.
pub struct DockerCodexRunner {
    launcher: Arc<dyn SandboxLauncher>,
    phase: SandboxPhase,
    image_tag: String,
    project_path: PathBuf,
    codex_auth_path: PathBuf,
    codex_config_path: PathBuf,
    user: Option<(u32, u32)>,
}

impl DockerCodexRunner {
    /// Build a runner for one phase. `project_path` is the on-host
    /// Project worktree path; Planning bind-mounts it read-only at
    /// `/work`; the other phases ignore it and use the per-call
    /// Assignment Worktree path from `AssignmentContext`.
    pub fn new(
        launcher: Arc<dyn SandboxLauncher>,
        phase: SandboxPhase,
        image_tag: impl Into<String>,
        project_path: impl Into<PathBuf>,
        codex_auth_path: impl Into<PathBuf>,
        codex_config_path: impl Into<PathBuf>,
        user: Option<(u32, u32)>,
    ) -> Self {
        Self {
            launcher,
            phase,
            image_tag: image_tag.into(),
            project_path: project_path.into(),
            codex_auth_path: codex_auth_path.into(),
            codex_config_path: codex_config_path.into(),
            user,
        }
    }

    fn standard_labels(
        &self,
        plan_run_id: &str,
        project_id: &str,
        assignment_id: Option<&str>,
        attempt_id: Option<&str>,
    ) -> Vec<(String, String)> {
        let mut labels = vec![
            (
                "agentic-afk.plan-run-id".to_string(),
                plan_run_id.to_string(),
            ),
            ("agentic-afk.project-id".to_string(), project_id.to_string()),
            (
                "agentic-afk.phase".to_string(),
                self.phase.as_label().to_string(),
            ),
        ];
        labels.push((
            "agentic-afk.issue-assignment-id".to_string(),
            assignment_id.unwrap_or("").to_string(),
        ));
        labels.push((
            "agentic-afk.assignment-attempt-id".to_string(),
            attempt_id.unwrap_or("").to_string(),
        ));
        labels
    }

    fn build_spec_planning(
        &self,
        prompt: &str,
        plan_run_id: &str,
        project_id: &str,
    ) -> SandboxLaunchSpec {
        let (memory, cpus, pids) = SandboxLaunchSpec::default_caps();
        SandboxLaunchSpec {
            image_tag: self.image_tag.clone(),
            container_name: format!("agentic-afk-planning-{plan_run_id}"),
            phase: self.phase,
            labels: self.standard_labels(plan_run_id, project_id, None, None),
            mounts: self.mounts(self.project_path.clone(), true),
            workdir: PathBuf::from("/work"),
            env: vec![("HOME".to_string(), "/tmp/codex-home".to_string())],
            command: codex_exec_command(prompt, self.phase.codex_model()),
            memory,
            cpus,
            pids_limit: pids,
            user: self.user,
        }
    }

    fn build_spec_assignment(
        &self,
        prompt: &str,
        context: &AssignmentContext<'_>,
    ) -> SandboxLaunchSpec {
        let (memory, cpus, pids) = SandboxLaunchSpec::default_caps();
        let attempt_id = context
            .assignment
            .latest_attempt
            .as_ref()
            .map(|a| a.id.as_str());
        let labels = self.standard_labels(
            &context.plan_run.id,
            context.project.id.0.as_str(),
            Some(&context.assignment.id),
            attempt_id,
        );
        SandboxLaunchSpec {
            image_tag: self.image_tag.clone(),
            container_name: format!(
                "agentic-afk-assignment-{}-{}",
                context.assignment.id,
                self.phase.as_label()
            ),
            phase: self.phase,
            labels,
            mounts: self.mounts(PathBuf::from(&context.assignment.worktree_path), false),
            workdir: PathBuf::from("/work"),
            env: vec![("HOME".to_string(), "/tmp/codex-home".to_string())],
            command: codex_exec_command(prompt, self.phase.codex_model()),
            memory,
            cpus,
            pids_limit: pids,
            user: self.user,
        }
    }

    fn mounts(&self, work_host: PathBuf, work_read_only: bool) -> Vec<SandboxMount> {
        vec![
            SandboxMount::Bind {
                host_path: work_host,
                container_path: PathBuf::from("/work"),
                read_only: work_read_only,
            },
            SandboxMount::Bind {
                host_path: self.codex_auth_path.clone(),
                container_path: PathBuf::from("/tmp/codex-home/.codex/auth.json"),
                read_only: false,
            },
            SandboxMount::Bind {
                host_path: self.codex_config_path.clone(),
                container_path: PathBuf::from("/tmp/codex-home/.codex/config.toml"),
                read_only: true,
            },
            SandboxMount::Volume {
                name: MISE_CACHE_VOLUME.to_string(),
                container_path: PathBuf::from("/cache/mise"),
                read_only: false,
            },
        ]
    }
}

struct CodexModelConfig {
    model: &'static str,
    reasoning_effort: &'static str,
}

trait CodexPhaseModel {
    fn codex_model(self) -> CodexModelConfig;
}

impl CodexPhaseModel for SandboxPhase {
    fn codex_model(self) -> CodexModelConfig {
        match self {
            SandboxPhase::Implementation => CodexModelConfig {
                model: "gpt-5.4",
                reasoning_effort: "medium",
            },
            SandboxPhase::Planning | SandboxPhase::Review | SandboxPhase::Merge => {
                CodexModelConfig {
                    model: "gpt-5.5",
                    reasoning_effort: "medium",
                }
            }
        }
    }
}

fn codex_exec_command(prompt: &str, model: CodexModelConfig) -> Vec<String> {
    let script = format!(
        r#"set -euo pipefail
last_message="$(mktemp /tmp/agentic-afk-last-message.XXXXXX)"
transcript="$(mktemp /tmp/agentic-afk-codex-transcript.XXXXXX)"
codex exec --model {model_name} -c 'model_reasoning_effort="{reasoning_effort}"' --dangerously-bypass-approvals-and-sandbox --output-last-message "$last_message" "$1" >"$transcript"
cat "$last_message""#,
        model_name = model.model,
        reasoning_effort = model.reasoning_effort,
    );
    vec![
        "bash".to_string(),
        "-lc".to_string(),
        script,
        "agentic-afk-codex-exec".to_string(),
        prompt.to_string(),
    ]
}

impl PlanningPhaseRunner for DockerCodexRunner {
    fn run(
        &self,
        prompt: &str,
        context: &PlanningContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        // Planning is invoked once per Plan Run before any assignment
        // exists, so the labels carry empty assignment/attempt slots.
        let spec =
            self.build_spec_planning(prompt, &context.plan_run.id, context.project.id.0.as_str());
        self.launcher
            .launch(spec, context.process_recorder)
            .map_err(|e| PlanRunPhaseError::Planning(e.to_string()))
    }
}

impl ImplementationPhaseRunner for DockerCodexRunner {
    fn run(
        &self,
        prompt: &str,
        context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        let spec = self.build_spec_assignment(prompt, context);
        self.launcher
            .launch(spec, context.process_recorder)
            .map_err(|e| PlanRunPhaseError::Implementation(e.to_string()))
    }
}

impl ReviewPhaseRunner for DockerCodexRunner {
    fn run(
        &self,
        prompt: &str,
        context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        let spec = self.build_spec_assignment(prompt, context);
        self.launcher
            .launch(spec, context.process_recorder)
            .map_err(|e| PlanRunPhaseError::Review(e.to_string()))
    }
}

impl MergePhaseRunner for DockerCodexRunner {
    fn run(
        &self,
        prompt: &str,
        context: &AssignmentContext<'_>,
    ) -> Result<String, PlanRunPhaseError> {
        let spec = self.build_spec_assignment(prompt, context);
        self.launcher
            .launch(spec, context.process_recorder)
            .map_err(|e| PlanRunPhaseError::Merge(e.to_string()))
    }
}
