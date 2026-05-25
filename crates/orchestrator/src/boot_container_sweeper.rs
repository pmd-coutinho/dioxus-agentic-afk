//! Boot Codex Sandbox container sweeper (issue #75 / ADR-0041).
//!
//! Runs once at server boot, after the DB-side
//! [`crate::boot_recovery_scanner`] from ADR-0042 and before HTTP bind.
//! Reconciles any Codex Sandbox containers left running by a previous
//! orchestrator process: kills + removes mid-flight containers, blocks
//! the owning Issue Assignment with
//! `BlockReason::OrchestratorRestart`, and removes containers whose
//! owner is already terminal.
//!
//! Docker daemon unavailable is **warn-not-fatal**: the read-only
//! dashboard still serves; the next Plan Run trigger fails preflight
//! with `urn:agentic-afk:sandbox-docker-unavailable` (issue #73) until
//! Docker is back. DB errors are fatal — partial recovery is worse than
//! none.

use std::path::PathBuf;
use std::process::Command;

use agentic_afk_contracts::BlockReason;
use agentic_afk_persistence::{self as persistence, Db, PersistenceError};

use crate::coordinator::EventPublisher;
use crate::sandbox::SandboxPhase;

/// One container observed by the sweeper. Identified by name and tagged
/// with the labels the orchestrator sets in
/// [`crate::codex_runner::DockerCodexRunner`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxContainer {
    pub name: String,
    pub phase: SandboxPhase,
    pub plan_run_id: String,
    pub assignment_id: Option<String>,
    pub project_id: Option<String>,
}

/// Narrow Docker-CLI seam the sweeper drives. Production wires
/// [`CliDockerOps`]; tests inject fakes that script container lists and
/// record kill/rm calls.
pub trait DockerContainerOps: Send + Sync {
    /// List containers carrying `label_key` (typically
    /// `agentic-afk.plan-run-id`). Returns Err if the Docker daemon is
    /// unreachable.
    fn list_with_label(&self, label_key: &str) -> Result<Vec<SandboxContainer>, String>;
    fn kill(&self, container_name: &str) -> Result<(), String>;
    fn rm_force(&self, container_name: &str) -> Result<(), String>;
}

/// What the sweeper did. Returned for INFO logging at the call site and
/// for integration-test observation.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct SandboxSweepReport {
    pub containers_inspected: usize,
    pub containers_killed: usize,
    pub containers_removed: usize,
    pub assignments_blocked: usize,
    pub docker_unavailable: bool,
}

const SWEEP_LABEL: &str = "agentic-afk.plan-run-id";

/// Run the sweep once. Synchronous in effect: returns when every
/// observed container has been disposed of and the corresponding
/// assignment transitions have been written.
pub async fn run(
    db: &Db,
    events: &dyn EventPublisher,
    docker: &dyn DockerContainerOps,
) -> Result<SandboxSweepReport, PersistenceError> {
    let containers = match docker.list_with_label(SWEEP_LABEL) {
        Ok(list) => list,
        Err(err) => {
            eprintln!(
                "boot container sweep: docker daemon unavailable ({err}); skipping. The next Plan Run trigger will fail preflight until Docker is back."
            );
            return Ok(SandboxSweepReport {
                docker_unavailable: true,
                ..SandboxSweepReport::default()
            });
        }
    };

    let mut report = SandboxSweepReport {
        containers_inspected: containers.len(),
        ..SandboxSweepReport::default()
    };

    for container in containers {
        eprintln!(
            "boot container sweep: handling container {} (phase={:?}, plan_run={}, assignment={:?})",
            container.name, container.phase, container.plan_run_id, container.assignment_id
        );
        process_container(db, events, docker, &container, &mut report).await?;
    }

    eprintln!(
        "boot container sweep: inspected {} container(s); killed {}; removed {}; blocked {} assignment(s)",
        report.containers_inspected,
        report.containers_killed,
        report.containers_removed,
        report.assignments_blocked,
    );
    Ok(report)
}

