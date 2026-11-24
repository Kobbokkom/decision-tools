mod store;

use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};

use crate::features::{
    decision::{DecideRequest, Decision, Question},
    registry::{DONE, INPUT, Registry, Step},
    script,
};
use store::{Claim, Store};

#[derive(Clone)]
pub struct Engine {
    registry: Arc<Registry>,
    store: Store,
    decision: Decision,
}

impl Engine {
    pub async fn open(registry: Registry, db: &Path, decision: Decision) -> Result<Self> {
        let store = Store::open(db, &registry.revision).await?;
        Ok(Self {
            registry: Arc::new(registry),
            store,
            decision,
        })
    }

    pub async fn ready(&self) -> Result<()> {
        self.store.ready().await
    }

    pub async fn list(&self) -> Result<Value> {
        let workflows: Vec<_> = self
            .registry
            .workflows
            .iter()
            .map(|(name, workflow)| {
                let input: BTreeMap<_, _> = workflow
                    .input
                    .iter()
                    .map(|(key, kind)| (key, kind.name()))
                    .collect();
                json!({"name":name, "description":workflow.description, "input":input})
            })
            .collect();
        let mut schedules = Vec::new();
        for (name, schedule) in &self.registry.schedules {
            let latest_run_id = self.store.latest_scheduled(name).await?;
            schedules.push(json!({
                "name":name, "workflow":schedule.workflow,
                "every_secs":schedule.every_secs, "latest_run_id":latest_run_id
            }));
        }
        let backend = if matches!(self.decision, Decision::Demo) {
            "demo"
        } else {
            "openrouter"
        };
        Ok(json!({"backend":backend, "workflows":workflows, "schedules":schedules}))
    }

    pub async fn decide(&self, request: DecideRequest) -> Result<Value> {
        self.decision.decide(request).await
    }

    pub async fn start(&self, workflow: &str, input: Map<String, Value>) -> Result<Value> {
        let definition = self
            .registry
            .workflows
            .get(workflow)
            .context("unknown workflow")?;
        definition.validate_input(&input)?;
        let id = self
            .store
            .insert(
                workflow,
                &self.registry.revision,
                &definition.entry,
                &input,
                None,
            )
            .await?
            .context("workflow queue is full")?;
        Ok(json!({"id":id, "status":"queued"}))
    }

    pub async fn get(&self, id: &str) -> Result<Value> {
        ensure!(uuid::Uuid::parse_str(id).is_ok(), "invalid run id");
        self.store.get(id).await
    }

    pub async fn cancel(&self, id: &str) -> Result<Value> {
        ensure!(uuid::Uuid::parse_str(id).is_ok(), "invalid run id");
        self.store.cancel(id).await?;
        self.store.get(id).await
    }

    pub async fn work(&self) -> Result<()> {
        loop {
            match self.store.claim().await? {
                Some(claim) => self.execute(claim).await?,
                None => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
    }

    pub async fn schedule(&self) -> Result<()> {
        let now = unix_time()?;
        let mut last = BTreeMap::new();
        for (id, schedule) in &self.registry.schedules {
            last.insert(id, now - now % schedule.every_secs);
        }
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let now = unix_time()?;
            for (id, schedule) in &self.registry.schedules {
                let slot = now - now % schedule.every_secs;
                let last_slot = last.get_mut(id).context("schedule missing")?;
                if slot <= *last_slot {
                    continue;
                }
                *last_slot = slot;
                let workflow = &self.registry.workflows[&schedule.workflow];
                self.store
                    .insert(
                        &schedule.workflow,
                        &self.registry.revision,
                        &workflow.entry,
                        &schedule.input,
                        Some((id, slot)),
                    )
                    .await?;
            }
        }
    }

    async fn execute(&self, claim: Claim) -> Result<()> {
        let definition = &self.registry.workflows[&claim.workflow];
        let mut results: Map<String, Value> = serde_json::from_str(&claim.results)?;
        let input: Value = serde_json::from_str(&claim.input)?;
        let step = &definition.steps[&claim.next_step];
        let result = tokio::select! {
            biased;
            cancellation = self.store.wait_for_cancel(&claim.id) => {
                cancellation?;
                return self.store.finish(&claim.id, "cancelled", None, &results, None).await;
            }
            result = self.step(step, definition.allow_provider, &input, &results) => result,
        };
        match result {
            Ok((value, next)) => {
                results.insert(claim.next_step.clone(), value);
                let status = if next == DONE { "succeeded" } else { "queued" };
                let next = (next != DONE).then_some(next);
                self.store
                    .finish(&claim.id, status, next, &results, None)
                    .await
            }
            Err(error) => {
                let message = format!("step {}: {error:#}", claim.next_step);
                self.store
                    .finish(&claim.id, "failed", None, &results, Some(&message))
                    .await
            }
        }
    }

    async fn step<'a>(
        &self,
        step: &'a Step,
        allow_provider: bool,
        input: &Value,
        results: &Map<String, Value>,
    ) -> Result<(Value, &'a str)> {
        match step {
            Step::Script { script: name, next } => {
                let payload = json!({"input":input, "results":results});
                let output = script::run(
                    &self.registry.scripts[name],
                    &self.registry.directory,
                    &payload,
                )
                .await?;
                Ok((output, next))
            }
            Step::Decision {
                source,
                question,
                on,
                min_score,
            } => {
                let evidence = if source == INPUT {
                    input
                } else {
                    results.get(source).context("missing step evidence")?
                };
                let answer = self
                    .decide(DecideRequest {
                        evidence: evidence.clone(),
                        questions: BTreeMap::from([("route".to_owned(), question.clone())]),
                        allow_provider,
                    })
                    .await?;
                let route = &answer["answers"]["route"];
                let outcome = match question {
                    Question::Choice { .. } => {
                        route["choice"].as_str().context("missing choice")?
                    }
                    Question::Noul { .. } => {
                        if route["noul"].as_f64().context("missing probability")? >= 0.5 {
                            "yes"
                        } else {
                            "no"
                        }
                    }
                    Question::Score { .. } => {
                        let threshold = min_score.context("missing score threshold")?;
                        if route["score"].as_f64().context("missing score")? >= threshold {
                            "at_or_above"
                        } else {
                            "below"
                        }
                    }
                };
                let next = on
                    .get(outcome)
                    .context("decision outcome is unconfigured")?;
                Ok((answer, next))
            }
        }
    }
}

fn unix_time() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .try_into()?)
}
