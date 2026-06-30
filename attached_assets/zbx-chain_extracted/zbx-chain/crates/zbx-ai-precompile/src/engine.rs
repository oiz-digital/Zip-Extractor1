//! INT8 Quantized Inference Engine — fully deterministic, no floating point.
//!
//! All arithmetic is performed in i32/i64 with fixed-point scaling.
//! Every validator runs the same code → same output → consensus safe.
//!
//! Architecture: Feed-Forward Network with ReLU activations.
//!   Input → Linear(W, b) → ReLU → Linear(W, b) → Softmax (int approx)

use crate::error::AiError;

/// Fixed-point scale factor: 1.0 in float = 128 in INT8 (Q7 format).
pub const SCALE: i32 = 128;
pub const SCALE_I64: i64 = 128;

/// Maximum layer size to prevent stack overflow.
pub const MAX_LAYER_SIZE: usize = 256;

/// A single INT8 linear layer: output = W*input + b, clamped to i8 range.
#[derive(Debug, Clone)]
pub struct Int8Linear {
    /// Weight matrix: [out_features][in_features], row-major.
    pub weights: Vec<Vec<i8>>,
    /// Bias vector: [out_features], stored as i32 (pre-scaled).
    pub biases:  Vec<i32>,
    pub out_size: usize,
    pub in_size:  usize,
}

impl Int8Linear {
    pub fn new(weights: Vec<Vec<i8>>, biases: Vec<i32>) -> Result<Self, AiError> {
        let out_size = weights.len();
        if out_size == 0 || out_size > MAX_LAYER_SIZE {
            return Err(AiError::InvalidModelWeights("layer out_size out of range".into()));
        }
        let in_size = weights[0].len();
        if in_size == 0 || in_size > MAX_LAYER_SIZE {
            return Err(AiError::InvalidModelWeights("layer in_size out of range".into()));
        }
        if biases.len() != out_size {
            return Err(AiError::InvalidModelWeights("bias/weight size mismatch".into()));
        }
        for row in &weights {
            if row.len() != in_size {
                return Err(AiError::InvalidModelWeights("weight row length mismatch".into()));
            }
        }
        Ok(Self { weights, biases, out_size, in_size })
    }

    /// Forward pass: output[i] = Σ(W[i][j] * input[j]) + bias[i]
    /// Result is in i32 (not yet scaled back to i8).
    pub fn forward(&self, input: &[i8]) -> Result<Vec<i32>, AiError> {
        if input.len() != self.in_size {
            return Err(AiError::InputSizeMismatch {
                expected: self.in_size,
                got: input.len(),
            });
        }
        let mut out = Vec::with_capacity(self.out_size);
        for (row, bias) in self.weights.iter().zip(self.biases.iter()) {
            let mut acc: i64 = *bias as i64;
            for (w, x) in row.iter().zip(input.iter()) {
                acc += (*w as i64) * (*x as i64);
            }
            // Descale: divide by SCALE to stay in i32 range
            let val = (acc / SCALE_I64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            out.push(val);
        }
        Ok(out)
    }
}

/// ReLU activation on i32 vector (clamp negatives to 0).
pub fn relu_i32(v: &[i32]) -> Vec<i32> {
    v.iter().copied().map(|x| x.max(0)).collect()
}

/// Clamp i32 to i8 range and cast.
pub fn quantize_i8(v: &[i32]) -> Vec<i8> {
    v.iter().copied().map(|x| x.clamp(-128, 127) as i8).collect()
}

/// Integer approximate softmax — returns probabilities as u16 basis points (sum ≈ 10000).
/// Uses shift-based approximation: no division, no floats.
pub fn softmax_bps(logits: &[i32]) -> Vec<u16> {
    if logits.is_empty() { return vec![]; }
    // Normalize by max to prevent overflow
    let max = *logits.iter().max().unwrap_or(&0);
    // Use linear approximation: exp(x) ≈ 1 + x for small x (shifted)
    let shifted: Vec<i64> = logits.iter().map(|&l| {
        let diff = (l - max).clamp(-127, 0) as i64;
        (SCALE_I64 + diff).max(1) // always >= 1 to avoid div by zero
    }).collect();
    let total: i64 = shifted.iter().sum();
    shifted.iter().map(|&s| {
        ((s * 10_000) / total).clamp(0, 10_000) as u16
    }).collect()
}

/// Full 2-layer INT8 neural network.
pub struct Int8Network {
    pub layer1: Int8Linear,
    pub layer2: Int8Linear,
}

impl Int8Network {
    pub fn new(l1: Int8Linear, l2: Int8Linear) -> Self {
        Self { layer1: l1, layer2: l2 }
    }

