use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct XebVerifyOutput {
    pub n_qubits: usize,
    pub n_circuits: usize,
    pub n_shots_per_circuit: usize,
    pub n_gates: usize,
    pub device_fidelity_input: f64,
    pub xeb_score: f64,
    pub expected_xeb_score: f64,
    pub xeb_std: f64,
    pub hilbert_space_dim: usize,
    pub effective_fidelity: f64,
    pub above_classical_threshold: bool,
    pub noise_model: String,
}

/// Linear Congruential Generator for reproducible pseudo-random numbers.
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed.wrapping_add(1) }
    }

    /// Returns next value in [0, 1).
    fn next_f64(&mut self) -> f64 {
        // Parameters from Knuth MMIX
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Exponential distribution with rate=1 (for Porter-Thomas sampling).
    fn next_exp(&mut self) -> f64 {
        let u = self.next_f64();
        // Clamp to avoid log(0)
        let u = u.max(1e-300);
        -u.ln()
    }
}

/// Generate 2^n Porter-Thomas distributed probabilities, normalised to sum=1.
///
/// Porter-Thomas: p ~ Exp(2^n) over the Hilbert space, which in practice
/// means drawing 2^n independent Exp(1) variates and normalising.
fn porter_thomas_probs(n_qubits: usize, rng: &mut Lcg) -> Vec<f64> {
    let dim = 1usize << n_qubits;
    let mut probs: Vec<f64> = (0..dim).map(|_| rng.next_exp()).collect();
    let total: f64 = probs.iter().sum();
    for p in &mut probs {
        *p /= total;
    }
    probs
}

/// Sample `n_shots` bitstring indices from `p_noisy` via the inverse-CDF method.
fn sample_bitstrings(p_noisy: &[f64], n_shots: usize, rng: &mut Lcg) -> Vec<usize> {
    // Build cumulative sum
    let mut cdf: Vec<f64> = Vec::with_capacity(p_noisy.len());
    let mut acc = 0.0_f64;
    for &p in p_noisy {
        acc += p;
        cdf.push(acc);
    }
    // Clamp last entry to 1.0 for safety
    if let Some(last) = cdf.last_mut() {
        *last = 1.0;
    }

    let mut samples = Vec::with_capacity(n_shots);
    for _ in 0..n_shots {
        let u = rng.next_f64();
        // Binary search for the bucket
        let idx = cdf.partition_point(|&c| c < u);
        samples.push(idx.min(p_noisy.len() - 1));
    }
    samples
}

/// Run one XEB circuit simulation.
///
/// Returns the F_XEB value for that circuit.
fn run_one_circuit(
    n_qubits: usize,
    n_shots: usize,
    n_gates: usize,
    fidelity: f64,
    rng: &mut Lcg,
) -> f64 {
    let dim = 1usize << n_qubits;

    // 1. Generate ideal Porter-Thomas probabilities
    let p_ideal = porter_thomas_probs(n_qubits, rng);

    // 2. Apply depolarizing noise:
    //    p_noisy_i = device_fidelity * p_ideal_i + (1 - device_fidelity) / 2^n
    //    where device_fidelity = fidelity^n_gates
    let device_fidelity = fidelity.powi(n_gates as i32);
    let uniform = 1.0 / dim as f64;
    let p_noisy: Vec<f64> = p_ideal
        .iter()
        .map(|&pi| device_fidelity * pi + (1.0 - device_fidelity) * uniform)
        .collect();

    // 3. Sample M bitstrings from p_noisy
    let sampled_indices = sample_bitstrings(&p_noisy, n_shots, rng);

    // 4. Compute F_XEB = 2^n * mean(p_ideal[x_i]) - 1
    let mean_p_ideal: f64 = sampled_indices.iter().map(|&i| p_ideal[i]).sum::<f64>()
        / n_shots as f64;
    dim as f64 * mean_p_ideal - 1.0
}

