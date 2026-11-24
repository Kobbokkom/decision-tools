use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use sqlx::{
    FromRow, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Store(SqlitePool);

// The queued row is the durable intent for its next step.
#[derive(FromRow)]
pub struct Claim {
    pub id: String,
    pub workflow: String,
    pub input: String,
    pub next_step: String,
    pub results: String,
}

#[derive(FromRow)]
struct Run {
    id: String,
    workflow: String,
    status: String,
    next_step: Option<String>,
    results: String,
    error: Option<String>,
    created_at: i64,
    updated_at: i64,
    schedule: Option<String>,
    scheduled_at: Option<i64>,
}

impl Store {
    pub async fn open(path: &Path, revision: &str) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .context("open workflow database")?;
        sqlx::migrate!()
            .run(&pool)
            .await
            .context("migrate workflow database")?;
        sqlx::query(
            "UPDATE runs SET status = 'interrupted', next_step = NULL, updated_at = unixepoch(),
                error = CASE WHEN status = 'queued' THEN 'workflow configuration changed'
                    ELSE 'service stopped while the step was in flight' END
             WHERE status IN ('running', 'cancelling') OR (status = 'queued' AND revision <> ?)",
        )
        .bind(revision)
        .execute(&pool)
        .await?;
        Ok(Self(pool))
    }

    pub async fn ready(&self) -> Result<()> {
        sqlx::query("SELECT 1 FROM runs LIMIT 1")
            .execute(&self.0)
            .await?;
        Ok(())
    }

    pub async fn insert(
        &self,
        name: &str,
        revision: &str,
        entry: &str,
        input: &Map<String, Value>,
        scheduled: Option<(&str, i64)>,
    ) -> Result<Option<String>> {
        let id = Uuid::new_v4().to_string();
        let inserted = sqlx::query_scalar(
            "INSERT INTO runs (id, workflow, revision, input, status, next_step, schedule, scheduled_at)
             SELECT ?, ?, ?, ?, 'queued', ?, ?, ?
             WHERE (SELECT count(*) FROM runs WHERE status IN ('queued', 'running', 'cancelling')) < 128
             ON CONFLICT(schedule, scheduled_at) DO NOTHING RETURNING id",
        )
        .bind(id).bind(name).bind(revision).bind(serde_json::to_string(input)?).bind(entry)
        .bind(scheduled.map(|slot| slot.0)).bind(scheduled.map(|slot| slot.1))
        .fetch_optional(&self.0).await?;
        Ok(inserted)
    }

    pub async fn latest_scheduled(&self, schedule: &str) -> Result<Option<String>> {
        let id = sqlx::query_scalar(
            "SELECT id FROM runs WHERE schedule = ? ORDER BY scheduled_at DESC LIMIT 1",
        )
        .bind(schedule)
        .fetch_optional(&self.0)
        .await?;
        Ok(id)
    }

    pub async fn get(&self, id: &str) -> Result<Value> {
        let run = sqlx::query_as::<_, Run>(
            "SELECT id, workflow, status, next_step, results, error, created_at, updated_at, schedule, scheduled_at
             FROM runs WHERE id = ?",
        ).bind(id).fetch_optional(&self.0).await?.context("unknown run")?;
        let results: Value = serde_json::from_str(&run.results)?;
        Ok(json!({
            "id":run.id, "workflow":run.workflow, "status":run.status, "next_step":run.next_step,
            "results":results, "error":run.error, "created_at":run.created_at, "updated_at":run.updated_at,
            "schedule":run.schedule, "scheduled_at":run.scheduled_at
        }))
    }

    pub async fn cancel(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE runs SET status = CASE WHEN status = 'running' THEN 'cancelling' ELSE 'cancelled' END,
                next_step = CASE WHEN status = 'running' THEN next_step ELSE NULL END, updated_at = unixepoch()
             WHERE id = ? AND status IN ('queued', 'running')",
        ).bind(id).execute(&self.0).await?;
        Ok(())
    }

    pub async fn claim(&self) -> Result<Option<Claim>> {
        let claim = sqlx::query_as(
            "UPDATE runs SET status = 'running', updated_at = unixepoch()
             WHERE id = (SELECT id FROM runs WHERE status = 'queued' ORDER BY created_at, id LIMIT 1)
             RETURNING id, workflow, input, next_step, results",
        ).fetch_optional(&self.0).await?;
        Ok(claim)
    }

    pub async fn finish(
        &self,
        id: &str,
        status: &str,
        next: Option<&str>,
        results: &Map<String, Value>,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE runs SET status = CASE WHEN status = 'cancelling' THEN 'cancelled' ELSE ? END,
                next_step = CASE WHEN status = 'cancelling' THEN NULL ELSE ? END,
                results = ?, error = CASE WHEN status = 'cancelling' THEN NULL ELSE ? END,
                updated_at = unixepoch() WHERE id = ? AND status IN ('running', 'cancelling')",
        )
        .bind(status)
        .bind(next)
        .bind(serde_json::to_string(results)?)
        .bind(error)
        .bind(id)
        .execute(&self.0)
        .await?;
        Ok(())
    }

    pub async fn wait_for_cancel(&self, id: &str) -> Result<()> {
        loop {
            let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = ?")
                .bind(id)
                .fetch_one(&self.0)
                .await?;
            if status == "cancelling" {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slots_are_unique_and_an_in_flight_step_is_never_replayed() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let db = directory.path().join("state.db");
        let store = Store::open(&db, "revision").await?;
        let input = Map::new();
        let id = store
            .insert(
                "workflow",
                "revision",
                "first",
                &input,
                Some(("scheduled", 100)),
            )
            .await?
            .unwrap();
        assert!(
            store
                .insert(
                    "workflow",
                    "revision",
                    "first",
                    &input,
                    Some(("scheduled", 100))
                )
                .await?
                .is_none()
        );
        assert_eq!(store.claim().await?.unwrap().id, id);
        store
            .finish(
                &id,
                "queued",
                Some("second"),
                &Map::from_iter([("first".into(), json!(1))]),
                None,
            )
            .await?;
        store.0.close().await;

        let store = Store::open(&db, "revision").await?;
        let claim = store.claim().await?.unwrap();
        assert_eq!(claim.id, id);
        assert_eq!(claim.next_step, "second");
        store.0.close().await;

        let store = Store::open(&db, "revision").await?;
        assert!(store.claim().await?.is_none());
        let run = store.get(&id).await?;
        assert_eq!(run["status"], "interrupted");
        assert_eq!(run["results"]["first"], 1);
        Ok(())
    }
}