    /// Forward pass through the network.
    /// Returns (class_index, confidence_bps).
    pub fn infer(&self, input: &[i8]) -> Result<(u8, u16), AiError> {
        let h1_raw = self.layer1.forward(input)?;
        let h1_act = relu_i32(&h1_raw);
        let h1_q   = quantize_i8(&h1_act);
        let h2_raw = self.layer2.forward(&h1_q)?;
        let probs  = softmax_bps(&h2_raw);
        if probs.is_empty() {
            return Err(AiError::Inference("empty output layer".into()));
        }
        let (best_class, best_prob) = probs.iter().enumerate()
            .max_by_key(|(_, &p)| p)
            .map(|(i, &p)| (i as u8, p))
            .unwrap_or((0, 0));
        Ok((best_class, best_prob))
    }
}

/// Deterministic stub network: derives weights from model_id byte.
/// Used in `stub` feature — all validators get the same weights → deterministic.
pub fn stub_network(model_id: u8, in_size: usize, hidden: usize, out_size: usize)
    -> Result<Int8Network, AiError>
{
    // Deterministic weight init: w[i][j] = ((model_id ^ i ^ j) as i8) % 8
    let w1: Vec<Vec<i8>> = (0..hidden).map(|i| {
        (0..in_size).map(|j| {
            let v = (model_id as u32 ^ i as u32 ^ j as u32) % 16;
            (v as i8) - 8  // range -8..7
        }).collect()
    }).collect();
    let b1: Vec<i32> = (0..hidden).map(|i| (i as i32 % 4) * SCALE).collect();

    let w2: Vec<Vec<i8>> = (0..out_size).map(|i| {
        (0..hidden).map(|j| {
            let v = (model_id as u32 ^ (i + 7) as u32 ^ j as u32) % 16;
            (v as i8) - 8
        }).collect()
    }).collect();
    let b2: Vec<i32> = (0..out_size).map(|i| (i as i32 % 3) * SCALE).collect();

    let l1 = Int8Linear::new(w1, b1)?;
    let l2 = Int8Linear::new(w2, b2)?;
    Ok(Int8Network::new(l1, l2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_forward_basic() {
        // 2-in, 2-out: identity-like
        let w = vec![vec![SCALE as i8, 0i8], vec![0i8, SCALE as i8]];
        // SCALE as i8 = 127 (clamped), so we use smaller weights
        let w2 = vec![vec![8i8, 0i8], vec![0i8, 8i8]];
        let b = vec![0i32, 0i32];
        let layer = Int8Linear::new(w2, b).unwrap();
        let out = layer.forward(&[16i8, 32i8]).unwrap();
        // 8*16/128 = 1, 8*32/128 = 2
        assert_eq!(out[0], 1);
        assert_eq!(out[1], 2);
    }

    #[test]
    fn relu_clamps_negatives() {
        let v = vec![-5i32, 0, 3, -100, 7];
        let r = relu_i32(&v);
        assert_eq!(r, vec![0, 0, 3, 0, 7]);
    }

    #[test]
    fn softmax_bps_sums_approx_10000() {
        let logits = vec![10i32, 5, 1, 20, 3];
        let probs = softmax_bps(&logits);
        let total: u32 = probs.iter().map(|&p| p as u32).sum();
        assert!(total >= 9990 && total <= 10010, "total = {total}");
    }

    #[test]
    fn stub_network_deterministic() {
        let n1 = stub_network(0x01, 8, 16, 4).unwrap();
        let n2 = stub_network(0x01, 8, 16, 4).unwrap();
        let input = vec![10i8; 8];
        let r1 = n1.infer(&input).unwrap();
        let r2 = n2.infer(&input).unwrap();
        assert_eq!(r1, r2, "Stub network must be deterministic");
    }

    #[test]
    fn stub_different_models_differ() {
        let n01 = stub_network(0x01, 8, 16, 4).unwrap();
        let n02 = stub_network(0x02, 8, 16, 4).unwrap();
        let input = vec![10i8; 8];
        let r1 = n01.infer(&input).unwrap();
        let r2 = n02.infer(&input).unwrap();
        // Different model IDs should produce different results
        assert_ne!(r1, r2);
    }

    #[test]
    fn confidence_in_range() {
        let n = stub_network(0x05, 16, 32, 8).unwrap();
        let input = vec![5i8; 16];
        let (_, conf) = n.infer(&input).unwrap();
        assert!(conf <= 10_000, "confidence {conf} > 10000");
    }
}
