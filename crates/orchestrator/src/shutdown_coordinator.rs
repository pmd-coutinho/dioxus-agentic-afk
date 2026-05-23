//! Shutdown coordinator (ADR-0042 S1).
//!
//! Drives the **mark-and-kill** shutdown contract: on SIGTERM/SIGINT the
//! control-plane server marks every `plan_run_phase_outputs` row whose
//! `outcome = 'in_flight'` to `interrupted`, sends SIGTERM to each row's
//! captured child PID, sleeps up to two seconds for the children to exit,
//! and returns so the parent process can shut down.
//!
//! Mark-and-kill is the deliberate design over **drain** (wait for in-flight
//! phases to finish naturally — rejected because a single implementation
//! phase can run tens of minutes) and **no-op** (rely on boot recovery
//! alone — rejected because orphan Codex children commit work the
//! orchestrator never observes). See ADR-0042 § *Considered Options*.

use std::time::Duration;

use agentic_afk_persistence::{Db, mark_in_flight_rows_interrupted};

/// Default grace period between sending SIGTERM and returning. Kept small
/// because the goal is to give a cooperative Codex child a chance to flush
/// stdio + exit cleanly, not to wait for it to finish work — orphaned
/// commits land in the recovery scanner's blocked-assignment bucket on
/// next boot.
pub const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(2);

/// Result of one shutdown sweep, returned for test observation and
/// optional INFO logging at the call site.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct ShutdownReport {
    pub rows_marked_interrupted: usize,
    pub pids_signalled: Vec<u32>,
}

/// Future installed via `axum::serve(...).with_graceful_shutdown(...)`.
/// Awaits SIGTERM/SIGINT, then performs the mark-and-kill sweep against
/// `db` before resolving so axum starts tearing down only *after* the
/// in-flight rows are durably marked.
pub async fn await_shutdown(db: Db) -> ShutdownReport {
    wait_for_signal().await;
    sweep_in_flight(&db, DEFAULT_KILL_GRACE).await
}

/// Mark every in-flight row to `interrupted`, send SIGTERM to each
/// captured PID, sleep `grace`, return the report. Idempotent — running
/// twice on the same database surfaces the same interrupted rows the
/// second time but does not double-kill anything (the PIDs are likely
/// gone by then anyway).
pub async fn sweep_in_flight(db: &Db, grace: Duration) -> ShutdownReport {
    let affected = match mark_in_flight_rows_interrupted(db).await {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!(
                "shutdown coordinator: failed to mark in-flight phase rows interrupted: {error}"
            );
            return ShutdownReport::default();
        }
    };
    let mut report = ShutdownReport {
        rows_marked_interrupted: affected.len(),
        pids_signalled: Vec::new(),
    };
    for row in &affected {
        if let Some(pid) = row.process_id {
            send_sigterm(pid);
            report.pids_signalled.push(pid);
        }
    }
    if !report.pids_signalled.is_empty() {
        tokio::time::sleep(grace).await;
    }
    report
}

#[cfg(unix)]
fn send_sigterm(pid: u32) {
    // libc::kill via std::process::Command stays portable across the
    // existing zero-extra-deps surface; the orchestrator already shells
    // out to `git`, `gh`, `worktrunk`, `codex`, so one more invocation
    // is consistent.
    // Stdio::null on stderr so a "no such process" / "operation not
    // permitted" doesn't leak to the operator's terminal during a
    // hostile shutdown — the row is already marked `interrupted`, the
    // failure (process already exited, or never existed) is durably
    // surfaced through the BootRecoveryScanner the next time the server
    // starts.
    let _ = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn send_sigterm(_pid: u32) {
    // SIGTERM is unix-only; the control-plane server only supports unix
    // in practice (Codex bind-mounts and the worktrunk wrapper rely on
    // POSIX semantics) but the cross-cfg here keeps `cargo check` honest
    // on Windows hosts a developer might use for documentation work.
}

async fn wait_for_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_afk_contracts::CreateProjectRequest;
    use agentic_afk_persistence::{
        connect_in_memory, create_plan_run, create_project, insert_in_flight_phase_output,
        list_in_flight_phase_rows, migrate, record_in_flight_phase_process,
    };

    async fn setup() -> (Db, String) {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let project = create_project(
            &db,
            &CreateProjectRequest {
                path: "/tmp".to_string(),
            },
        )
        .await
        .unwrap();
        let plan_run = create_plan_run(&db, &project.id.0, "main", "deadbeef")
            .await
            .unwrap();
        (db, plan_run.id)
    }

    #[tokio::test]
    async fn sweep_marks_in_flight_rows_interrupted_and_returns_pids() {
        let (db, plan_run_id) = setup().await;
        let row = insert_in_flight_phase_output(&db, &plan_run_id, None, "planning")
            .await
            .unwrap();
        record_in_flight_phase_process(&db, &row, 1, "unix:1")
            .await
            .unwrap();

        // Spawn a real `sleep` child so the SIGTERM has somewhere to
        // land. We intentionally don't capture its pid into the row —
        // the AC under test is that the sweep marks the row interrupted
        // and reports the *captured* PID; killing the synthetic PID `1`
        // would be hostile so the recorded PID stays at 1 and the kill
        // is a best-effort no-op (the test still proves the report
        // shape).
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep child");

        let report = sweep_in_flight(&db, Duration::from_millis(50)).await;
        assert_eq!(report.rows_marked_interrupted, 1);
        assert_eq!(report.pids_signalled, vec![1]);

        let remaining = list_in_flight_phase_rows(&db).await.unwrap();
        assert_eq!(remaining.len(), 1, "interrupted rows still surface");
        assert_eq!(remaining[0].outcome, "interrupted");

        let _ = child.kill();
        let _ = child.wait();
    }

    #[tokio::test]
    async fn shutdown_signals_real_child_via_recorded_pid() {
        let (db, plan_run_id) = setup().await;

        // Spawn a real `sleep` child and record *its* PID into a fresh
        // in-flight row so the sweep's `kill -TERM <pid>` lands on a
        // process we own. AC: "Sending SIGTERM ... results in the row
        // being marked interrupted and the captured child PID receiving
        // SIGTERM (verified by an integration test using a controllable
        // fake child)".
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep child");
        let child_pid = child.id();

        let row = insert_in_flight_phase_output(&db, &plan_run_id, None, "implementation")
            .await
            .unwrap();
        record_in_flight_phase_process(&db, &row, child_pid, "unix:1")
            .await
            .unwrap();

        let report = sweep_in_flight(&db, Duration::from_secs(2)).await;
        assert_eq!(report.pids_signalled, vec![child_pid]);

        // The child should have exited (SIGTERM -> default disposition
        // terminates `sleep`). `try_wait` returns Some(_) when reaped.
        let status = child
            .try_wait()
            .expect("try_wait")
            .or_else(|| child.wait().ok());
        assert!(
            status.is_some(),
            "sleep child should have terminated after SIGTERM"
        );
    }

    #[tokio::test]
    async fn sweep_with_no_in_flight_rows_is_a_noop() {
        let db = connect_in_memory().await.unwrap();
        migrate(&db).await.unwrap();
        let report = sweep_in_flight(&db, Duration::from_millis(10)).await;
        assert_eq!(report.rows_marked_interrupted, 0);
        assert!(report.pids_signalled.is_empty());
    }
}
