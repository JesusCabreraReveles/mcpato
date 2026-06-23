use rand::Rng;

use crate::models::Genome;

pub const INPUT_SIZE: usize = 12;
pub const HIDDEN_SIZE: usize = 8;
pub const OUTPUT_SIZE: usize = 2;
pub const GENOME_LEN: usize =
    INPUT_SIZE * HIDDEN_SIZE + HIDDEN_SIZE + HIDDEN_SIZE * OUTPUT_SIZE + OUTPUT_SIZE;

pub fn random_genome<R: Rng>(rng: &mut R) -> Genome {
    let weights = (0..GENOME_LEN)
        .map(|_| rng.gen_range(-0.5f64..0.5f64))
        .collect();
    Genome { weights }
}

pub fn forward(genome: &Genome, features: &[f64; INPUT_SIZE]) -> (f64, f64) {
    let mut idx = 0;

    let mut hidden = [0.0f64; HIDDEN_SIZE];
    for h in 0..HIDDEN_SIZE {
        let mut sum = 0.0;
        for &x in features.iter().take(INPUT_SIZE) {
            sum += x * genome.weights[idx];
            idx += 1;
        }
        sum += genome.weights[idx];
        idx += 1;
        hidden[h] = sum.tanh();
    }

    let mut out = [0.0f64; OUTPUT_SIZE];
    for o in 0..OUTPUT_SIZE {
        let mut sum = 0.0;
        for &hval in hidden.iter().take(HIDDEN_SIZE) {
            sum += hval * genome.weights[idx];
            idx += 1;
        }
        sum += genome.weights[idx];
        idx += 1;
        out[o] = sum;
    }

    let signal = out[0].tanh();
    let risk = 1.0 / (1.0 + (-out[1]).exp());
    (signal, risk)
}