async fn process_container(
    db: &Db,
    events: &dyn EventPublisher,
    docker: &dyn DockerContainerOps,
    container: &SandboxContainer,
    report: &mut SandboxSweepReport,
) -> Result<(), PersistenceError> {
    let owner_terminal = owner_already_terminal(db, container).await?;
    if owner_terminal {
        if let Err(err) = docker.rm_force(&container.name) {
            eprintln!(
                "boot container sweep: docker rm -f {} failed: {err}",
                container.name
            );
        } else {
            report.containers_removed += 1;
        }
        return Ok(());
    }

    if let Err(err) = docker.kill(&container.name) {
        eprintln!(
            "boot container sweep: docker kill {} failed: {err}",
            container.name
        );
    } else {
        report.containers_killed += 1;
    }
    if let Err(err) = docker.rm_force(&container.name) {
        eprintln!(
            "boot container sweep: docker rm -f {} failed: {err}",
            container.name
        );
    } else {
        report.containers_removed += 1;
    }

    // Planning containers carry no assignment; the DB-side recovery
    // scanner (ADR-0042) handles the Plan Run terminal transition.
    let Some(assignment_id) = container.assignment_id.as_deref() else {
        return Ok(());
    };

    let assignment = persistence::get_issue_assignment_public(db, assignment_id).await?;
    if matches!(
        assignment.status.as_str(),
        "merged" | "blocked" | "merge_staged"
    ) {
        // Idempotent: the DB-side scanner may have already blocked this
        // assignment, or the merge already succeeded.
        return Ok(());
    }

    let blocked = persistence::record_blocked_with_kind(
        db,
        assignment_id,
        BlockReason::OrchestratorRestart,
        Some(&format!(
            "orchestrator restarted; Codex Sandbox container {} was killed during {} phase",
            container.name,
            container.phase.as_label()
        )),
    )
    .await?;
    let project_id = blocked.project_id.0.clone();
    events.assignment_status_changed(&project_id, blocked.clone());
    events.record_activity(
        &project_id,
        Some(&blocked.id),
        crate::boot_recovery_scanner::ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART,
        Some(&format!(
            "blocked on orchestrator restart during {} phase (container={})",
            container.phase.as_label(),
            container.name
        )),
    );
    let _ = persistence::record_project_activity(
        db,
        &project_id,
        Some(&blocked.id),
        crate::boot_recovery_scanner::ACTIVITY_KIND_ASSIGNMENT_BLOCKED_ON_RESTART,
        Some(&format!(
            "blocked on orchestrator restart during {} phase (container={})",
            container.phase.as_label(),
            container.name
        )),
    )
    .await;
    report.assignments_blocked += 1;
    Ok(())
}

async fn owner_already_terminal(
    db: &Db,
    container: &SandboxContainer,
) -> Result<bool, PersistenceError> {
    if let Some(assignment_id) = container.assignment_id.as_deref() {
        let assignment = match persistence::get_issue_assignment_public(db, assignment_id).await {
            Ok(a) => a,
            Err(PersistenceError::NotFound(_) | PersistenceError::AssignmentNotFound(_)) => {
                return Ok(true);
            }
            Err(err) => return Err(err),
        };
        Ok(matches!(assignment.status.as_str(), "merged"))
    } else {
        // Planning container: terminal iff the Plan Run is no longer
        // running. The DB-side recovery scanner finishes such Plan Runs
        // before this sweep, so a planning container whose Plan Run is
        // terminal at this point is genuinely orphaned.
        let plan_run = match persistence::get_plan_run(db, &container.plan_run_id).await {
            Ok(run) => run,
            Err(PersistenceError::NotFound(_)) => return Ok(true),
            Err(err) => return Err(err),
        };
        Ok(plan_run.state != agentic_afk_contracts::PlanRunState::Running)
    }
}

/// Production [`DockerContainerOps`] backed by the local `docker` CLI.
pub struct CliDockerOps {
    docker_binary: PathBuf,
}

impl CliDockerOps {
    pub fn new(docker_binary: impl Into<PathBuf>) -> Self {
        Self {
            docker_binary: docker_binary.into(),
        }
    }
}

