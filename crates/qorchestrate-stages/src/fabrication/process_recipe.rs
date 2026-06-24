use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Evaluate the Josephson-junction process recipe for the design.
///
/// Resolves a recipe name from an explicit `recipe` param, else from the
/// `foundry` profile's `junction_recipe`, else a sane default. POSTs to the
/// quantum-api `/junction/recipe` endpoint and returns the nominal junction
/// (I_c / E_J / L_J / area), the derived `process_params`, and the recipe — so
/// `oqfp_build` can populate the OQFP device + fabrication layers and
/// `tapeout_package` can record the process in the manifest.
pub struct ProcessRecipeStage {
    client: Client,
}

impl ProcessRecipeStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for ProcessRecipeStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the recipe name: explicit `recipe` param wins, then the `foundry`
/// profile's junction recipe, then the default.
fn resolve_recipe_name(input: &Value) -> String {
    if let Some(r) = input.get("recipe").and_then(|v| v.as_str()) {
        return r.to_string();
    }
    if let Some(f) = input.get("foundry").and_then(|v| v.as_str())
        && let Some(profile) = qservices_common::foundry::profile(f)
    {
        return profile.junction_recipe.to_string();
    }
    "dolan_alox_standard".to_string()
}

#[async_trait]
impl Stage for ProcessRecipeStage {
    fn stage_type(&self) -> StageType {
        StageType::ProcessRecipe
    }

    fn timeout_secs(&self) -> u64 {
        15
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let recipe = resolve_recipe_name(&input);
        let resp = self
            .client
            .post(format!("{}/junction/recipe", ctx.quantum_api_url))
            .json(&json!({ "recipe": recipe }))
            .send()
            .await
            .map_err(|e| StageError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StageError::BackendError(format!("{}: {}", status, body)));
        }

        resp.json::<Value>()
            .await
            .map_err(|e| StageError::ParseError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_recipe_wins() {
        let v = json!({ "recipe": "manhattan_alox_tight", "foundry": "university_snf" });
        assert_eq!(resolve_recipe_name(&v), "manhattan_alox_tight");
    }

    #[test]
    fn foundry_profile_supplies_recipe() {
        let v = json!({ "foundry": "commercial_foundry" });
        assert_eq!(resolve_recipe_name(&v), "manhattan_alox_tight");
    }

    #[test]
    fn defaults_when_unspecified() {
        assert_eq!(resolve_recipe_name(&json!({})), "dolan_alox_standard");
    }
}
