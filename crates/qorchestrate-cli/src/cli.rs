use anyhow::Context;
use std::path::PathBuf;

use qorchestrate_core::{dag::DagBuilder, pipeline::PipelineDef};
use qorchestrate_executor::{PipelineRunState, StageRunStatus};

use crate::server::AppState;

pub async fn run_pipeline(
    state: AppState,
    template: String,
    params: Vec<String>,
    follow: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    let toml_path = state.templates_dir.join(format!("{template}.toml"));
    let toml_str = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("Template '{template}' not found at {}", toml_path.display()))?;
    let def = PipelineDef::from_toml(&toml_str)?;

    let params_map = parse_key_value_params(&params)?;
    let params_val = serde_json::to_value(params_map)?;

    if dry_run {
        def.validate()
            .map_err(|e| anyhow::anyhow!("{}", e.join("; ")))?;
        let batches = DagBuilder::topological_batches(&def.stages)?;
        println!("DRY RUN: pipeline '{}' is valid", def.meta.name);
        println!("  stages: {}", def.stages.len());
        println!("  batches: {}", batches.len());
        return Ok(());
    }

    if follow {
        eprintln!("Running pipeline '{}'...", def.meta.name);
        let run_state = state
            .executor
            .run_pipeline(&def, params_val, &state.brain_path)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        print_run_summary(&run_state);
    } else {
        eprintln!(
            "Running pipeline '{}'... (use --follow to stream events)",
            def.meta.name
        );
        let run_state = state
            .executor
            .run_pipeline(&def, params_val, &state.brain_path)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("{}", run_state.id);
        eprintln!("Status: {:?}", run_state.status);
    }
    Ok(())
}

pub async fn resume_pipeline(state: AppState, run_id: uuid::Uuid) -> anyhow::Result<()> {
    let saved = state.checkpoint.load(run_id)?;
    let toml_path = state
        .templates_dir
        .join(format!("{}.toml", saved.template));
    let toml_str = std::fs::read_to_string(&toml_path)?;
    let def = PipelineDef::from_toml(&toml_str)?;
    let run_state = state
        .executor
        .resume_pipeline(run_id, &def)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    print_run_summary(&run_state);
    Ok(())
}

pub async fn get_status(state: AppState, run_id: uuid::Uuid) -> anyhow::Result<()> {
    let run_state = state.checkpoint.load(run_id)?;
    println!("{}", serde_json::to_string_pretty(&run_state)?);
    Ok(())
}

pub async fn get_result(state: AppState, run_id: uuid::Uuid) -> anyhow::Result<()> {
    let run_state = state.checkpoint.load(run_id)?;
    match run_state.output {
        Some(output) => println!("{}", serde_json::to_string_pretty(&output)?),
        None => eprintln!("No output yet (status: {:?})", run_state.status),
    }
    Ok(())
}

pub async fn validate(
    state: AppState,
    template: Option<String>,
    file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let toml_str = match (template, file) {
        (Some(t), _) => {
            let path = state.templates_dir.join(format!("{t}.toml"));
            std::fs::read_to_string(&path)?
        }
        (_, Some(f)) => std::fs::read_to_string(&f)?,
        _ => return Err(anyhow::anyhow!("Must provide --template or --file")),
    };
    let def = PipelineDef::from_toml(&toml_str)?;
    match def.validate() {
        Ok(()) => println!("OK — '{}' ({} stages) is valid", def.meta.name, def.stages.len()),
        Err(errs) => {
            eprintln!("INVALID — {} error(s):", errs.len());
            for e in &errs {
                eprintln!("  - {e}");
            }
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn print_dag(state: AppState, template: String) -> anyhow::Result<()> {
    let toml_path = state.templates_dir.join(format!("{template}.toml"));
    let toml_str = std::fs::read_to_string(&toml_path)?;
    let def = PipelineDef::from_toml(&toml_str)?;
    let mermaid = DagBuilder::to_mermaid(&def.stages, &def.meta.name);
    println!("{mermaid}");
    Ok(())
}

pub async fn list_templates(state: AppState) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(&state.templates_dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml")
            && let Ok(toml_str) = std::fs::read_to_string(&path)
            && let Ok(def) = PipelineDef::from_toml(&toml_str) {
            println!(
                "{:<30} v{}  {} stages  — {}",
                def.meta.name,
                def.meta.version,
                def.stages.len(),
                def.meta.description
            );
        }
    }
    Ok(())
}

fn parse_key_value_params(
    params: &[String],
) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    let mut map = serde_json::Map::new();
    for p in params {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("Param '{}' must be key=value format", p))?;
        let val = if v.starts_with('{') || v.starts_with('[') {
            // Looks like JSON object/array — try to parse. Fall back to a
            // plain string if parsing fails (so a literal {abc} doesn't
            // error out).
            serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string()))
        } else if let Ok(n) = v.parse::<f64>() {
            serde_json::Value::Number(
                serde_json::Number::from_f64(n)
                    .ok_or_else(|| anyhow::anyhow!("Non-finite float in param '{}'", p))?,
            )
        } else if v == "true" {
            serde_json::Value::Bool(true)
        } else if v == "false" {
            serde_json::Value::Bool(false)
        } else {
            serde_json::Value::String(v.to_string())
        };
        map.insert(k.to_string(), val);
    }
    Ok(map)
}

pub(crate) fn print_run_summary(state: &PipelineRunState) {
    eprintln!(
        "Pipeline '{}' — {:?}  ({})",
        state.template, state.status, state.id
    );
    eprintln!("Elapsed: {}s", state.elapsed_secs());
    for (id, s) in &state.stages {
        let dur = s
            .duration_ms
            .map(|d| format!("{d}ms"))
            .unwrap_or_else(|| "—".to_string());
        let fallback = if s.used_fallback { " [fallback]" } else { "" };
        eprintln!("  {id:<30} {:?}  {dur}{fallback}", s.status);
    }
    if let Some(err) = state.stages.values().filter_map(|s| s.error.as_ref()).next() {
        eprintln!("First error: {err}");
    }
}

#[allow(dead_code)]
fn stage_run_status_is_terminal(status: &StageRunStatus) -> bool {
    matches!(
        status,
        StageRunStatus::Completed | StageRunStatus::Failed | StageRunStatus::Skipped
    )
}
