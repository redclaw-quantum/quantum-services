use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A condition expression that can be a single condition or a logical combinator.
///
/// `#[serde(untagged)]` means JSON/TOML is matched by structure:
/// - `{ field, op, value }` → `Single`
/// - `{ all: [...] }`       → `All`
/// - `{ any: [...] }`       → `Any`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConditionExpr {
    All { all: Vec<Condition> },
    Any { any: Vec<Condition> },
    Single(Condition),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// Dot-path: first segment is stage_id, second is "output" (reserved/ignored),
    /// remaining segments are the JSON path within that stage's output value.
    /// Example: "stage1.output.fidelity" → look up stage_outputs["stage1"]["fidelity"]
    pub field: String,
    pub op: ConditionOp,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    Gt,
    Lt,
    Gte,
    Lte,
    Eq,
    Neq,
    Contains,
    Exists,
}

impl ConditionExpr {
    /// Evaluate this expression against the accumulated stage outputs.
    /// Returns `true` when the downstream stage should run.
    pub fn evaluate(&self, stage_outputs: &HashMap<String, Value>) -> bool {
        match self {
            ConditionExpr::Single(c) => c.evaluate(stage_outputs),
            ConditionExpr::All { all } => all.iter().all(|c| c.evaluate(stage_outputs)),
            ConditionExpr::Any { any } => any.iter().any(|c| c.evaluate(stage_outputs)),
        }
    }
}

impl Condition {
    /// Evaluate a single condition.
    pub fn evaluate(&self, stage_outputs: &HashMap<String, Value>) -> bool {
        let resolved = resolve_field(&self.field, stage_outputs);

        match self.op {
            ConditionOp::Exists => resolved.is_some(),
            ConditionOp::Eq => resolved.map(|v| values_eq(v, &self.value)).unwrap_or(false),
            ConditionOp::Neq => resolved.map(|v| !values_eq(v, &self.value)).unwrap_or(false),
            ConditionOp::Gt => compare_numeric(resolved, &self.value, |a, b| a > b),
            ConditionOp::Lt => compare_numeric(resolved, &self.value, |a, b| a < b),
            ConditionOp::Gte => compare_numeric(resolved, &self.value, |a, b| a >= b),
            ConditionOp::Lte => compare_numeric(resolved, &self.value, |a, b| a <= b),
            ConditionOp::Contains => {
                let Some(target) = resolved else { return false };
                match (target, &self.value) {
                    (Value::String(haystack), Value::String(needle)) => {
                        haystack.contains(needle.as_str())
                    }
                    (Value::Array(arr), needle) => arr.contains(needle),
                    _ => false,
                }
            }
        }
    }
}

/// Parse the field path and look up the value in stage_outputs.
///
/// Format: `"<stage_id>.output.<key1>.<key2>..."` — the literal "output" segment
/// is treated as the separator and skipped; everything after it is traversed as a
/// JSON pointer path within the stage's output object.
///
/// If "output" is absent (fewer than 3 segments), we treat all segments after the
/// stage_id as the JSON path directly.
fn resolve_field<'a>(
    field: &str,
    stage_outputs: &'a HashMap<String, Value>,
) -> Option<&'a Value> {
    let parts: Vec<&str> = field.splitn(3, '.').collect();
    if parts.is_empty() {
        return None;
    }

    let stage_id = parts[0];
    let root = stage_outputs.get(stage_id)?;

    // Build the remaining path after skipping the optional "output" literal.
    let path_str: &str = match parts.as_slice() {
        [_] => return Some(root),
        [_, second] => second,
        [_, second, rest] => {
            if *second == "output" {
                rest
            } else {
                // No "output" token — treat second+rest as raw path.
                // Reconstruct manually without allocating unnecessarily.
                // We must return a borrow, so we traverse iteratively below.
                let full_path = field.split_once('.').map(|x| x.1).unwrap_or("");
                return traverse_json(root, full_path);
            }
        }
        _ => return None,
    };

    traverse_json(root, path_str)
}

/// Traverse a `Value` using a dot-separated key path.
fn traverse_json<'a>(mut current: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(current);
    }
    for key in path.split('.') {
        current = current.get(key)?;
    }
    Some(current)
}

