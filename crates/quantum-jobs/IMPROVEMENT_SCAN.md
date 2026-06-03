# Rust Improvement Scan: quantum-jobs
**Date:** 2026-04-26 UTC
**Rust:** 1.95 stable -- Edition 2024 capable

## Changes Made
- Cargo.toml: edition 2021 -> 2024 (Rust 1.85+ required; we have 1.95)
- src/main.rs:306: '--json'.to_string() -> .to_owned() (string literal idiom)
- src/main.rs:357: '--in'.to_string() -> .to_owned() (string literal idiom)
- src/main.rs:358: .path().to_str().unwrap() -> .expect("temp file path is valid UTF-8") for clarity
- src/main.rs:361-372: Collapsed nested if-let into if-let chain (edition-2024 collapsible_if lint)

## Security Notes
cargo-audit not installed; no unsafe blocks in production code.

## Files Over Limit
All files within 1300-line limit:
- src/main.rs: 564 lines

## Remaining Opportunities
- Line 110: reqwest::Client::builder().build().unwrap() in AppState::new() -- safe in practice
  (reqwest Client::build() only fails with invalid TLS config, not applicable here)
- Lines 70-71: Uuid::new_v4().to_string() and tool.to_string() -- these require String not &str, correct
- Clone count of 14 -- all are on non-Copy types; appropriate
