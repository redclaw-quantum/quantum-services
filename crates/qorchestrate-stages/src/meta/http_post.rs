use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Generic POST against `ctx.quantum_api_url + input["path"]`.
///
/// Body is the input map with the `path` field stripped out (and `template`
/// stripped too, since it's a control field added by `pipeline_call`).
/// Returns the JSON response verbatim.
///
/// Lets templates use any quantum-api endpoint as a stage without having
/// to add a hand-written stage type per endpoint. Trade-off: the template
/// owner has to know the request shape, but there's no compile-time check
/// — typos go to the endpoint as 4xx errors, which surface as
/// `StageError::BackendError`.
pub struct HttpPostStage {
    client: Client,
}

impl HttpPostStage {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }
}

impl Default for HttpPostStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for HttpPostStage {
    fn stage_type(&self) -> StageType {
        StageType::HttpPost
    }

    fn timeout_secs(&self) -> u64 {
        60
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                StageError::InvalidInput("http_post requires 'path' param".into())
            })?
            .to_string();

        // Optional `remap`: rename source keys to target keys in the body
        // before POSTing. Lets a template wire an upstream stage's output
        // (which the executor exposes as `<dep_id>_output`) into the
        // request field the endpoint actually expects. Example:
        //
        //   [stage.params]
        //   path = "/qspin/fidelity"
        //   remap = { design_output = "array" }
        //
        // Source keys that don't exist in the input are silently ignored.
        let remap: Option<Vec<(String, String)>> = input
            .get("remap")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            });

        // Strip control fields from the body. `path` and `remap` are ours;
        // `template` shows up when the parent is a `pipeline_call`.
        let body = if let Value::Object(mut map) = input.clone() {
            map.remove("path");
            map.remove("template");
            map.remove("remap");
            if let Some(pairs) = &remap {
                for (src, dst) in pairs {
                    if let Some(val) = map.remove(src) {
                        map.insert(dst.clone(), val);
                    }
                }
            }
            Value::Object(map)
        } else {
            input
        };

        let resp = self
            .client
            .post(format!("{}{}", ctx.quantum_api_url, path))
            .json(&body)
            .send()
            .await
            .map_err(|e| StageError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(StageError::BackendError(format!(
                "POST {path}: {status}: {body_text}"
            )));
        }

        resp.json::<Value>()
            .await
            .map_err(|e| StageError::ParseError(e.to_string()))
    }
}
