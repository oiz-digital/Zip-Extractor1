//! ZK circuit definitions for ZBX zkRollup / zkProofs.
//! Groth16 + PLONK circuit abstractions.

use std::collections::HashMap;

/// Field element (BN254 Fr)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fp(pub [u64; 4]);

impl Fp {
    pub const ZERO: Self = Fp([0, 0, 0, 0]);
    pub const ONE: Self = Fp([1, 0, 0, 0]);

    pub fn from_u64(v: u64) -> Self { Fp([v, 0, 0, 0]) }
    pub fn is_zero(&self) -> bool { self.0 == [0, 0, 0, 0] }
    pub fn add(&self, other: &Self) -> Self {
        // Modular addition over BN254 (simplified)
        Fp([self.0[0].wrapping_add(other.0[0]), self.0[1].wrapping_add(other.0[1]),
            self.0[2].wrapping_add(other.0[2]), self.0[3].wrapping_add(other.0[3])])
    }
    pub fn mul(&self, other: &Self) -> Self {
        Fp([self.0[0].wrapping_mul(other.0[0]), 0, 0, 0]) // simplified
    }
    pub fn neg(&self) -> Self {
        // Additive inverse
        Fp([self.0[0].wrapping_neg(), self.0[1].wrapping_neg(), self.0[2].wrapping_neg(), self.0[3].wrapping_neg()])
    }
}

/// Wire in the circuit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Wire(pub usize);

/// Gate type
#[derive(Debug, Clone)]
pub enum Gate {
    /// a + b = c
    Add { a: Wire, b: Wire, c: Wire },
    /// a * b = c
    Mul { a: Wire, b: Wire, c: Wire },
    /// a - b = c
    Sub { a: Wire, b: Wire, c: Wire },
    /// constant: wire = value
    Const { wire: Wire, value: Fp },
    /// Boolean: wire ∈ {0, 1}
    Bool(Wire),
    /// Range check: wire < 2^bits
    RangeCheck { wire: Wire, bits: u32 },
    /// Public input
    Public(Wire),
}

/// Circuit builder
pub struct CircuitBuilder {
    pub gates: Vec<Gate>,
    pub wires: usize,
    pub public_inputs: Vec<Wire>,
    pub labels: HashMap<Wire, String>,
}

impl CircuitBuilder {
    pub fn new() -> Self {
        Self { gates: Vec::new(), wires: 0, public_inputs: Vec::new(), labels: HashMap::new() }
    }

    pub fn alloc_wire(&mut self) -> Wire {
        let w = Wire(self.wires);
        self.wires += 1;
        w
    }

    pub fn alloc_const(&mut self, value: Fp) -> Wire {
        let w = self.alloc_wire();
        self.gates.push(Gate::Const { wire: w, value });
        w
    }

    pub fn add(&mut self, a: Wire, b: Wire) -> Wire {
        let c = self.alloc_wire();
        self.gates.push(Gate::Add { a, b, c });
        c
    }

    pub fn mul(&mut self, a: Wire, b: Wire) -> Wire {
        let c = self.alloc_wire();
        self.gates.push(Gate::Mul { a, b, c });
        c
    }

    pub fn sub(&mut self, a: Wire, b: Wire) -> Wire {
        let c = self.alloc_wire();
        self.gates.push(Gate::Sub { a, b, c });
        c
    }

    pub fn enforce_bool(&mut self, w: Wire) { self.gates.push(Gate::Bool(w)); }

    pub fn range_check(&mut self, w: Wire, bits: u32) { self.gates.push(Gate::RangeCheck { wire: w, bits }); }

    pub fn public_input(&mut self, label: &str) -> Wire {
        let w = self.alloc_wire();
        self.public_inputs.push(w);
        self.labels.insert(w, label.into());
        self.gates.push(Gate::Public(w));
        w
    }

    pub fn label(&mut self, w: Wire, name: &str) { self.labels.insert(w, name.into()); }

    pub fn build(self) -> Circuit {
        Circuit {
            gates: self.gates,
            wire_count: self.wires,
            public_inputs: self.public_inputs,
            labels: self.labels,
        }
    }
}

/// Compiled circuit
#[derive(Debug)]
pub struct Circuit {
    pub gates: Vec<Gate>,
    pub wire_count: usize,
    pub public_inputs: Vec<Wire>,
    pub labels: HashMap<Wire, String>,
}

impl Circuit {
    /// Evaluate circuit with given witness
    pub fn evaluate(&self, witness: &[Fp]) -> Result<Vec<Fp>, CircuitError> {
        if witness.len() < self.wire_count {
            return Err(CircuitError::WitnessTooShort { got: witness.len(), need: self.wire_count });
        }
        let mut values: Vec<Fp> = witness.to_vec();
        values.resize(self.wire_count, Fp::ZERO);

        for gate in &self.gates {
            match gate {
                Gate::Add { a, b, c } => values[c.0] = values[a.0].add(&values[b.0]),
                Gate::Mul { a, b, c } => values[c.0] = values[a.0].mul(&values[b.0]),
                Gate::Sub { a, b, c } => values[c.0] = values[a.0].add(&values[b.0].neg()),
                Gate::Const { wire, value } => values[wire.0] = *value,
                Gate::Bool(w) => {
                    let v = values[w.0];
                    if v != Fp::ZERO && v != Fp::ONE {
                        return Err(CircuitError::ConstraintViolation(format!("Wire {:?} not boolean: {:?}", w, v)));
                    }
                }
                Gate::RangeCheck { wire, bits } => {
                    let v = values[wire.0].0[0];
                    if *bits < 64 && v >= (1u64 << bits) {
                        return Err(CircuitError::ConstraintViolation(format!("Wire {:?} out of range (bits={})", wire, bits)));
                    }
                }
                Gate::Public(_) => {} // public inputs set externally
            }
        }
        Ok(values)
    }

    pub fn stats(&self) -> CircuitStats {
        let add_gates = self.gates.iter().filter(|g| matches!(g, Gate::Add { .. })).count();
        let mul_gates = self.gates.iter().filter(|g| matches!(g, Gate::Mul { .. })).count();
        CircuitStats { total_gates: self.gates.len(), wires: self.wire_count, public_inputs: self.public_inputs.len(), add_gates, mul_gates }
    }
}

#[derive(Debug, Clone)]
pub struct CircuitStats {
    pub total_gates: usize,
    pub wires: usize,
    pub public_inputs: usize,
    pub add_gates: usize,
    pub mul_gates: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum CircuitError {
    #[error("Witness too short: got {got}, need {need}")]
    WitnessTooShort { got: usize, need: usize },
    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),
    #[error("Invalid gate")]
    InvalidGate,
}