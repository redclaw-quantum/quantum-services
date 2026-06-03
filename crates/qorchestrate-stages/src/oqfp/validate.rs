use async_trait::async_trait;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct OqfpValidateStage;

impl OqfpValidateStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OqfpValidateStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for OqfpValidateStage {
    fn stage_type(&self) -> StageType {
        StageType::OqfpValidate
    }

    fn timeout_secs(&self) -> u64 {
        5
    }

    async fn execute_raw(&self, input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        let spec = input
            .get("oqfp_build_output")
            .and_then(|v| v.get("oqfp_spec"))
            .or_else(|| input.get("oqfp_spec"))
            .ok_or_else(|| {
                StageError::InvalidInput("missing oqfp_spec in input".to_string())
            })?;

        let layers = spec.get("layers").ok_or_else(|| {
            StageError::InvalidInput("oqfp_spec missing 'layers'".to_string())
        })?;

        let required = [
            "device",
            "connectivity",
            "frequency",
            "qec",
            "control",
            "fabrication",
            "performance",
            "application",
        ];

        let mut errors: Vec<String> = Vec::new();
        for layer in &required {
            if layers.get(layer).is_none() {
                errors.push(format!("missing layer: {}", layer));
            }
        }

        if !errors.is_empty() {
            return Err(StageError::InvalidInput(errors.join(", ")));
        }

        Ok(json!({
            "validated": true,
            "validated_spec": spec,
            "errors": []
        }))
    }
}
