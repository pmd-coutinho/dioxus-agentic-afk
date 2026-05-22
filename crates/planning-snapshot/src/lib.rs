//! **Planning Snapshot** normalization. Pure functions over **Source Issues**
//! producing the bucketed view the **Planning Phase** reads. Buckets defined
//! by ADR-0036.

use agentic_afk_contracts::{IssueSource, PlanningSnapshotResponse, SourceIssueSnapshot};

/// Raw inputs for [`normalize`] — the unbucketed **Source Issue** snapshot for
/// one **Project** plus its **Issue Source** sync metadata.
pub struct RawPlanningSnapshot {
    pub source: IssueSource,
    pub last_successful_sync_at: Option<String>,
    pub last_failure: Option<String>,
    pub issues: Vec<SourceIssueSnapshot>,
}

/// Bucket **Source Issues** into the **Planning Phase** view defined by ADR-0036.
///
/// Bucket rules:
/// - `non_ready`: `readiness != "ready"`
/// - `completed`: ready AND `lifecycle_status == "completed"`
/// - `active`: ready AND `lifecycle_status` in `{ "claimed", "running", "blocked" }`
/// - `dependency_blocked`: ready AND has at least one **Issue Dependency** whose id
///   is in the set of ready issue ids (distinct from Lifecycle Status `Blocked`,
///   which lands in `active` — see ADR-0036).
/// - `eligible`: ready AND none of the above
pub fn normalize(raw: RawPlanningSnapshot) -> PlanningSnapshotResponse {
    let RawPlanningSnapshot {
        source,
        last_successful_sync_at,
        last_failure,
        issues,
    } = raw;

    let ready_ids = issues
        .iter()
        .filter(|issue| issue.readiness == "ready")
        .map(|issue| issue.source_id.clone())
        .collect::<std::collections::HashSet<_>>();

    let mut non_ready = Vec::new();
    let mut dependency_blocked = Vec::new();
    let mut active = Vec::new();
    let mut completed = Vec::new();
    let mut eligible = Vec::new();

    for issue in issues {
        if issue.readiness != "ready" {
            non_ready.push(issue);
        } else if issue.lifecycle_status == "completed" {
            completed.push(issue);
        } else if matches!(
            issue.lifecycle_status.as_str(),
            "claimed" | "running" | "blocked"
        ) {
            active.push(issue);
        } else if issue
            .issue_dependencies
            .iter()
            .any(|dependency| ready_ids.contains(dependency))
        {
            dependency_blocked.push(issue);
        } else {
            eligible.push(issue);
        }
    }

    PlanningSnapshotResponse {
        source,
        last_successful_sync_at,
        last_failure,
        non_ready,
        dependency_blocked,
        active,
        completed,
        eligible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(
        source_id: &str,
        readiness: &str,
        lifecycle_status: &str,
        deps: &[&str],
    ) -> SourceIssueSnapshot {
        SourceIssueSnapshot {
            source_id: source_id.to_string(),
            title: format!("issue {source_id}"),
            readiness: readiness.to_string(),
            lifecycle_status: lifecycle_status.to_string(),
            parent_issue: None,
            issue_dependencies: deps.iter().map(|d| d.to_string()).collect(),
            source_order: 0,
            raw_text: String::new(),
        }
    }

    fn raw(issues: Vec<SourceIssueSnapshot>) -> RawPlanningSnapshot {
        RawPlanningSnapshot {
            source: IssueSource {
                kind: "github".to_string(),
                locator: "owner/repo".to_string(),
            },
            last_successful_sync_at: None,
            last_failure: None,
            issues,
        }
    }

    fn ids(issues: &[SourceIssueSnapshot]) -> Vec<&str> {
        issues.iter().map(|i| i.source_id.as_str()).collect()
    }

    #[test]
    fn ready_no_deps_is_eligible() {
        let out = normalize(raw(vec![issue("1", "ready", "ready", &[])]));
        assert_eq!(ids(&out.eligible), vec!["1"]);
        assert!(out.dependency_blocked.is_empty());
        assert!(out.active.is_empty());
        assert!(out.completed.is_empty());
        assert!(out.non_ready.is_empty());
    }

    #[test]
    fn ready_with_unresolved_dep_is_blocked() {
        let out = normalize(raw(vec![
            issue("1", "ready", "ready", &["2"]),
            issue("2", "ready", "ready", &[]),
        ]));
        assert_eq!(ids(&out.dependency_blocked), vec!["1"]);
        assert_eq!(ids(&out.eligible), vec!["2"]);
    }

    #[test]
    fn ready_lifecycle_completed_is_completed() {
        let out = normalize(raw(vec![issue("1", "ready", "completed", &[])]));
        assert_eq!(ids(&out.completed), vec!["1"]);
        assert!(out.eligible.is_empty());
    }

    #[test]
    fn ready_lifecycle_claimed_is_active() {
        let out = normalize(raw(vec![issue("1", "ready", "claimed", &[])]));
        assert_eq!(ids(&out.active), vec!["1"]);
    }

    #[test]
    fn ready_lifecycle_running_is_active() {
        let out = normalize(raw(vec![issue("1", "ready", "running", &[])]));
        assert_eq!(ids(&out.active), vec!["1"]);
    }

    #[test]
    fn ready_lifecycle_blocked_is_active_not_blocked_bucket() {
        let out = normalize(raw(vec![issue("1", "ready", "blocked", &[])]));
        assert_eq!(ids(&out.active), vec!["1"]);
        assert!(out.dependency_blocked.is_empty());
    }

    #[test]
    fn non_ready_is_non_ready() {
        let out = normalize(raw(vec![issue("1", "open", "ready", &[])]));
        assert_eq!(ids(&out.non_ready), vec!["1"]);
        assert!(out.eligible.is_empty());
    }

    #[test]
    fn dep_on_non_ready_issue_is_eligible() {
        // Dep id "2" exists but is not in the ready set, so "1" is eligible.
        let out = normalize(raw(vec![
            issue("1", "ready", "ready", &["2"]),
            issue("2", "open", "ready", &[]),
        ]));
        assert_eq!(ids(&out.eligible), vec!["1"]);
        assert_eq!(ids(&out.non_ready), vec!["2"]);
        assert!(out.dependency_blocked.is_empty());
    }

    #[test]
    fn multiple_buckets_in_one_call() {
        let out = normalize(raw(vec![
            issue("e", "ready", "ready", &[]),
            issue("b", "ready", "ready", &["e"]),
            issue("a", "ready", "running", &[]),
            issue("c", "ready", "completed", &[]),
            issue("n", "open", "ready", &[]),
        ]));
        assert_eq!(ids(&out.eligible), vec!["e"]);
        assert_eq!(ids(&out.dependency_blocked), vec!["b"]);
        assert_eq!(ids(&out.active), vec!["a"]);
        assert_eq!(ids(&out.completed), vec!["c"]);
        assert_eq!(ids(&out.non_ready), vec!["n"]);
    }

    #[test]
    fn preserves_source_metadata() {
        let r = RawPlanningSnapshot {
            source: IssueSource {
                kind: "github".to_string(),
                locator: "owner/repo".to_string(),
            },
            last_successful_sync_at: Some("2026-01-01T00:00:00Z".to_string()),
            last_failure: Some("boom".to_string()),
            issues: vec![],
        };
        let out = normalize(r);
        assert_eq!(out.source.kind, "github");
        assert_eq!(out.source.locator, "owner/repo");
        assert_eq!(
            out.last_successful_sync_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(out.last_failure.as_deref(), Some("boom"));
    }
}