impl DockerContainerOps for CliDockerOps {
    fn list_with_label(&self, label_key: &str) -> Result<Vec<SandboxContainer>, String> {
        let format = r#"{{.Names}}\t{{.Label "agentic-afk.plan-run-id"}}\t{{.Label "agentic-afk.phase"}}\t{{.Label "agentic-afk.issue-assignment-id"}}\t{{.Label "agentic-afk.project-id"}}"#;
        let output = Command::new(&self.docker_binary)
            .args([
                "ps",
                "-a",
                "--filter",
                &format!("label={label_key}"),
                "--format",
                format,
            ])
            .output()
            .map_err(|e| format!("docker ps failed to spawn: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "docker ps exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let mut containers = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 3 {
                continue;
            }
            let phase = match cols[2] {
                "planning" => SandboxPhase::Planning,
                "implementation" => SandboxPhase::Implementation,
                "review" => SandboxPhase::Review,
                "merge" => SandboxPhase::Merge,
                other => {
                    eprintln!(
                        "boot container sweep: skipping container {} with unknown phase label '{other}'",
                        cols[0]
                    );
                    continue;
                }
            };
            containers.push(SandboxContainer {
                name: cols[0].to_string(),
                phase,
                plan_run_id: cols[1].to_string(),
                assignment_id: cols.get(3).filter(|s| !s.is_empty()).map(|s| s.to_string()),
                project_id: cols.get(4).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            });
        }
        Ok(containers)
    }

    fn kill(&self, container_name: &str) -> Result<(), String> {
        let output = Command::new(&self.docker_binary)
            .args(["kill", container_name])
            .output()
            .map_err(|e| format!("docker kill failed to spawn: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "docker kill {container_name}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    }

    fn rm_force(&self, container_name: &str) -> Result<(), String> {
        let output = Command::new(&self.docker_binary)
            .args(["rm", "-f", container_name])
            .output()
            .map_err(|e| format!("docker rm -f failed to spawn: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "docker rm -f {container_name}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot_recovery_scanner::NoopEventPublisher;
    use agentic_afk_contracts::{CreateProjectRequest, IssueSource, SourceIssueSnapshot};
    use agentic_afk_persistence::{
        connect_in_memory, create_plan_run, create_plan_run_assignment, create_project, migrate,
        set_assignment_status,
    };
    use std::sync::Mutex;

    struct FakeDocker {
        containers: Mutex<Vec<SandboxContainer>>,
        listed_error: Mutex<Option<String>>,
        killed: Mutex<Vec<String>>,
        removed: Mutex<Vec<String>>,
    }

    impl FakeDocker {
        fn with_containers(containers: Vec<SandboxContainer>) -> Self {
            Self {
                containers: Mutex::new(containers),
                listed_error: Mutex::new(None),
                killed: Mutex::new(Vec::new()),
                removed: Mutex::new(Vec::new()),
            }
        }

        fn unavailable() -> Self {
            Self {
                containers: Mutex::new(Vec::new()),
                listed_error: Mutex::new(Some("Cannot connect to the Docker daemon".into())),
                killed: Mutex::new(Vec::new()),
                removed: Mutex::new(Vec::new()),
            }
        }
    }

    impl DockerContainerOps for FakeDocker {
        fn list_with_label(&self, _label_key: &str) -> Result<Vec<SandboxContainer>, String> {
            if let Some(err) = self.listed_error.lock().unwrap().as_ref() {
                return Err(err.clone());
            }
            Ok(self.containers.lock().unwrap().clone())
        }

        fn kill(&self, container_name: &str) -> Result<(), String> {
            self.killed.lock().unwrap().push(container_name.to_string());
            Ok(())
        }

        fn rm_force(&self, container_name: &str) -> Result<(), String> {
            self.removed
                .lock()
                .unwrap()
                .push(container_name.to_string());
            Ok(())
        }
    }

    async fn seed_assignment(db: &Db, status: &str) -> (String, String, String) {
        let path = std::env::temp_dir().join(format!(
            "agentic-afk-sweep-{}-{}",
            std::process::id(),
            uuid()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let project = create_project(
            db,
            &CreateProjectRequest {
                path: path.to_string_lossy().into_owned(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(db, &project.id.0, "main", "baseline-sha")
            .await
            .unwrap();
        let snapshot = SourceIssueSnapshot {
            source_id: "issue-1".to_string(),
            title: "t".to_string(),
            readiness: "ready".to_string(),
            lifecycle_status: "ready".to_string(),
            parent_issue: None,
            issue_dependencies: Vec::new(),
            source_order: 0,
            raw_text: String::new(),
        };
        let source = IssueSource {
            kind: "local_markdown".to_string(),
            locator: "issues".to_string(),
        };
        let assignment = create_plan_run_assignment(
            db,
            &plan_run.id,
            &project.id.0,
            &source,
            &snapshot,
            "agent/issue-1",
            "stub",
        )
        .await
        .unwrap();
        set_assignment_status(db, &assignment.id, status, None)
            .await
            .unwrap();
        (project.id.0, plan_run.id, assignment.id)
    }

    fn uuid() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
    }

    #[tokio::test]
    async fn implementation_container_with_non_terminal_owner_is_killed_removed_and_assignment_blocked()
     {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let (_project_id, plan_run_id, assignment_id) = seed_assignment(&db, "implementing").await;

        let container = SandboxContainer {
            name: format!("agentic-afk-assignment-{assignment_id}-implementation"),
            phase: SandboxPhase::Implementation,
            plan_run_id: plan_run_id.clone(),
            assignment_id: Some(assignment_id.clone()),
            project_id: None,
        };
        let docker = FakeDocker::with_containers(vec![container.clone()]);
        let report = run(&db, &NoopEventPublisher, &docker).await.unwrap();

        assert_eq!(report.containers_inspected, 1);
        assert_eq!(report.containers_killed, 1);
        assert_eq!(report.containers_removed, 1);
        assert_eq!(report.assignments_blocked, 1);
        assert!(docker.killed.lock().unwrap().contains(&container.name));
        assert!(docker.removed.lock().unwrap().contains(&container.name));

        let assignment = persistence::get_issue_assignment_public(&db, &assignment_id)
            .await
            .unwrap();
        assert_eq!(assignment.status, "blocked");
        let kind = assignment.block_reason.as_ref().map(|r| r.kind);
        assert_eq!(kind, Some(BlockReason::OrchestratorRestart));
    }

    #[tokio::test]
    async fn container_with_terminal_owner_is_removed_without_db_transition() {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let (_project_id, plan_run_id, assignment_id) = seed_assignment(&db, "merged").await;

        let container = SandboxContainer {
            name: "agentic-afk-leftover".to_string(),
            phase: SandboxPhase::Review,
            plan_run_id,
            assignment_id: Some(assignment_id.clone()),
            project_id: None,
        };
        let docker = FakeDocker::with_containers(vec![container.clone()]);
        let report = run(&db, &NoopEventPublisher, &docker).await.unwrap();

        assert_eq!(report.containers_inspected, 1);
        assert_eq!(report.containers_killed, 0);
        assert_eq!(report.containers_removed, 1);
        assert_eq!(report.assignments_blocked, 0);
        assert!(docker.killed.lock().unwrap().is_empty());
        assert!(docker.removed.lock().unwrap().contains(&container.name));

        let assignment = persistence::get_issue_assignment_public(&db, &assignment_id)
            .await
            .unwrap();
        assert_eq!(assignment.status, "merged");
    }

    #[tokio::test]
    async fn planning_container_is_killed_and_removed() {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let (_project_id, plan_run_id, _assignment_id) = seed_assignment(&db, "implementing").await;

        let container = SandboxContainer {
            name: format!("agentic-afk-planning-{plan_run_id}"),
            phase: SandboxPhase::Planning,
            plan_run_id,
            assignment_id: None,
            project_id: None,
        };
        let docker = FakeDocker::with_containers(vec![container.clone()]);
        let report = run(&db, &NoopEventPublisher, &docker).await.unwrap();

        assert_eq!(report.containers_killed, 1);
        assert_eq!(report.containers_removed, 1);
        assert!(docker.killed.lock().unwrap().contains(&container.name));
    }

    #[tokio::test]
    async fn second_sweep_is_idempotent_no_double_block() {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let (_project_id, plan_run_id, assignment_id) = seed_assignment(&db, "implementing").await;

        let container = SandboxContainer {
            name: format!("agentic-afk-assignment-{assignment_id}-implementation"),
            phase: SandboxPhase::Implementation,
            plan_run_id,
            assignment_id: Some(assignment_id.clone()),
            project_id: None,
        };
        let docker_first = FakeDocker::with_containers(vec![container.clone()]);
        let _ = run(&db, &NoopEventPublisher, &docker_first).await.unwrap();

        // Simulate a second boot where the same container exists in `ps -a`
        // (rm failed) and the DB already shows the assignment as blocked.
        let docker_second = FakeDocker::with_containers(vec![container.clone()]);
        let report = run(&db, &NoopEventPublisher, &docker_second).await.unwrap();
        assert_eq!(report.assignments_blocked, 0, "no double block on rerun");
        assert_eq!(report.containers_removed, 1, "still removes leftover");
    }

    #[tokio::test]
    async fn docker_unavailable_returns_warn_report_without_db_writes() {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let (_, _, assignment_id) = seed_assignment(&db, "implementing").await;
        let docker = FakeDocker::unavailable();

        let report = run(&db, &NoopEventPublisher, &docker).await.unwrap();
        assert!(report.docker_unavailable);
        assert_eq!(report.containers_inspected, 0);
        assert_eq!(report.assignments_blocked, 0);

        let assignment = persistence::get_issue_assignment_public(&db, &assignment_id)
            .await
            .unwrap();
        assert_eq!(
            assignment.status, "implementing",
            "docker outage must not touch DB state"
        );
    }
}
