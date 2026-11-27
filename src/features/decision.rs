use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const ENDPOINT: &str = "https://openrouter.ai/api/alpha/decisions";
const CHAT_ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
const MAX_RESPONSE: usize = 128 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Question {
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
    },
    Noul {
        instructions: String,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

impl Question {
    pub fn validate(&self) -> Result<()> {
        self.validate_with(16)
    }

    /// Choice and score questions normally offer at most 16 criteria; local tools raise the bound.
    fn validate_with(&self, max_criteria: usize) -> Result<()> {
        let instructions = match self {
            Self::Choice {
                instructions,
                criteria,
            } => {
                ensure!(
                    (2..=max_criteria).contains(&criteria.len())
                        && criteria.iter().all(|(label, text)| !label.trim().is_empty()
                            && label.len() <= 80
                            && valid_text(text)),
                    "choice requires 2 to {max_criteria} named criteria"
                );
                instructions
            }
            Self::Noul { instructions } => instructions,
            Self::Score {
                instructions,
                criteria,
            } => {
                ensure!(
                    (2..=max_criteria).contains(&criteria.len())
                        && criteria.iter().all(|text| valid_text(text)),
                    "score requires 2 to {max_criteria} ordered criteria"
                );
                instructions
            }
        };
        ensure!(valid_text(instructions), "invalid decision instructions");
        Ok(())
    }

    fn validate_answer(&self, answer: &Value) -> Result<()> {
        match self {
            Self::Choice { criteria, .. } => {
                ensure!(
                    answer["type"] == "choice"
                        && answer["choice"]
                            .as_str()
                            .is_some_and(|key| criteria.contains_key(key)),
                    "invalid choice answer"
                );
            }
            Self::Noul { .. } => {
                ensure!(
                    answer["type"] == "noul"
                        && answer["noul"]
                            .as_f64()
                            .is_some_and(|p| (0.0..=1.0).contains(&p)),
                    "invalid yes/no probability"
                );
            }
            Self::Score { criteria, .. } => {
                let max = (criteria.len() - 1) as f64;
                ensure!(
                    answer["type"] == "score"
                        && answer["score"]
                            .as_f64()
                            .is_some_and(|score| (0.0..=max).contains(&score)),
                    "invalid score answer"
                );
            }
        }
        Ok(())
    }

    fn demo_answer(&self) -> Value {
        match self {
            Self::Choice { criteria, .. } => {
                json!({"type":"choice", "choice":criteria.keys().next(), "confidence":1.0})
            }
            Self::Noul { .. } => json!({"type":"noul", "noul":1.0, "confidence":1.0}),
            Self::Score { .. } => json!({"type":"score", "score":0.0, "confidence":1.0}),
        }
    }
}

fn valid_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 4096
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecideRequest {
    /// Relevant text or JSON evidence. Do not include secrets.
    pub evidence: Value,
    pub questions: BTreeMap<String, Question>,
    /// Set only after confirming that OpenRouter may receive the evidence.
    pub allow_provider: bool,
}

#[derive(Clone)]
pub enum Decision {
    Demo,
    OpenRouter {
        client: reqwest::Client,
        model: String,
        key: String,
    },
}

impl Decision {
    pub fn openrouter(model: String, key: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self::OpenRouter { client, model, key })
    }

    pub async fn decide(&self, request: DecideRequest) -> Result<Value> {
        self.decide_at(request, ENDPOINT, 16).await
    }

    /// Same call for local tools whose choices list every element on a page.
    pub async fn decide_wide(&self, request: DecideRequest) -> Result<Value> {
        self.decide_at(request, ENDPOINT, 256).await
    }

    /// The same provider with another model, for tools that need the Jev models.
    pub fn with_model(&self, model: &str) -> Self {
        match self {
            Self::Demo => Self::Demo,
            Self::OpenRouter { client, key, .. } => Self::OpenRouter {
                client: client.clone(),
                model: model.to_owned(),
                key: key.clone(),
            },
        }
    }

    /// One short chat completion that returns a JSON object, for text the decisions API cannot write.
    pub async fn complete_json(&self, model: &str, system: &str, user: &Value) -> Result<Value> {
        let Self::OpenRouter { client, key, .. } = self else {
            bail!("text generation is unavailable in demo mode");
        };
        let response = client
            .post(CHAT_ENDPOINT)
            .bearer_auth(key)
            .json(&json!({
                "model": model, "max_tokens": 1024,
                "response_format": {"type": "json_object"},
                "reasoning": {"enabled": false},
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user.to_string()},
                ],
            }))
            .send()
            .await
            .context("OpenRouter chat request failed")?;
        ensure!(
            response.status().is_success(),
            "OpenRouter chat returned HTTP {}",
            response.status().as_u16()
        );
        let body: Value = response
            .json()
            .await
            .context("invalid OpenRouter chat JSON")?;
        let text = body["choices"][0]["message"]["content"]
            .as_str()
            .context("missing chat content")?;
        serde_json::from_str(text).context("chat content is not JSON")
    }

    async fn decide_at(
        &self,
        request: DecideRequest,
        endpoint: &str,
        max_criteria: usize,
    ) -> Result<Value> {
        ensure!(
            (1..=16).contains(&request.questions.len())
                && request.questions.keys().all(|key| crate::valid_id(key)),
            "provide 1 to 16 named questions"
        );
        for question in request.questions.values() {
            question.validate_with(max_criteria)?;
        }
        // A string state prevents JSON evidence from becoming provider-side image URLs.
        let state = match request.evidence {
            Value::String(text) => text,
            evidence => serde_json::to_string(&evidence)?,
        };
        ensure!(
            !state.trim().is_empty() && state.len() <= 64 * 1024,
            "evidence must contain 1 to 65536 bytes"
        );
        if let Self::Demo = self {
            let answers: BTreeMap<_, _> = request
                .questions
                .iter()
                .map(|(id, question)| (id, question.demo_answer()))
                .collect();
            return Ok(json!({
                "model": "demo", "answers": answers, "usage": {"cost": 0}
            }));
        }
        ensure!(
            request.allow_provider,
            "OpenRouter evidence requires allow_provider"
        );
        let Self::OpenRouter { client, model, key } = self else {
            unreachable!()
        };
        let mut response = client
            .post(endpoint)
            .bearer_auth(key)
            .json(&json!({"model":model, "state":state, "questions":request.questions}))
            .send()
            .await
            .context("OpenRouter request failed")?;
        if !response.status().is_success() {
            bail!("OpenRouter returned HTTP {}", response.status().as_u16());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("OpenRouter response failed")?
        {
            ensure!(
                body.len() + chunk.len() <= MAX_RESPONSE,
                "OpenRouter response exceeds limit"
            );
            body.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&body).context("invalid OpenRouter JSON")?;
        let answers = value["answers"]
            .as_object()
            .context("missing decision answers")?;
        let mut validated = serde_json::Map::new();
        for (id, question) in &request.questions {
            let answer = answers
                .get(id)
                .context("missing requested decision answer")?;
            question.validate_answer(answer)?;
            validated.insert(id.clone(), answer.clone());
        }
        Ok(json!({"model": value["model"], "answers": validated, "usage": value["usage"]}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_provider_answers() {
        let question = Question::Noul {
            instructions: "Healthy?".into(),
        };
        assert!(
            question
                .validate_answer(&json!({"type":"refusal"}))
                .is_err()
        );
        assert!(
            question
                .validate_answer(&json!({"type":"noul","noul":1.2}))
                .is_err()
        );
        assert!(
            question
                .validate_answer(&json!({"type":"noul","noul":0.75}))
                .is_ok()
        );
    }

    #[test]
    fn wide_choices_are_a_local_tool_allowance() {
        let wide = Question::Choice {
            instructions: "Which?".into(),
            criteria: (0..100)
                .map(|i| (i.to_string(), "option".to_owned()))
                .collect(),
        };
        assert!(wide.validate().is_err());
        assert!(wide.validate_with(256).is_ok());
    }

    #[tokio::test]
    async fn unapproved_evidence_is_rejected_before_http() {
        let result = Decision::openrouter("model".into(), "unused".into())
            .unwrap()
            .decide(DecideRequest {
                evidence: json!("private"),
                questions: BTreeMap::from([(
                    "check".into(),
                    Question::Noul {
                        instructions: "Healthy?".into(),
                    },
                )]),
                allow_provider: false,
            })
            .await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "OpenRouter evidence requires allow_provider"
        );
    }
    #[tokio::test]
    async fn openrouter_wire_contract_preserves_answer_and_cost() -> Result<()> {
        use axum::{Json, Router, http::HeaderMap, routing::post};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/decisions", listener.local_addr()?);
        let app = Router::new().route(
            "/decisions",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer test-key");
                assert_eq!(body["model"], "liquid/d1.2-ultra");
                assert_eq!(body["state"], r#"{"ready":true}"#);
                assert_eq!(body["questions"]["healthy"]["type"], "noul");
                Json(json!({
                    "model":"liquid/d1.2-ultra",
                    "answers":{"healthy":{"type":"noul","noul":0.92}},
                    "usage":{"input_tokens":8,"output_tokens":0,"cost":0.000001}
                }))
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let response = Decision::openrouter("liquid/d1.2-ultra".into(), "test-key".into())?
            .decide_at(
                DecideRequest {
                    evidence: json!({"ready":true}),
                    questions: BTreeMap::from([(
                        "healthy".into(),
                        Question::Noul {
                            instructions: "Is the sample healthy?".into(),
                        },
                    )]),
                    allow_provider: true,
                },
                &endpoint,
                16,
            )
            .await;
        server.abort();
        let response = response?;
        assert_eq!(response["answers"]["healthy"]["noul"], 0.92);
        assert_eq!(response["usage"]["cost"], 0.000001);
        Ok(())
    }
}