/// Compare two `Value`s for equality, supporting bool/string/number heterogeneity.
fn values_eq(a: &Value, b: &Value) -> bool {
    // Exact structural match first.
    if a == b {
        return true;
    }
    // Bool vs bool (already covered by ==, but be explicit).
    if let (Value::Bool(av), Value::Bool(bv)) = (a, b) {
        return av == bv;
    }
    // Numeric comparison via f64.
    if let (Some(af), Some(bf)) = (as_f64(a), as_f64(b)) {
        return (af - bf).abs() < f64::EPSILON;
    }
    false
}

fn compare_numeric(
    resolved: Option<&Value>,
    rhs: &Value,
    cmp: impl Fn(f64, f64) -> bool,
) -> bool {
    let Some(lhs) = resolved else { return false };
    match (as_f64(lhs), as_f64(rhs)) {
        (Some(a), Some(b)) => cmp(a, b),
        _ => false,
    }
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_outputs(stage_id: &str, data: Value) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert(stage_id.to_string(), data);
        m
    }

    #[test]
    fn test_gt_condition() {
        let outputs = make_outputs("stage1", json!({ "fidelity": 0.998 }));
        let cond = Condition {
            field: "stage1.output.fidelity".to_string(),
            op: ConditionOp::Gt,
            value: json!(0.99),
        };
        assert!(cond.evaluate(&outputs));
    }

    #[test]
    fn test_lt_condition() {
        let outputs = make_outputs("stage1", json!({ "max_zz_khz": 47.0 }));
        let cond = Condition {
            field: "stage1.output.max_zz_khz".to_string(),
            op: ConditionOp::Lt,
            value: json!(50.0),
        };
        assert!(cond.evaluate(&outputs));
    }

    #[test]
    fn test_eq_condition_true() {
        let outputs = make_outputs("stage1", json!({ "collision_free": true }));
        let cond = Condition {
            field: "stage1.output.collision_free".to_string(),
            op: ConditionOp::Eq,
            value: json!(true),
        };
        assert!(cond.evaluate(&outputs));
    }

    #[test]
    fn test_eq_condition_false() {
        let outputs = make_outputs("stage1", json!({ "collision_free": false }));
        let cond = Condition {
            field: "stage1.output.collision_free".to_string(),
            op: ConditionOp::Eq,
            value: json!(true),
        };
        assert!(!cond.evaluate(&outputs));
    }

    #[test]
    fn test_compound_all_passes() {
        let outputs = make_outputs(
            "stage1",
            json!({ "fidelity": 0.998, "max_zz_khz": 47.0 }),
        );
        let expr = ConditionExpr::All {
            all: vec![
                Condition {
                    field: "stage1.output.fidelity".to_string(),
                    op: ConditionOp::Gt,
                    value: json!(0.99),
                },
                Condition {
                    field: "stage1.output.max_zz_khz".to_string(),
                    op: ConditionOp::Lt,
                    value: json!(50.0),
                },
            ],
        };
        assert!(expr.evaluate(&outputs));
    }

    #[test]
    fn test_compound_all_fails_when_one_fails() {
        let outputs = make_outputs(
            "stage1",
            json!({ "fidelity": 0.95, "max_zz_khz": 47.0 }),
        );
        let expr = ConditionExpr::All {
            all: vec![
                Condition {
                    field: "stage1.output.fidelity".to_string(),
                    op: ConditionOp::Gt,
                    value: json!(0.99),
                },
                Condition {
                    field: "stage1.output.max_zz_khz".to_string(),
                    op: ConditionOp::Lt,
                    value: json!(50.0),
                },
            ],
        };
        assert!(!expr.evaluate(&outputs));
    }

    #[test]
    fn test_field_not_found_returns_false() {
        let outputs = make_outputs("stage1", json!({ "fidelity": 0.998 }));
        let cond = Condition {
            field: "stage1.output.nonexistent_field".to_string(),
            op: ConditionOp::Gt,
            value: json!(0.5),
        };
        // Missing field must not panic — it returns false.
        assert!(!cond.evaluate(&outputs));
    }

    #[test]
    fn test_field_stage_not_found_returns_false() {
        let outputs: HashMap<String, Value> = HashMap::new();
        let cond = Condition {
            field: "missing_stage.output.fidelity".to_string(),
            op: ConditionOp::Exists,
            value: json!(null),
        };
        assert!(!cond.evaluate(&outputs));
    }

    #[test]
    fn test_exists_op() {
        let outputs = make_outputs("stage1", json!({ "fidelity": 0.998 }));
        let present = Condition {
            field: "stage1.output.fidelity".to_string(),
            op: ConditionOp::Exists,
            value: json!(null),
        };
        let absent = Condition {
            field: "stage1.output.missing".to_string(),
            op: ConditionOp::Exists,
            value: json!(null),
        };
        assert!(present.evaluate(&outputs));
        assert!(!absent.evaluate(&outputs));
    }

    #[test]
    fn test_contains_string() {
        let outputs = make_outputs("stage1", json!({ "label": "transmon_cross_v2" }));
        let cond = Condition {
            field: "stage1.output.label".to_string(),
            op: ConditionOp::Contains,
            value: json!("transmon"),
        };
        assert!(cond.evaluate(&outputs));
    }

    #[test]
    fn test_contains_array() {
        let outputs = make_outputs("stage1", json!({ "codes": ["surface", "bacon_shor"] }));
        let cond = Condition {
            field: "stage1.output.codes".to_string(),
            op: ConditionOp::Contains,
            value: json!("surface"),
        };
        assert!(cond.evaluate(&outputs));
    }

    #[test]
    fn test_gte_condition() {
        let outputs = make_outputs("stage1", json!({ "fidelity": 0.99 }));
        let at_threshold = Condition {
            field: "stage1.output.fidelity".into(), op: ConditionOp::Gte, value: json!(0.99),
        };
        let above = Condition {
            field: "stage1.output.fidelity".into(), op: ConditionOp::Gte, value: json!(0.98),
        };
        let below = Condition {
            field: "stage1.output.fidelity".into(), op: ConditionOp::Gte, value: json!(0.999),
        };
        assert!(at_threshold.evaluate(&outputs), "0.99 >= 0.99 should be true");
        assert!(above.evaluate(&outputs), "0.99 >= 0.98 should be true");
        assert!(!below.evaluate(&outputs), "0.99 >= 0.999 should be false");
    }

    #[test]
    fn test_lte_condition() {
        let outputs = make_outputs("stage1", json!({ "zz_khz": 50.0 }));
        let at = Condition {
            field: "stage1.output.zz_khz".into(), op: ConditionOp::Lte, value: json!(50.0),
        };
        let above = Condition {
            field: "stage1.output.zz_khz".into(), op: ConditionOp::Lte, value: json!(49.0),
        };
        assert!(at.evaluate(&outputs), "50 <= 50 should be true");
        assert!(!above.evaluate(&outputs), "50 <= 49 should be false");
    }

    #[test]
    fn test_neq_condition() {
        let outputs = make_outputs("stage1", json!({ "status": "completed" }));
        let not_eq = Condition {
            field: "stage1.output.status".into(), op: ConditionOp::Neq, value: json!("failed"),
        };
        let is_eq = Condition {
            field: "stage1.output.status".into(), op: ConditionOp::Neq, value: json!("completed"),
        };
        assert!(not_eq.evaluate(&outputs), "'completed' != 'failed' should be true");
        assert!(!is_eq.evaluate(&outputs), "'completed' != 'completed' should be false");
    }

    #[test]
    fn test_compound_any_passes_when_one_passes() {
        let outputs = make_outputs("stage1", json!({ "fidelity": 0.998, "zz_khz": 55.0 }));
        let expr = ConditionExpr::Any {
            any: vec![
                Condition {
                    field: "stage1.output.fidelity".into(), op: ConditionOp::Gt, value: json!(0.999),
                },
                Condition {
                    field: "stage1.output.zz_khz".into(), op: ConditionOp::Lt, value: json!(60.0),
                },
            ],
        };
        // First fails (0.998 > 0.999 = false), second passes (55 < 60 = true)
        assert!(expr.evaluate(&outputs), "Any should pass when at least one condition passes");
    }

    #[test]
    fn test_compound_any_fails_when_all_fail() {
        let outputs = make_outputs("stage1", json!({ "fidelity": 0.95, "zz_khz": 80.0 }));
        let expr = ConditionExpr::Any {
            any: vec![
                Condition {
                    field: "stage1.output.fidelity".into(), op: ConditionOp::Gt, value: json!(0.99),
                },
                Condition {
                    field: "stage1.output.zz_khz".into(), op: ConditionOp::Lt, value: json!(50.0),
                },
            ],
        };
        assert!(!expr.evaluate(&outputs), "Any should fail when all conditions fail");
    }
}
