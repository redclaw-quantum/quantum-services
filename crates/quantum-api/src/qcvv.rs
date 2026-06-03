//! QCVV — Quantum Characterization, Verification & Validation
//! Extracted from main.rs to keep file within 1300-line limit.

use axum::response::IntoResponse;
use axum::Json;
use serde_json::{Value, json};


// ─────────────────────────────────────────────────────────────────────────────
// QCVV — Quantum Characterization, Verification and Validation (Phase 8B)
// ─────────────────────────────────────────────────────────────────────────────

pub async fn qcvv_health() -> impl IntoResponse {
    Json(json!({"status": "ok", "service": "qcvv"}))
}

/// POST /qcvv/quantum-volume
///
/// Analytically estimates Quantum Volume from device noise parameters.
///
/// QV = 2^n_qv where n_qv is the largest width such that a random n×n
/// square circuit's heavy output probability (HOP) exceeds 2/3. The HOP
/// is modelled as:
///
///   HOP(n) = exp(-n² × ε_gate / 2) × exp(-n × t_gate / T_eff) × (1 - ε_ro)^n
///
/// where T_eff = harmonic mean of T1 and 2·T2.
pub async fn qcvv_quantum_volume(Json(body): Json<Value>) -> impl IntoResponse {
    let n_qubits = body["n_qubits"].as_u64().unwrap_or(10) as usize;
    let t1_us    = body["t1_us"].as_f64().unwrap_or(80.0);
    let t2_us    = body["t2_us"].as_f64().unwrap_or(60.0);
    let gate_error = body["gate_error"].as_f64().unwrap_or(1e-3);
    let readout_error = body["readout_error"].as_f64().unwrap_or(1e-2);
    let gate_time_ns = body["gate_time_ns"].as_f64().unwrap_or(40.0);

    // Effective coherence time (harmonic mean of T1 and 2*T2)
    let t_eff_us = 2.0 * t1_us * t2_us / (t1_us + t2_us);
    let t_eff_ns = t_eff_us * 1_000.0;

    let mut qv_n = 0usize;
    let mut hop_by_width: Vec<serde_json::Value> = Vec::new();

    for n in 1..=n_qubits.min(20) {
        let n_f = n as f64;
        // n layers of n/2 two-qubit gates → total n²/2 2Q gates per circuit
        let n_2q_gates = n_f * n_f / 2.0;
        // Depolarising contribution from 2Q gate errors
        let gate_survival = (-n_2q_gates * gate_error).exp();
        // Decoherence: each gate consumes gate_time_ns of coherence budget
        let total_gate_time_ns = n_2q_gates * gate_time_ns;
        let decoherence_survival = (-(total_gate_time_ns / t_eff_ns)).exp();
        // Readout error on n qubits
        let readout_survival = (1.0 - readout_error).powi(n as i32);
        // Model: HOP ≈ (1 + gate×decoherence×readout) / 2
        // (ideal = 1.0, worst = 0.5 random)
        let ideal_contribution = gate_survival * decoherence_survival * readout_survival;
        let hop = 0.5 + 0.5 * ideal_contribution;

        hop_by_width.push(json!({
            "n": n,
            "hop": (hop * 1000.0).round() / 1000.0,
            "passes": hop > 2.0 / 3.0,
        }));

        if hop > 2.0 / 3.0 {
            qv_n = n;
        }
    }

    let qv = 1u64 << qv_n;
    let bottleneck = if gate_error > gate_time_ns / (t_eff_ns * n_qubits as f64) {
        "gate_error"
    } else {
        "decoherence"
    };

    Json(json!({
        "quantum_volume": qv,
        "log2_qv": qv_n,
        "bottleneck": bottleneck,
        "t_eff_us": (t_eff_us * 100.0).round() / 100.0,
        "hop_by_width": hop_by_width,
        "inputs": {
            "n_qubits": n_qubits,
            "t1_us": t1_us,
            "t2_us": t2_us,
            "gate_error": gate_error,
            "readout_error": readout_error,
            "gate_time_ns": gate_time_ns,
        }
    }))
}

