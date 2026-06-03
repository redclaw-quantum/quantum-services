use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::condition::ConditionExpr;
use crate::stage::StageType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDef {
    #[serde(rename = "pipeline")]
    pub meta: PipelineMeta,
    #[serde(rename = "stage", default)]
    pub stages: Vec<StageSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_timeout")]
    pub default_timeout_secs: u64,
    #[serde(default)]
    pub output: Option<OutputSpec>,
}

fn default_concurrency() -> usize {
    4
}

fn default_timeout() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageSpec {
    pub id: String,
    #[serde(rename = "type")]
    pub stage_type: StageType,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub params: HashMap<String, Value>,
    pub condition: Option<ConditionExpr>,
    pub fallback: Option<FallbackSpec>,
    pub retry: Option<RetrySpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackSpec {
    #[serde(rename = "type")]
    pub stage_type: StageType,
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub params: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrySpec {
    pub max_attempts: usize,
    #[serde(default = "default_backoff")]
    pub backoff_secs: u64,
}

fn default_backoff() -> u64 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputSpec {
    pub stage: String,
    pub field: String,
    pub artifact_key: String,
}

impl PipelineDef {
    /// Parse a pipeline definition from TOML.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Validate: all `depends_on` references must resolve to a declared stage,
    /// and no stage may have an empty `id`.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors: Vec<String> = Vec::new();
        let ids: std::collections::HashSet<&str> =
            self.stages.iter().map(|s| s.id.as_str()).collect();

        for stage in &self.stages {
            if stage.id.is_empty() {
                errors.push("Stage has empty id".to_string());
            }
            for dep in &stage.depends_on {
                if !ids.contains(dep.as_str()) {
                    errors.push(format!(
                        "Stage '{}' depends_on unknown stage '{}'",
                        stage.id, dep
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_TOML: &str = r#"
[pipeline]
name = "test-pipeline"
version = "0.1.0"
description = "A minimal test pipeline"
max_concurrency = 2
default_timeout_secs = 120

[[stage]]
id = "freq_opt"
type = "freq_optimize"

[[stage]]
id = "xtalk"
type = "xtalk_analyze"
depends_on = ["freq_opt"]
timeout_secs = 60
"#;

    #[test]
    fn test_toml_roundtrip() {
        let def = PipelineDef::from_toml(MINIMAL_TOML).expect("parse failed");
        assert_eq!(def.meta.name, "test-pipeline");
        assert_eq!(def.meta.version, "0.1.0");
        assert_eq!(def.meta.max_concurrency, 2);
        assert_eq!(def.meta.default_timeout_secs, 120);
        assert_eq!(def.stages.len(), 2);

        let freq = &def.stages[0];
        assert_eq!(freq.id, "freq_opt");
        assert_eq!(freq.stage_type, StageType::FreqOptimize);
        assert!(freq.depends_on.is_empty());

        let xtalk = &def.stages[1];
        assert_eq!(xtalk.id, "xtalk");
        assert_eq!(xtalk.stage_type, StageType::XtalkAnalyze);
        assert_eq!(xtalk.depends_on, vec!["freq_opt"]);
        assert_eq!(xtalk.timeout_secs, Some(60));
    }

    #[test]
    fn test_validate_ok() {
        let def = PipelineDef::from_toml(MINIMAL_TOML).expect("parse failed");
        assert!(def.validate().is_ok());
    }

    #[test]
    fn test_validate_missing_dep() {
        const BAD_TOML: &str = r#"
[pipeline]
name = "bad"
version = "0.1.0"

[[stage]]
id = "xtalk"
type = "xtalk_analyze"
depends_on = ["nonexistent"]
"#;
        let def = PipelineDef::from_toml(BAD_TOML).expect("parse failed");
        let errs = def.validate().expect_err("should fail");
        assert!(errs.iter().any(|e| e.contains("nonexistent")));
    }

    #[test]
    fn test_default_concurrency_and_timeout() {
        const BARE_TOML: &str = r#"
[pipeline]
name = "bare"
version = "1.0.0"
"#;
        let def = PipelineDef::from_toml(BARE_TOML).expect("parse failed");
        assert_eq!(def.meta.max_concurrency, 4);
        assert_eq!(def.meta.default_timeout_secs, 300);
        assert!(def.stages.is_empty());
    }

    /// Verify all 5 bundled pipeline templates parse without error.
    #[test]
    fn test_all_templates_parse() {
        let templates_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()   // crates/
            .parent().unwrap()   // qorchestrate/
            .join("templates");

        let templates = [
            "design-to-chip.toml",
            "chip-to-calibration.toml",
            "yield-sweep.toml",
            "full-loop.toml",
            "active-design-loop.toml",
        ];

        for name in &templates {
            let path = templates_dir.join(name);
            let toml_str = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("Failed to read {}: {}", name, e));
            let def = PipelineDef::from_toml(&toml_str)
                .unwrap_or_else(|e| panic!("Failed to parse {}: {}", name, e));
            assert!(!def.meta.name.is_empty(), "{} has empty pipeline name", name);
            // validate stage graph
            if let Err(errs) = def.validate() {
                panic!("{} failed validation: {:?}", name, errs);
            }
        }
    }
}
