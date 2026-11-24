use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::decision::Question;

pub const DONE: &str = "$done";
pub const INPUT: &str = "$input";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    #[serde(default)]
    pub scripts: BTreeMap<String, Script>,
    #[serde(default)]
    pub workflows: BTreeMap<String, Workflow>,
    #[serde(default)]
    pub schedules: BTreeMap<String, Schedule>,
    #[serde(skip)]
    pub directory: PathBuf,
    #[serde(skip)]
    pub revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub timeout_secs: u64,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    String,
    Boolean,
    Integer,
    Number,
    StringList,
}

impl InputType {
    pub fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::StringList => "string_list",
        }
    }

    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Boolean => value.is_boolean(),
            Self::Integer => value.is_i64() || value.is_u64(),
            Self::Number => value.is_number(),
            Self::StringList => value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub description: String,
    pub entry: String,
    #[serde(default)]
    pub input: BTreeMap<String, InputType>,
    #[serde(default)]
    pub allow_provider: bool,
    pub steps: BTreeMap<String, Step>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Script {
        script: String,
        next: String,
    },
    Decision {
        source: String,
        question: Question,
        on: BTreeMap<String, String>,
        min_score: Option<f64>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    pub workflow: String,
    pub every_secs: i64,
    #[serde(default)]
    pub input: Map<String, Value>,
}

impl Registry {
    pub fn load(path: &Path, backend: &str) -> Result<Self> {
        let path = path.canonicalize().context("locate workflow config")?;
        let bytes = std::fs::read_to_string(&path).context("read workflow config")?;
        let mut registry: Self = toml::from_str(&bytes).context("parse workflow config")?;
        registry.directory = path
            .parent()
            .context("config directory missing")?
            .to_path_buf();
        let mut hash = Sha256::new();
        hash.update(backend);
        hash.update([0]);
        hash.update(bytes);
        registry.revision = format!("{:x}", hash.finalize());
        registry.validate()?;
        Ok(registry)
    }

    pub fn validate(&self) -> Result<()> {
        for (id, script) in &self.scripts {
            ensure!(crate::valid_id(id), "invalid script id {id}");
            ensure!(
                script.program.is_absolute()
                    || script.program.starts_with("./")
                    || script.program.components().count() > 1,
                "script {id} program must be an absolute or explicit relative path"
            );
            ensure!(
                (1..=600).contains(&script.timeout_secs),
                "script {id} timeout must be 1 to 600 seconds"
            );
        }
        for (id, workflow) in &self.workflows {
            ensure!(crate::valid_id(id), "invalid workflow id {id}");
            workflow
                .validate(&self.scripts)
                .with_context(|| format!("workflow {id}"))?;
        }
        for (id, schedule) in &self.schedules {
            ensure!(crate::valid_id(id), "invalid schedule id {id}");
            ensure!(
                (1..=31_536_000).contains(&schedule.every_secs),
                "schedule {id} interval must be 1 to 31536000 seconds"
            );
            let workflow = self
                .workflows
                .get(&schedule.workflow)
                .context("unknown scheduled workflow")?;
            workflow
                .validate_input(&schedule.input)
                .with_context(|| format!("schedule {id}"))?;
        }
        Ok(())
    }
}

impl Workflow {
    pub fn validate_input(&self, input: &Map<String, Value>) -> Result<()> {
        ensure!(
            input.len() == self.input.len(),
            "input fields do not match the workflow schema"
        );
        for (key, kind) in &self.input {
            ensure!(
                input.get(key).is_some_and(|value| kind.accepts(value)),
                "input {key} must be {}",
                kind.name()
            );
        }
        ensure!(
            serde_json::to_vec(input)?.len() <= 64 * 1024,
            "workflow input exceeds 65536 bytes"
        );
        Ok(())
    }

    fn validate(&self, scripts: &BTreeMap<String, Script>) -> Result<()> {
        ensure!(
            !self.description.trim().is_empty(),
            "description is required"
        );
        ensure!(
            self.steps.contains_key(&self.entry) && self.steps.len() <= 32,
            "invalid entry or over 32 steps"
        );
        ensure!(
            self.input.keys().all(|id| crate::valid_id(id)),
            "invalid input key"
        );
        for (id, step) in &self.steps {
            ensure!(crate::valid_id(id), "invalid step id {id}");
            match step {
                Step::Script { script, .. } => {
                    ensure!(
                        scripts.contains_key(script),
                        "step {id} references an unknown script"
                    );
                }
                Step::Decision {
                    source,
                    question,
                    on,
                    min_score,
                } => {
                    ensure!(self.allow_provider, "step {id} requires allow_provider");
                    question.validate()?;
                    ensure!(
                        *source == INPUT || (source != id && self.steps.contains_key(source)),
                        "step {id} has an invalid evidence source"
                    );
                    let expected = match question {
                        Question::Choice { criteria, .. } => {
                            criteria.keys().map(String::as_str).collect()
                        }
                        Question::Noul { .. } => BTreeSet::from(["no", "yes"]),
                        Question::Score { criteria, .. } => {
                            let max = (criteria.len() - 1) as f64;
                            ensure!(
                                min_score.is_some_and(|n| (0.0..=max).contains(&n)),
                                "step {id} has an invalid min_score"
                            );
                            BTreeSet::from(["at_or_above", "below"])
                        }
                    };
                    ensure!(
                        matches!(question, Question::Score { .. }) || min_score.is_none(),
                        "step {id} only accepts min_score with a score question"
                    );
                    ensure!(
                        on.keys().map(String::as_str).collect::<BTreeSet<_>>() == expected,
                        "step {id} must map every outcome"
                    );
                }
            }
            for target in step.targets() {
                ensure!(
                    target == DONE || self.steps.contains_key(target),
                    "unknown step {target}"
                );
            }
        }

        let mut visited = BTreeSet::new();
        self.visit(&self.entry, &mut BTreeSet::new(), &mut visited)?;
        ensure!(
            visited.len() == self.steps.len(),
            "workflow contains unreachable steps"
        );
        for (id, step) in &self.steps {
            if let Step::Decision { source, .. } = step
                && source != INPUT
            {
                ensure!(
                    !self.path_avoids(&self.entry, id, source, &mut BTreeSet::new()),
                    "evidence source {source} must precede {id} on every path"
                );
            }
        }
        Ok(())
    }

    fn visit<'a>(
        &'a self,
        id: &'a str,
        visiting: &mut BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
    ) -> Result<()> {
        if id == DONE || visited.contains(id) {
            return Ok(());
        }
        ensure!(visiting.insert(id), "workflow cycle at {id}");
        for target in self.steps[id].targets() {
            self.visit(target, visiting, visited)?;
        }
        visiting.remove(id);
        visited.insert(id);
        Ok(())
    }

    fn path_avoids<'a>(
        &'a self,
        id: &'a str,
        target: &str,
        avoid: &str,
        visited: &mut BTreeSet<&'a str>,
    ) -> bool {
        if id == DONE || id == avoid || !visited.insert(id) {
            return false;
        }
        id == target
            || self.steps[id]
                .targets()
                .any(|next| self.path_avoids(next, target, avoid, visited))
    }
}

impl Step {
    fn targets(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Self::Script { next, .. } => Box::new(std::iter::once(next.as_str())),
            Self::Decision { on, .. } => Box::new(on.values().map(String::as_str)),
        }
    }
}
