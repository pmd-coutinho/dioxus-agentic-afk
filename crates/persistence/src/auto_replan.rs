use agentic_afk_contracts::{AutoReplanState, PauseReason};

use crate::{Db, PersistenceError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutoReplanStatus {
    pub state: AutoReplanState,
    pub pause_reason: Option<PauseReason>,
}

pub struct AutoReplanStateStore<'a> {
    db: &'a Db,
}

impl<'a> AutoReplanStateStore<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    pub async fn get(&self, project_id: &str) -> Result<AutoReplanStatus, PersistenceError> {
        let row = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT auto_replan_state, auto_replan_pause_reason FROM projects WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(self.db)
        .await?;

        let Some((state, reason)) = row else {
            return Err(PersistenceError::NotFound(project_id.to_string()));
        };

        status_from_row(project_id, state, reason)
    }

    pub async fn set(
        &self,
        project_id: &str,
        state: AutoReplanState,
        pause_reason: Option<PauseReason>,
    ) -> Result<AutoReplanStatus, PersistenceError> {
        let result = sqlx::query(
            r#"
            UPDATE projects
            SET auto_replan_state = ?, auto_replan_pause_reason = ?
            WHERE id = ?
            "#,
        )
        .bind(state.as_wire())
        .bind(pause_reason.map(PauseReason::as_wire))
        .bind(project_id)
        .execute(self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PersistenceError::NotFound(project_id.to_string()));
        }

        self.get(project_id).await
    }
}

pub(crate) fn status_from_row(
    project_id: &str,
    state: String,
    reason: Option<String>,
) -> Result<AutoReplanStatus, PersistenceError> {
    let state = AutoReplanState::from_wire(&state).ok_or_else(|| {
        PersistenceError::InvalidAutoReplanState {
            project_id: project_id.to_string(),
            value: state.clone(),
        }
    })?;
    let pause_reason = reason
        .map(|raw| {
            PauseReason::from_wire(&raw).ok_or_else(|| PersistenceError::InvalidPauseReason {
                project_id: project_id.to_string(),
                value: raw,
            })
        })
        .transpose()?;

    if state != AutoReplanState::Paused && pause_reason.is_some() {
        return Err(PersistenceError::InvalidAutoReplanTransition(
            "pause reason is only valid when Auto-Replan is paused".to_string(),
        ));
    }
    if state == AutoReplanState::Paused && pause_reason.is_none() {
        return Err(PersistenceError::InvalidAutoReplanTransition(
            "paused Auto-Replan requires a pause reason".to_string(),
        ));
    }

    Ok(AutoReplanStatus {
        state,
        pause_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_afk_contracts::CreateProjectRequest;

    fn reasons_for(state: AutoReplanState) -> Vec<Option<PauseReason>> {
        match state {
            AutoReplanState::Paused => vec![
                Some(PauseReason::EmptyBacklog),
                Some(PauseReason::AssignmentBlocked),
                Some(PauseReason::PushNonFastForward),
                Some(PauseReason::MergeStagedLeft),
                Some(PauseReason::PlanningFailed),
                Some(PauseReason::SyncFailed),
            ],
            AutoReplanState::Off | AutoReplanState::Armed => vec![None],
        }
    }

    #[tokio::test]
    async fn round_trips_every_valid_state_reason_pair() {
        let db = crate::connect_in_memory().await.unwrap();
        crate::migrate(&db).await.unwrap();
        let dir = std::env::temp_dir().join(format!(
            "agentic-afk-auto-replan-store-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let project = crate::create_project(
            &db,
            &CreateProjectRequest {
                path: dir.to_string_lossy().into_owned(),
            },
        )
        .await
        .unwrap();
        let store = AutoReplanStateStore::new(&db);

        for state in [
            AutoReplanState::Off,
            AutoReplanState::Armed,
            AutoReplanState::Paused,
        ] {
            for reason in reasons_for(state) {
                let written = store.set(&project.id.0, state, reason).await.unwrap();
                assert_eq!(written.state, state);
                assert_eq!(written.pause_reason, reason);
                let read = store.get(&project.id.0).await.unwrap();
                assert_eq!(read, written);
            }
        }
    }

    #[test]
    fn rejects_unknown_state_and_reason_from_sql_boundary() {
        let state = status_from_row("p", "mystery".to_string(), None).unwrap_err();
        assert!(matches!(
            state,
            PersistenceError::InvalidAutoReplanState { .. }
        ));

        let reason =
            status_from_row("p", "paused".to_string(), Some("mystery".to_string())).unwrap_err();
        assert!(matches!(
            reason,
            PersistenceError::InvalidPauseReason { .. }
        ));
    }
}