/// POST /qcvv/process-fidelity
///
/// Computes average gate fidelity, process fidelity, and upper-bound diamond
/// distance from device noise parameters.
///
/// Relations used:
///   F_avg = 1 - ε_gate/2 × (1 - e^{-t_gate/T1}) × (1 - e^{-t_gate/2T2})
///   F_p   = (d·F_avg - 1) / (d - 1)   [d = 2 for single-qubit gate]
///   ε_⋄   ≤ d·(1 - F_p)               [diamond distance upper bound]
pub async fn qcvv_process_fidelity(Json(body): Json<Value>) -> impl IntoResponse {
    let gate_error   = body["gate_error"].as_f64().unwrap_or(1e-3);
    let t1_us        = body["t1_us"].as_f64().unwrap_or(80.0);
    let t2_us        = body["t2_us"].as_f64().unwrap_or(60.0);
    let gate_time_ns = body["gate_time_ns"].as_f64().unwrap_or(40.0);
    let n_qubits_gate = body["n_qubits_gate"].as_u64().unwrap_or(2) as u32; // 1 or 2

    let t1_ns = t1_us * 1_000.0;
    let t2_ns = t2_us * 1_000.0;

    // T1 and T2 survival during gate
    let t1_survival = (-gate_time_ns / t1_ns).exp();
    let t2_survival = (-gate_time_ns / (2.0 * t2_ns)).exp();

    // Average gate fidelity incorporating decoherence and gate error
    let decoherence_infidelity = 1.0 - t1_survival * t2_survival;
    let total_infidelity = gate_error + decoherence_infidelity - gate_error * decoherence_infidelity;
    let f_avg = (1.0 - total_infidelity).clamp(0.0, 1.0);

    // Process fidelity (d = 2^n_qubits_gate)
    let d = (1u64 << n_qubits_gate) as f64;
    let f_process = ((d * f_avg) - 1.0) / (d - 1.0);
    let f_process = f_process.clamp(0.0, 1.0);

    // Diamond distance upper bound: ε_⋄ ≤ d * (1 - F_p)
    let diamond_distance_ub = d * (1.0 - f_process);

    // Error probability in depolarising channel approximation
    let p_error = (d * d / (d * d - 1.0)) * (1.0 - f_process);

    Json(json!({
        "average_gate_fidelity":  (f_avg * 1e6).round() / 1e6,
        "process_fidelity":       (f_process * 1e6).round() / 1e6,
        "diamond_distance_upper_bound": (diamond_distance_ub * 1e6).round() / 1e6,
        "depolarising_error_prob": (p_error * 1e6).round() / 1e6,
        "t1_survival":  (t1_survival * 1e6).round() / 1e6,
        "t2_survival":  (t2_survival * 1e6).round() / 1e6,
        "decoherence_infidelity": (decoherence_infidelity * 1e6).round() / 1e6,
        "inputs": {
            "gate_error": gate_error,
            "t1_us": t1_us,
            "t2_us": t2_us,
            "gate_time_ns": gate_time_ns,
            "n_qubits_gate": n_qubits_gate,
        }
    }))
}