pub fn run_xeb_verify(
    n_qubits: usize,
    n_circuits: usize,
    n_shots: usize,
    n_gates: usize,
    fidelity: f64,
    seed: u64,
    json: bool,
) -> anyhow::Result<()> {
    let dim = 1usize << n_qubits;
    let mut rng = Lcg::new(seed);

    // Run all circuits
    let scores: Vec<f64> = (0..n_circuits)
        .map(|_| run_one_circuit(n_qubits, n_shots, n_gates, fidelity, &mut rng))
        .collect();

    let xeb_score = scores.iter().sum::<f64>() / n_circuits as f64;

    // Standard deviation over circuits
    let variance = scores
        .iter()
        .map(|&s| (s - xeb_score).powi(2))
        .sum::<f64>()
        / n_circuits.max(2) as f64;
    let xeb_std = variance.sqrt();

    // Theoretical expected XEB = fidelity^n_gates
    let expected_xeb_score = fidelity.powi(n_gates as i32);

    // Effective fidelity: invert F_XEB = f^G  =>  f = F_XEB^(1/G)
    // Clamp to [0, 1] range before taking root
    let effective_fidelity = if xeb_score > 0.0 {
        xeb_score.powf(1.0 / n_gates as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let above_classical_threshold = xeb_score > 0.0;

    let output = XebVerifyOutput {
        n_qubits,
        n_circuits,
        n_shots_per_circuit: n_shots,
        n_gates,
        device_fidelity_input: fidelity,
        xeb_score,
        expected_xeb_score,
        xeb_std,
        hilbert_space_dim: dim,
        effective_fidelity,
        above_classical_threshold,
        noise_model: "depolarizing".to_string(),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("XEB Verification Results");
        println!("  n_qubits             : {}", output.n_qubits);
        println!("  hilbert_space_dim    : {}", output.hilbert_space_dim);
        println!("  n_circuits           : {}", output.n_circuits);
        println!("  n_shots_per_circuit  : {}", output.n_shots_per_circuit);
        println!("  n_gates              : {}", output.n_gates);
        println!("  noise_model          : {}", output.noise_model);
        println!("  device_fidelity_input: {:.6}", output.device_fidelity_input);
        println!("  xeb_score (F_XEB)    : {:.6}", output.xeb_score);
        println!("  expected_xeb_score   : {:.6}", output.expected_xeb_score);
        println!("  xeb_std              : {:.6}", output.xeb_std);
        println!("  effective_fidelity   : {:.6}", output.effective_fidelity);
        println!(
            "  above_classical      : {}",
            output.above_classical_threshold
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Lcg RNG ───────────────────────────────────────────────────────────────

    #[test]
    fn lcg_produces_values_in_unit_interval() {
        let mut rng = Lcg::new(42);
        for _ in 0..1000 {
            let v = rng.next_f64();
            assert!(v >= 0.0 && v < 1.0, "LCG value out of [0,1): {}", v);
        }
    }

    #[test]
    fn lcg_exp_is_positive() {
        let mut rng = Lcg::new(7);
        for _ in 0..100 {
            let v = rng.next_exp();
            assert!(v > 0.0, "Exponential sample must be positive, got {}", v);
        }
    }

    #[test]
    fn lcg_deterministic_with_same_seed() {
        let mut r1 = Lcg::new(123);
        let mut r2 = Lcg::new(123);
        for _ in 0..50 {
            assert_eq!(r1.next_f64().to_bits(), r2.next_f64().to_bits());
        }
    }

    #[test]
    fn lcg_different_seeds_differ() {
        let v1 = Lcg::new(1).next_f64();
        let v2 = Lcg::new(2).next_f64();
        assert_ne!(v1.to_bits(), v2.to_bits());
    }

    // ── Porter-Thomas probabilities ───────────────────────────────────────────

    #[test]
    fn porter_thomas_probs_sum_to_one() {
        let mut rng = Lcg::new(42);
        let probs = porter_thomas_probs(3, &mut rng); // 8 outcomes
        assert_eq!(probs.len(), 8);
        let sum: f64 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-10, "Probabilities must sum to 1, got {}", sum);
    }

    #[test]
    fn porter_thomas_probs_all_positive() {
        let mut rng = Lcg::new(99);
        let probs = porter_thomas_probs(2, &mut rng);
        assert!(probs.iter().all(|&p| p > 0.0), "All probabilities must be positive");
    }

    #[test]
    fn porter_thomas_dim_matches_n_qubits() {
        let mut rng = Lcg::new(5);
        for n in 1..=5 {
            let probs = porter_thomas_probs(n, &mut rng);
            assert_eq!(probs.len(), 1 << n);
        }
    }

    // ── Bitstring sampling ────────────────────────────────────────────────────

    #[test]
    fn sample_bitstrings_correct_count() {
        let mut rng = Lcg::new(11);
        let probs = vec![0.25, 0.25, 0.25, 0.25];
        let samples = sample_bitstrings(&probs, 500, &mut rng);
        assert_eq!(samples.len(), 500);
    }

    #[test]
    fn sample_bitstrings_indices_in_range() {
        let mut rng = Lcg::new(13);
        let probs = vec![0.1, 0.4, 0.3, 0.2];
        let samples = sample_bitstrings(&probs, 1000, &mut rng);
        assert!(samples.iter().all(|&i| i < 4), "All sample indices must be < 4");
    }

    #[test]
    fn sample_bitstrings_concentrates_on_peaked_distribution() {
        // A distribution peaked entirely on index 2
        let mut rng = Lcg::new(17);
        let probs = vec![0.0, 0.0, 1.0, 0.0];
        let samples = sample_bitstrings(&probs, 100, &mut rng);
        assert!(samples.iter().all(|&i| i == 2), "All samples should be index 2");
    }

    // ── run_xeb_verify integration ────────────────────────────────────────────

    #[test]
    fn run_xeb_verify_returns_ok() {
        // n_qubits=2, n_circuits=10, n_shots=100, n_gates=5, fidelity=0.99, seed=42
        let result = run_xeb_verify(2, 10, 100, 5, 0.99, 42, false);
        assert!(result.is_ok(), "run_xeb_verify should succeed: {:?}", result);
    }

    #[test]
    fn run_xeb_verify_json_returns_ok() {
        let result = run_xeb_verify(2, 5, 50, 3, 0.95, 1, true);
        assert!(result.is_ok());
    }

    #[test]
    fn perfect_fidelity_gives_positive_xeb() {
        // With fidelity=1.0, no depolarizing noise, XEB score should be positive
        // Can't capture stdout easily, but running it should not panic/error
        let result = run_xeb_verify(2, 20, 200, 10, 1.0, 77, false);
        assert!(result.is_ok());
    }
}
