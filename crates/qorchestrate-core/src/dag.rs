use std::collections::{HashMap, HashSet, VecDeque};

use crate::errors::PipelineError;
use crate::pipeline::StageSpec;

pub struct DagBuilder;

impl DagBuilder {
    /// Return stages grouped into batches that can run concurrently.
    ///
    /// Uses Kahn's algorithm:
    /// 1. Compute in-degree for every node from `depends_on` declarations.
    /// 2. Seed a queue with all zero-in-degree nodes.
    /// 3. Drain the queue into a batch; decrement successors' in-degree; enqueue
    ///    newly-zero-in-degree successors for the next batch.
    /// 4. If the total number of processed nodes < number of stages, a cycle exists.
    pub fn topological_batches(
        stages: &[StageSpec],
    ) -> Result<Vec<Vec<String>>, PipelineError> {
        // Map stage_id → all stages that depend on it (successors).
        let mut successors: HashMap<&str, Vec<&str>> = HashMap::new();
        // In-degree: how many unresolved deps each stage has.
        let mut in_degree: HashMap<&str, usize> = HashMap::new();

        for stage in stages {
            in_degree.entry(stage.id.as_str()).or_insert(0);
            for dep in &stage.depends_on {
                successors
                    .entry(dep.as_str())
                    .or_default()
                    .push(stage.id.as_str());
                *in_degree.entry(stage.id.as_str()).or_insert(0) += 1;
            }
        }

        // Seed: all stages with in-degree 0.
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter_map(|(&id, &deg)| if deg == 0 { Some(id) } else { None })
            .collect();

        // Sort for deterministic output in tests.
        let mut seed: Vec<&str> = queue.drain(..).collect();
        seed.sort_unstable();
        queue.extend(seed);

        let mut batches: Vec<Vec<String>> = Vec::new();
        let mut processed = 0usize;

        while !queue.is_empty() {
            // Drain the entire current queue into one batch.
            let batch_size = queue.len();
            let mut batch: Vec<String> = Vec::with_capacity(batch_size);
            let mut next_queue: Vec<&str> = Vec::new();

            for _ in 0..batch_size {
                let node = queue.pop_front().expect("queue non-empty by loop guard");
                batch.push(node.to_string());
                processed += 1;

                if let Some(succs) = successors.get(node) {
                    for &succ in succs {
                        let deg = in_degree.get_mut(succ).expect("successor must be in map");
                        *deg -= 1;
                        if *deg == 0 {
                            next_queue.push(succ);
                        }
                    }
                }
            }

            // Sort next batch for determinism.
            next_queue.sort_unstable();
            queue.extend(next_queue);
            batches.push(batch);
        }

        if processed < stages.len() {
            // Find the first stage not yet processed — it is part of a cycle.
            let processed_set: HashSet<&str> = batches
                .iter()
                .flat_map(|b| b.iter().map(String::as_str))
                .collect();
            let culprit = stages
                .iter()
                .find(|s| !processed_set.contains(s.id.as_str()))
                .map(|s| s.id.clone())
                .unwrap_or_else(|| "unknown".to_string());

            return Err(PipelineError::CycleDetected { stage: culprit });
        }

        Ok(batches)
    }

    /// Generate a Mermaid LR graph string for the pipeline DAG.
    ///
    /// Start nodes (no deps) are styled blue; end nodes (no dependents) green.
    pub fn to_mermaid(stages: &[StageSpec], pipeline_name: &str) -> String {
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("%% Pipeline: {pipeline_name}"));
        lines.push("graph LR".to_string());

        // Collect all stage IDs that appear as a dependency target (i.e. have dependents).
        let has_dependents: HashSet<&str> = stages
            .iter()
            .flat_map(|s| s.depends_on.iter().map(String::as_str))
            .collect();

        // Emit edges.
        let mut has_any_edge = false;
        for stage in stages {
            for dep in &stage.depends_on {
                lines.push(format!("    {} --> {}", dep, stage.id));
                has_any_edge = true;
            }
        }

        // Isolated stages (no edges) need an explicit node declaration.
        if !has_any_edge {
            for stage in stages {
                lines.push(format!("    {}", stage.id));
            }
        }

        // Style start nodes (zero in-degree) blue.
        for stage in stages {
            if stage.depends_on.is_empty() {
                lines.push(format!(
                    "    style {} fill:#4a9eff,color:#fff",
                    stage.id
                ));
            }
        }

        // Style end nodes (no dependents) green.
        for stage in stages {
            if !has_dependents.contains(stage.id.as_str()) {
                lines.push(format!(
                    "    style {} fill:#22cc55,color:#fff",
                    stage.id
                ));
            }
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::pipeline::StageSpec;
    use crate::stage::StageType;

    fn spec(id: &str, deps: &[&str]) -> StageSpec {
        StageSpec {
            id: id.to_string(),
            stage_type: StageType::Skip,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            timeout_secs: None,
            params: HashMap::new(),
            condition: None,
            fallback: None,
            retry: None,
        }
    }

    #[test]
    fn test_linear_pipeline() {
        let stages = vec![spec("A", &[]), spec("B", &["A"]), spec("C", &["B"])];
        let batches = DagBuilder::topological_batches(&stages).expect("no cycle");
        assert_eq!(batches, vec![vec!["A"], vec!["B"], vec!["C"]]);
    }

    #[test]
    fn test_parallel_stages() {
        // A and B both independent; C depends on both.
        let stages = vec![spec("A", &[]), spec("B", &[]), spec("C", &["A", "B"])];
        let batches = DagBuilder::topological_batches(&stages).expect("no cycle");
        assert_eq!(batches.len(), 2);
        let mut first = batches[0].clone();
        first.sort();
        assert_eq!(first, vec!["A", "B"]);
        assert_eq!(batches[1], vec!["C"]);
    }

    #[test]
    fn test_cycle_detection() {
        // A → B → A
        let stages = vec![spec("A", &["B"]), spec("B", &["A"])];
        let result = DagBuilder::topological_batches(&stages);
        assert!(matches!(result, Err(PipelineError::CycleDetected { .. })));
    }

    #[test]
    fn test_diamond() {
        // A → B, A → C, B → D, C → D
        let stages = vec![
            spec("A", &[]),
            spec("B", &["A"]),
            spec("C", &["A"]),
            spec("D", &["B", "C"]),
        ];
        let batches = DagBuilder::topological_batches(&stages).expect("no cycle");
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0], vec!["A"]);
        let mut second = batches[1].clone();
        second.sort();
        assert_eq!(second, vec!["B", "C"]);
        assert_eq!(batches[2], vec!["D"]);
    }
}