/// POST /qcvv/zne
///
/// Zero-Noise Extrapolation: given expectation values measured at noise
/// scale factors λ = [1, 2, 3, …], extrapolate to the zero-noise limit
/// using Richardson extrapolation (linear or polynomial) or exponential fit.
///
/// Input:
///   noise_scales: [1.0, 2.0, 3.0]
///   expectation_values: [0.85, 0.72, 0.61]
///   method: "linear" | "richardson" | "exponential"
pub async fn qcvv_zne(Json(body): Json<Value>) -> impl IntoResponse {
    let scales: Vec<f64> = body["noise_scales"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_else(|| vec![1.0, 2.0, 3.0]);

    let values: Vec<f64> = body["expectation_values"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_default();

    let method = body["method"].as_str().unwrap_or("richardson");

    if scales.len() < 2 || values.len() < 2 || scales.len() != values.len() {
        return Json(json!({
            "error": "noise_scales and expectation_values must have the same length (≥2)"
        }));
    }

    let n = scales.len();
    let zero_noise = match method {
        "linear" | "richardson" if n == 2 => {
            // Richardson: E(0) = (λ2·E1 - λ1·E2) / (λ2 - λ1)
            let (l1, l2) = (scales[0], scales[1]);
            let (e1, e2) = (values[0], values[1]);
            (l2 * e1 - l1 * e2) / (l2 - l1)
        }
        "richardson" => {
            // Generalised Richardson extrapolation via Vandermonde system
            // For n points: fit polynomial and evaluate at λ=0
            // Simplified: use Neville's algorithm
            let mut c = values.clone();
            for order in 1..n {
                for i in 0..(n - order) {
                    let denom = scales[i] - scales[i + order];
                    if denom.abs() < 1e-12 { continue; }
                    c[i] = (scales[i + order] * c[i] - scales[i] * c[i + 1]) / denom;
                }
            }
            c[0]
        }
        "exponential" => {
            // Fit E(λ) = A + B·exp(C·λ); extrapolate to λ=0 → A + B
            // Simplified 2-parameter model: E(λ) = E0 · exp(-α·(λ-1))
            // ln(E(λ)/E(1)) = -α·(λ-1) → linear fit
            let e1 = values[0];
            if e1.abs() < 1e-12 {
                values[0]
            } else {
                let mut sum_x = 0.0f64;
                let mut sum_y = 0.0f64;
                let mut sum_xx = 0.0f64;
                let mut sum_xy = 0.0f64;
                let mut count = 0usize;
                for i in 0..n {
                    let x = scales[i] - scales[0];
                    let ratio = values[i] / e1;
                    if ratio <= 0.0 { continue; }
                    let y = ratio.ln();
                    sum_x += x;
                    sum_y += y;
                    sum_xx += x * x;
                    sum_xy += x * y;
                    count += 1;
                }
                if count < 2 {
                    values[0]
                } else {
                    let n_f = count as f64;
                    let alpha = (n_f * sum_xy - sum_x * sum_y) / (n_f * sum_xx - sum_x * sum_x);
                    // Extrapolate to λ=scales[0]-1 steps before first point → λ=0
                    e1 * (alpha * (1.0 - scales[0])).exp()
                }
            }
        }
        _ => {
            // Default: linear extrapolation using first two points
            let (l1, l2) = (scales[0], scales[1]);
            let (e1, e2) = (values[0], values[1]);
            e1 + (e1 - e2) / (l2 - l1) * (0.0 - l1)
        }
    };

    // Estimate uncertainty from variation in measured values
    let mean_val = values.iter().sum::<f64>() / n as f64;
    let variance = values.iter().map(|v| (v - mean_val).powi(2)).sum::<f64>() / n as f64;
    let std_val = variance.sqrt();
    // Extrapolation amplifies noise; uncertainty ≈ std * |λ_max / λ_min|
    let scale_ratio = scales.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
        / scales.iter().cloned().fold(f64::INFINITY, f64::min);
    let uncertainty = std_val * scale_ratio;

    // Improvement over raw (first-scale) value
    let raw_value = values[0];
    let improvement = if raw_value.abs() > 1e-12 {
        (zero_noise - raw_value) / raw_value.abs()
    } else {
        0.0
    };

    Json(json!({
        "zero_noise_estimate":  (zero_noise * 1e8).round() / 1e8,
        "uncertainty":          (uncertainty * 1e8).round() / 1e8,
        "raw_value":            raw_value,
        "improvement_fraction": (improvement * 1e6).round() / 1e6,
        "method":               method,
        "n_points":             n,
        "noise_scales":         scales,
        "expectation_values":   values,
    }))
}

/// POST /qcvv/clops
///
/// Computes Circuit Layer Operations Per Second (CLOPS) — IBM's quantum
/// throughput metric — and related performance numbers.
///
/// CLOPS = (M × K × D × shots) / execution_time_s
///
/// where:
///   M = number of circuit templates
///   K = number of parameter updates per template
///   D = circuit depth (number of QV layers)
///   shots = measurements per circuit
pub async fn qcvv_clops(Json(body): Json<Value>) -> impl IntoResponse {
    let m_templates   = body["m_templates"].as_f64().unwrap_or(100.0);
    let k_params      = body["k_params"].as_f64().unwrap_or(10.0);
    let depth         = body["depth"].as_f64().unwrap_or(10.0);
    let shots         = body["shots"].as_f64().unwrap_or(100.0);
    let exec_time_s   = body["execution_time_s"].as_f64().unwrap_or(1.0);

    let clops = (m_templates * k_params * depth * shots) / exec_time_s;

    // Effective gate throughput: gates per second assuming depth/2 2Q gates/layer
    let n_qubits = body["n_qubits"].as_f64().unwrap_or(10.0);
    let gates_per_circuit = depth * n_qubits / 2.0;
    let total_circuits = m_templates * k_params;
    let gate_throughput = total_circuits * shots * gates_per_circuit / exec_time_s;

    // Time per circuit
    let time_per_circuit_ms = exec_time_s / (total_circuits * shots) * 1_000.0;

    // CLOPS target comparison (IBM Eagle R2: ~2000 CLOPS)
    let eagle_clops = 2000.0;
    let ratio_vs_eagle = clops / eagle_clops;

    Json(json!({
        "clops":                    clops.round() as u64,
        "gate_throughput_per_sec":  gate_throughput.round() as u64,
        "time_per_circuit_us":      (time_per_circuit_ms * 1000.0).round() / 1000.0,
        "ratio_vs_eagle_r2":        (ratio_vs_eagle * 100.0).round() / 100.0,
        "inputs": {
            "m_templates": m_templates,
            "k_params": k_params,
            "depth": depth,
            "shots": shots,
            "execution_time_s": exec_time_s,
            "n_qubits": n_qubits,
        }
    }))
}

/// POST /qcvv/rb-analysis
///
/// Analyses Randomised Benchmarking (RB) survival probability data to extract
/// gate error rate and separate coherent vs. incoherent contributions.
///
/// Input:
///   cliff_lengths: [0, 10, 20, 50, 100, 200]
///   survival_probs: [0.98, 0.85, 0.74, 0.55, 0.38, 0.19]
///   rb_type: "standard" | "interleaved"
///   reference_fidelity: 0.999   (only for interleaved RB)
pub async fn qcvv_rb_analysis(Json(body): Json<Value>) -> impl IntoResponse {
    let lengths: Vec<f64> = body["cliff_lengths"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_else(|| vec![0.0, 10.0, 20.0, 50.0, 100.0, 200.0]);

    let probs: Vec<f64> = body["expectation_values"]
        .as_array()
        .or_else(|| body["survival_probs"].as_array())
        .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_else(|| vec![0.98, 0.85, 0.74, 0.55, 0.38, 0.19]);

    let rb_type = body["rb_type"].as_str().unwrap_or("standard");
    let ref_fidelity = body["reference_fidelity"].as_f64().unwrap_or(1.0);

    if lengths.len() < 2 || probs.len() < 2 || lengths.len() != probs.len() {
        return Json(json!({
            "error": "cliff_lengths and survival_probs must have equal length (≥2)"
        }));
    }

    // Fit P(m) = A·p^m + B via linear regression on log scale
    // log(P - B) = log(A) + m·log(p)
    // Estimate B ≈ 1/d (d=2 for single qubit) or use last point as offset
    let d = 2.0_f64; // single-qubit Clifford group dimension
    let b_offset = 1.0 / d;

    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut sum_xx = 0.0f64;
    let mut sum_xy = 0.0f64;
    let mut valid = 0usize;

    for i in 0..lengths.len() {
        let p_adj = probs[i] - b_offset;
        if p_adj <= 0.0 { continue; }
        let x = lengths[i];
        let y = p_adj.ln();
        sum_x += x;
        sum_y += y;
        sum_xx += x * x;
        sum_xy += x * y;
        valid += 1;
    }

    if valid < 2 {
        return Json(json!({"error": "insufficient valid data points for fit"}));
    }

    let n_f = valid as f64;
    let log_p = (n_f * sum_xy - sum_x * sum_y) / (n_f * sum_xx - sum_x * sum_x);
    let p_rb = log_p.exp().clamp(0.0, 1.0);

    // EPC (error per Clifford): EPC = (d-1)/d × (1 - p)
    let epc = (d - 1.0) / d * (1.0 - p_rb);

    // Average gate fidelity: F_avg = 1 - EPC
    let f_avg = 1.0 - epc;

    // For interleaved RB: gate fidelity of the interleaved gate
    let (gate_epc, gate_fidelity) = if rb_type == "interleaved" {
        let p_ref = 1.0 - (d / (d - 1.0)) * (1.0 - ref_fidelity);
        let gate_p = p_rb / p_ref.max(1e-9);
        let g_epc = (d - 1.0) / d * (1.0 - gate_p.clamp(0.0, 1.0));
        let g_fid = 1.0 - g_epc;
        (Some(g_epc), Some(g_fid))
    } else {
        (None, None)
    };

    // T1/T2 estimate from EPC (rough): EPC ≈ t_gate / T_eff
    // → T_eff ≈ t_gate_ns / (EPC × 1e6) us
    let gate_time_ns = body["gate_time_ns"].as_f64().unwrap_or(40.0);
    let t_eff_est_us = if epc > 1e-9 {
        Some((gate_time_ns / (epc * 1e3)).round() / 1000.0) // in µs
    } else {
        None
    };

    let mut result = json!({
        "depolarising_parameter_p": (p_rb * 1e8).round() / 1e8,
        "error_per_clifford": (epc * 1e8).round() / 1e8,
        "average_gate_fidelity": (f_avg * 1e8).round() / 1e8,
        "t_eff_estimate_us": t_eff_est_us,
        "rb_type": rb_type,
        "n_points": lengths.len(),
        "cliff_lengths": lengths,
        "survival_probs": probs,
    });

    if let (Some(g_epc), Some(g_fid)) = (gate_epc, gate_fidelity) {
        result["interleaved_gate_epc"] = json!((g_epc * 1e8).round() / 1e8);
        result["interleaved_gate_fidelity"] = json!((g_fid * 1e8).round() / 1e8);
    }

    Json(result)
}
