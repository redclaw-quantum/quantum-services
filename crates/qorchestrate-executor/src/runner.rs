use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::time::timeout;

use qorchestrate_core::{
    errors::StageError,
    event::StageEvent,
    pipeline::StageSpec,
    stage::StageContext,
};

use crate::registry::StageRegistry;
use crate::state::{StageRunState, StageRunStatus};

pub struct StageRunner {
    registry: Arc<StageRegistry>,
}

impl StageRunner {
    pub fn new(registry: Arc<StageRegistry>) -> Self {
        Self { registry }
    }

    pub async fn run(
        &self,
        spec: &StageSpec,
        input: Value,
        ctx: &StageContext,
        stage_state: &mut StageRunState,
        condition_passed: bool,
    ) -> Result<Value, StageError> {
        // Condition evaluated false → skip without running.
        if !condition_passed {
            stage_state.status = StageRunStatus::Skipped;
            stage_state.skipped_reason = Some("condition evaluated to false".to_string());
            let _ = ctx.event_tx.send(StageEvent::skipped(
                ctx.pipeline_run_id,
                ctx.stage_id.clone(),
                "condition_false",
            ));
            return Err(StageError::SkippedByCondition);
        }

        let timeout_secs = spec.timeout_secs.unwrap_or(60);
        let max_attempts = spec.retry.as_ref().map(|r| r.max_attempts).unwrap_or(1);

        stage_state.status = StageRunStatus::Running;
        stage_state.started_at = Some(chrono::Utc::now());

        let _ = ctx
            .event_tx
            .send(StageEvent::started(ctx.pipeline_run_id, ctx.stage_id.clone()));

        let mut last_error = StageError::BackendError("no attempts made".to_string());

        for attempt in 1..=max_attempts {
            stage_state.attempts = attempt;
            if attempt > 1 {
                let backoff = spec.retry.as_ref().map(|r| r.backoff_secs).unwrap_or(5);
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                let _ = ctx.event_tx.send(StageEvent::progress(
                    ctx.pipeline_run_id,
                    ctx.stage_id.clone(),
                    format!("Retrying (attempt {}/{})", attempt, max_attempts),
                ));
            }

            let start = Instant::now();
            let stage = self
                .registry
                .get(&spec.stage_type)
                .ok_or_else(|| {
                    StageError::BackendError(format!(
                        "No stage registered for {:?}",
                        spec.stage_type
                    ))
                })?;

            let result = timeout(
                Duration::from_secs(timeout_secs),
                stage.execute_raw(input.clone(), ctx),
            )
            .await;

            match result {
                Ok(Ok(output)) => {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    stage_state.status = StageRunStatus::Completed;
                    stage_state.completed_at = Some(chrono::Utc::now());
                    stage_state.duration_ms = Some(duration_ms);
                    stage_state.output = Some(output.clone());
                    let _ = ctx.event_tx.send(StageEvent::completed(
                        ctx.pipeline_run_id,
                        ctx.stage_id.clone(),
                        duration_ms,
                        None,
                    ));
                    return Ok(output);
                }
                Ok(Err(e)) => {
                    last_error = e;
                }
                Err(_elapsed) => {
                    last_error = StageError::Timeout;
                }
            }
        }

        // All attempts exhausted — try fallback if configured.
        if let Some(fallback) = &spec.fallback {
            let fallback_type_str = format!("{:?}", fallback.stage_type);
            stage_state.status = StageRunStatus::FallingBack;
            stage_state.used_fallback = true;
            let _ = ctx.event_tx.send(StageEvent::falling_back(
                ctx.pipeline_run_id,
                ctx.stage_id.clone(),
                &fallback_type_str,
            ));

            let fallback_timeout = fallback.timeout_secs.unwrap_or(timeout_secs * 2);
            let fallback_stage = self
                .registry
                .get(&fallback.stage_type)
                .ok_or_else(|| {
                    StageError::BackendError(format!(
                        "No stage registered for fallback {:?}",
                        fallback.stage_type
                    ))
                })?;

            let start = Instant::now();
            let mut fallback_input = input.clone();
            if let (Value::Object(m), Value::Object(ref extra)) = (
                &mut fallback_input,
                serde_json::to_value(&fallback.params).unwrap_or_default(),
            ) {
                m.extend(extra.clone());
            }

            let result = timeout(
                Duration::from_secs(fallback_timeout),
                fallback_stage.execute_raw(fallback_input, ctx),
            )
            .await;

            match result {
                Ok(Ok(output)) => {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    stage_state.status = StageRunStatus::Completed;
                    stage_state.completed_at = Some(chrono::Utc::now());
                    stage_state.duration_ms = Some(duration_ms);
                    stage_state.output = Some(output.clone());
                    let _ = ctx.event_tx.send(StageEvent::completed(
                        ctx.pipeline_run_id,
                        ctx.stage_id.clone(),
                        duration_ms,
                        None,
                    ));
                    return Ok(output);
                }
                Ok(Err(e)) => {
                    last_error = e;
                }
                Err(_elapsed) => {
                    last_error = StageError::Timeout;
                }
            }
        }

        // Complete failure.
        let err_str = last_error.to_string();
        stage_state.status = StageRunStatus::Failed;
        stage_state.error = Some(err_str.clone());
        stage_state.completed_at = Some(chrono::Utc::now());
        let _ = ctx.event_tx.send(StageEvent::failed(
            ctx.pipeline_run_id,
            ctx.stage_id.clone(),
            &err_str,
        ));
        Err(last_error)
    }
}

