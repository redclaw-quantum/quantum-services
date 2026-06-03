//! Drift-triggered recalibration monitor.
//!
//! Polls POST /qtwin/compare every `poll_interval` seconds.
//! When `critical_count > critical_threshold`, automatically submits a
//! `chip-to-calibration` pipeline run via POST /pipeline/run (localhost).
//!
//! Replaces the cron-driven recal_trigger.sh bash script with a typed,
//! observable, resumable pipeline submission.

use std::time::Duration;
use anyhow::Result;
use reqwest::Client;
use tracing::{error, info, warn};
use serde_json::Value;

pub struct MonitorConfig {
    /// Quantum-api base URL (e.g. http://localhost:8765)
    pub quantum_api_url: String,
    /// Pipeline API base URL — same host, same port as qorchestrate
    pub pipeline_api_url: String,
    /// Chip ID to monitor (e.g. "lab3-qpu")
    pub chip_id: String,
    /// How often to poll the twin comparison endpoint (seconds)
    pub poll_interval_secs: u64,
    /// Critical deviation threshold — trigger recal if exceeded
    pub critical_threshold: usize,
    /// Brain path for pipeline runs
    pub brain_path: String,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            quantum_api_url: "http://localhost:8765".to_string(),
            pipeline_api_url: "http://localhost:8767".to_string(),
            chip_id: "lab3-qpu".to_string(),
            poll_interval_secs: 600, // 10 minutes
            critical_threshold: 0,
            brain_path: "/nvme/quantum/data/brains/lab3-qpu.brain".to_string(),
        }
    }
}

/// Run the monitor loop indefinitely.  Spawn from a tokio::spawn task.
pub async fn run_monitor(config: MonitorConfig) {
    let client = Client::new();
    let mut interval =
        tokio::time::interval(Duration::from_secs(config.poll_interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    info!(
        chip_id = %config.chip_id,
        poll_interval_secs = config.poll_interval_secs,
        "Monitor daemon started"
    );

    loop {
        interval.tick().await;

        match poll_twin(&client, &config).await {
            Ok(critical_count) => {
                info!(
                    chip_id = %config.chip_id,
                    critical_count,
                    "Twin comparison polled"
                );
                if critical_count > config.critical_threshold {
                    warn!(
                        chip_id = %config.chip_id,
                        critical_count,
                        threshold = config.critical_threshold,
                        "Critical deviations detected — submitting chip-to-calibration pipeline"
                    );
                    match submit_recal_pipeline(&client, &config).await {
                        Ok(pipeline_id) => {
                            info!(pipeline_id = %pipeline_id, "Recalibration pipeline submitted");
                        }
                        Err(e) => {
                            error!("Failed to submit recalibration pipeline: {e}");
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Twin poll failed (will retry next interval): {e}");
            }
        }
    }
}

async fn poll_twin(client: &Client, config: &MonitorConfig) -> Result<usize> {
    let resp = client
        .get(format!("{}/qtwin/{}/compare", config.quantum_api_url, config.chip_id))
        .timeout(Duration::from_secs(30))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Twin compare returned {status}: {body}"));
    }

    let body: Value = resp.json().await?;
    let critical_count = body
        .get("critical_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    Ok(critical_count)
}

async fn submit_recal_pipeline(client: &Client, config: &MonitorConfig) -> Result<String> {
    let resp = client
        .post(format!("{}/pipeline/run", config.pipeline_api_url))
        .json(&serde_json::json!({
            "template": "chip-to-calibration",
            "params": {
                "chip_id": config.chip_id,
            },
            "brain_path": config.brain_path,
        }))
        .timeout(Duration::from_secs(10))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Pipeline submit returned {status}: {body}"));
    }

    let body: Value = resp.json().await?;
    let pipeline_id = body
        .get("pipeline_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(pipeline_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Phase 1: Red tests (define contract before implementation) ──────

    #[test]
    fn test_monitor_config_defaults() {
        let config = MonitorConfig::default();
        assert_eq!(config.poll_interval_secs, 600);
        assert_eq!(config.critical_threshold, 0);
        assert_eq!(config.chip_id, "lab3-qpu");
    }

    #[test]
    fn test_monitor_config_custom() {
        let config = MonitorConfig {
            chip_id: "test-chip".to_string(),
            poll_interval_secs: 60,
            critical_threshold: 2,
            ..MonitorConfig::default()
        };
        assert_eq!(config.chip_id, "test-chip");
        assert_eq!(config.poll_interval_secs, 60);
        assert_eq!(config.critical_threshold, 2);
    }

    #[test]
    fn test_monitor_config_default_urls() {
        let config = MonitorConfig::default();
        assert_eq!(config.quantum_api_url, "http://localhost:8765");
        assert_eq!(config.pipeline_api_url, "http://localhost:8767");
    }

    #[test]
    fn test_monitor_config_default_brain_path() {
        let config = MonitorConfig::default();
        assert!(!config.brain_path.is_empty());
        assert!(
            config.brain_path.ends_with(".brain"),
            "brain_path should end with .brain, got: {}",
            config.brain_path
        );
    }

    #[test]
    fn test_monitor_config_threshold_zero_triggers_on_any_critical() {
        // threshold=0 means: trigger if critical_count > 0
        let config = MonitorConfig {
            critical_threshold: 0,
            ..MonitorConfig::default()
        };
        let critical_count: usize = 1;
        assert!(
            critical_count > config.critical_threshold,
            "count=1 should exceed threshold=0"
        );
    }

    #[test]
    fn test_monitor_config_threshold_two_does_not_trigger_on_one() {
        let config = MonitorConfig {
            critical_threshold: 2,
            ..MonitorConfig::default()
        };
        let critical_count: usize = 1;
        assert!(
            !(critical_count > config.critical_threshold),
            "count=1 should NOT exceed threshold=2"
        );
    }

    #[test]
    fn test_monitor_config_threshold_two_triggers_on_three() {
        let config = MonitorConfig {
            critical_threshold: 2,
            ..MonitorConfig::default()
        };
        let critical_count: usize = 3;
        assert!(
            critical_count > config.critical_threshold,
            "count=3 should exceed threshold=2"
        );
    }

    #[test]
    fn test_poll_interval_minimum_sensible() {
        // Ensure the default polling interval is at least 60 s (safety guard).
        let config = MonitorConfig::default();
        assert!(
            config.poll_interval_secs >= 60,
            "poll_interval_secs should be >= 60 to avoid hammering the API"
        );
    }

    #[test]
    fn test_chip_id_is_not_empty() {
        let config = MonitorConfig::default();
        assert!(!config.chip_id.is_empty(), "chip_id must not be empty");
    }

    #[test]
    fn test_custom_urls_are_stored() {
        let config = MonitorConfig {
            quantum_api_url: "http://custom-api:9000".to_string(),
            pipeline_api_url: "http://custom-orch:9001".to_string(),
            ..MonitorConfig::default()
        };
        assert_eq!(config.quantum_api_url, "http://custom-api:9000");
        assert_eq!(config.pipeline_api_url, "http://custom-orch:9001");
    }
}
