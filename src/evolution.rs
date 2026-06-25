use rand::{distributions::WeightedIndex, prelude::Distribution, Rng};

use crate::{
    agent::simulate_agent_warmup,
    config::Config,
    models::{Candle, EvalResult, Genome},
    nn,
};

pub struct GenerationOutcome {
    pub results: Vec<EvalResult>,
    pub next_champion: Genome,
    pub next_population: Vec<Genome>,
    pub avg_fitness: f64,
    pub best_fitness: f64,
    pub survival_rate: f64,
    pub extinction_happened: bool,
}

pub fn evaluate_generation<R: Rng>(
    rng: &mut R,
    candles: &[Candle],
    population: &[Genome],
    cfg: &Config,
    generation_id: i64,
) -> GenerationOutcome {
    evaluate_generation_warmup(rng, &[], candles, population, cfg, generation_id)
}

/// Igual que `evaluate_generation` pero con un prefijo `warmup` para precalentar
/// los indicadores de cada agente (lo usa el harness/pre-entrenamiento; el bucle
/// en vivo usa la versión sin warm-up).
pub fn evaluate_generation_warmup<R: Rng>(
    rng: &mut R,
    warmup: &[Candle],
    scored: &[Candle],
    population: &[Genome],
    cfg: &Config,
    generation_id: i64,
) -> GenerationOutcome {
    let mut results: Vec<EvalResult> = population
        .iter()
        .cloned()
        .map(|g| simulate_agent_warmup(g, warmup, scored, cfg, generation_id))
        .collect();

    results.sort_by(|a, b| b.fitness.total_cmp(&a.fitness));

    let best = results[0].clone();
    let avg_fitness = results.iter().map(|r| r.fitness).sum::<f64>() / results.len() as f64;
    let best_fitness = best.fitness;
    let survivors = results.iter().filter(|r| r.survival_ratio >= 1.0).count() as f64;
    let survival_rate = survivors / results.len() as f64;
    let extinction_happened = survivors == 0.0;

    let next_population = if extinction_happened {
        let parent =
            pick_parent_for_extinction(rng, &results).unwrap_or_else(|| best.genome.clone());
        seed_population_from_parent(rng, &parent, cfg, true)
    } else {
        seed_population_from_parent(rng, &best.genome, cfg, false)
    };

    GenerationOutcome {
        results,
        next_champion: best.genome,
        next_population,
        avg_fitness,
        best_fitness,
        survival_rate,
        extinction_happened,
    }
}

pub fn bootstrap_population<R: Rng>(rng: &mut R, cfg: &Config) -> (Genome, Vec<Genome>) {
    let champion = nn::random_genome(rng);
    let mut pop = Vec::with_capacity(cfg.population_size);
    pop.push(champion.clone());
    while pop.len() < cfg.population_size {
        pop.push(nn::random_genome(rng));
    }
    (champion, pop)
}

pub fn mutate_genome<R: Rng>(
    rng: &mut R,
    genome: &Genome,
    cfg: &Config,
    radiation: bool,
) -> Genome {
    let mut out = genome.clone();
    for w in &mut out.weights {
        if rng.gen_bool(cfg.p_mut.clamp(0.0, 1.0)) {
            let mut sigma = sample_sigma(rng, cfg);
            if radiation {
                sigma *= 2.0;
            }
            *w += gaussian_like(rng) * sigma;
            *w = w.clamp(-3.0, 3.0);
        }
    }
    out
}

fn seed_population_from_parent<R: Rng>(
    rng: &mut R,
    parent: &Genome,
    cfg: &Config,
    radiation: bool,
) -> Vec<Genome> {
    let mut pop = Vec::with_capacity(cfg.population_size);
    pop.push(parent.clone());
    while pop.len() < cfg.population_size {
        pop.push(mutate_genome(rng, parent, cfg, radiation));
    }
    pop
}

fn sample_sigma<R: Rng>(rng: &mut R, cfg: &Config) -> f64 {
    let u = rng.gen::<f64>();
    if u < cfg.p_big {
        cfg.sigma_big
    } else if u < cfg.p_big + cfg.p_med {
        cfg.sigma_med
    } else {
        cfg.sigma_small
    }
}

fn gaussian_like<R: Rng>(rng: &mut R) -> f64 {
    let mut s = 0.0;
    for _ in 0..12 {
        s += rng.gen::<f64>();
    }
    s - 6.0
}

fn pick_parent_for_extinction<R: Rng>(rng: &mut R, results: &[EvalResult]) -> Option<Genome> {
    if results.is_empty() {
        return None;
    }
    let weights: Vec<f64> = results
        .iter()
        .map(|r| (r.lived_candles as f64 + 1.0).max(1.0))
        .collect();
    let dist = WeightedIndex::new(weights).ok()?;
    let idx = dist.sample(rng);
    Some(results[idx].genome.clone())
}
