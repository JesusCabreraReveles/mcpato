//! Medición out-of-sample honesta (el "primerísimo paso"): entrena el motor
//! evolutivo SOLO sobre la parte vieja del histórico y mide al campeón en la
//! parte final, que nunca vio, contra dos baselines tontos (buy&hold y cash).
//! Repite con varias semillas para no concluir desde una corrida con suerte.
//!
//! No forma parte del daemon: se dispara con MCPATO_OOS_CHECK=true y termina.

use anyhow::{bail, Result};
use rand::{rngs::StdRng, SeedableRng};

use crate::{
    agent::simulate_agent,
    config::Config,
    evolution,
    models::{Candle, Genome},
    rest_binance,
};

const SEEDS: [u64; 6] = [1, 2, 3, 4, 5, 6];
const TRAIN_FRACTION: f64 = 0.70;

pub async fn run(cfg: Config) -> Result<()> {
    let days: u32 = std::env::var("MCPATO_OOS_DAYS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(180);

    println!(
        "\n=== OOS CHECK · {} días · {} {} · velas/gen={} · eval_windows={} ===",
        days, cfg.symbol, cfg.interval, cfg.candles_per_day, cfg.eval_windows
    );

    let end = chrono::Utc::now();
    let start = end - chrono::Duration::days(days as i64);
    let candles = rest_binance::fetch_klines_range(
        &cfg.symbol,
        &cfg.interval,
        start.timestamp_millis(),
        end.timestamp_millis(),
    )
    .await?;

    if candles.len() < cfg.candles_per_day * 4 {
        bail!(
            "histórico insuficiente: {} velas (se necesitan al menos {})",
            candles.len(),
            cfg.candles_per_day * 4
        );
    }

    // Partición temporal: entrena en lo viejo, mide en lo nuevo (intocado).
    let split = ((candles.len() as f64) * TRAIN_FRACTION) as usize;
    let (train, test) = candles.split_at(split);
    println!(
        "Train: {:>6} velas  {} → {}",
        train.len(),
        train.first().unwrap().ts.date_naive(),
        train.last().unwrap().ts.date_naive()
    );
    println!(
        "Test : {:>6} velas  {} → {}   [intocado]\n",
        test.len(),
        test.first().unwrap().ts.date_naive(),
        test.last().unwrap().ts.date_naive()
    );

    // Baselines sobre el test.
    let bh_return = test.last().unwrap().close / test.first().unwrap().close - 1.0;
    println!("Baseline buy&hold (test): {:+.2}%", bh_return * 100.0);
    println!("Baseline cash     (test): +0.00%\n");

    // Config para la evaluación OOS: una sola ventana continua (como correría en vivo).
    let mut cfg_eval = cfg.clone();
    cfg_eval.eval_windows = 1;

    println!("Entrenando {} campeones (una semilla cada uno) y midiéndolos OOS...\n", SEEDS.len());

    let mut test_returns = Vec::with_capacity(SEEDS.len());
    let mut daily_beat_rates = Vec::with_capacity(SEEDS.len());

    for &seed in &SEEDS {
        let mut rng = StdRng::seed_from_u64(seed);
        let champion = train_champion(&mut rng, train, &cfg);

        // Métrica principal: simulación continua sobre todo el test.
        let res = simulate_agent(champion.clone(), test, &cfg_eval, 0);
        let ret = res.equity_final / 100.0 - 1.0;
        test_returns.push(ret);

        // Consistencia: % de días del test en que bate al buy&hold de ESE día.
        let beat = daily_beat_rate(&champion, test, &cfg_eval);
        daily_beat_rates.push(beat);

        println!(
            "seed {seed}:  retorno OOS {:+6.2}%   alpha {:+6.2}%   maxDD {:4.1}%   bate B&H {:4.0}% de los días",
            ret * 100.0,
            (ret - bh_return) * 100.0,
            res.max_drawdown * 100.0,
            beat * 100.0
        );
    }

    summarize(&test_returns, &daily_beat_rates, bh_return);
    Ok(())
}

/// Entrena el motor evolutivo sobre `train` (ventanas de `candles_per_day`) y
/// devuelve el campeón final — exactamente lo que el bot desplegaría.
fn train_champion(rng: &mut StdRng, train: &[Candle], cfg: &Config) -> Genome {
    let (mut champion, mut population) = evolution::bootstrap_population(rng, cfg);
    let mut gen = 0i64;
    for chunk in train.chunks(cfg.candles_per_day) {
        if chunk.len() < cfg.candles_per_day {
            break;
        }
        gen += 1;
        let outcome = evolution::evaluate_generation(rng, chunk, &population, cfg, gen);
        champion = outcome.next_champion.clone();
        population = outcome.next_population;
    }
    champion
}

/// Fracción de días del test en que el campeón (broker fresco por día) termina
/// por encima del buy&hold de ese mismo día.
fn daily_beat_rate(champion: &Genome, test: &[Candle], cfg: &Config) -> f64 {
    let mut days = 0u32;
    let mut wins = 0u32;
    for day in test.chunks(cfg.candles_per_day) {
        if day.len() < cfg.candles_per_day {
            break;
        }
        let res = simulate_agent(champion.clone(), day, cfg, 0);
        let champ_ret = res.equity_final / 100.0 - 1.0;
        let bh = day.last().unwrap().close / day.first().unwrap().close - 1.0;
        if champ_ret > bh {
            wins += 1;
        }
        days += 1;
    }
    if days == 0 {
        0.0
    } else {
        wins as f64 / days as f64
    }
}

fn summarize(test_returns: &[f64], daily_beat_rates: &[f64], bh_return: f64) {
    let n = test_returns.len() as f64;
    let mean = test_returns.iter().sum::<f64>() / n;
    let best = test_returns.iter().cloned().fold(f64::MIN, f64::max);
    let worst = test_returns.iter().cloned().fold(f64::MAX, f64::min);
    let beat_bh = test_returns.iter().filter(|&&r| r > bh_return).count();
    let positive = test_returns.iter().filter(|&&r| r > 0.0).count();
    let avg_daily_beat = daily_beat_rates.iter().sum::<f64>() / n;

    println!("\n=== RESUMEN OOS ({} semillas) ===", test_returns.len());
    println!(
        "Retorno champion:  medio {:+.2}%   (peor {:+.2}%  ·  mejor {:+.2}%)",
        mean * 100.0,
        worst * 100.0,
        best * 100.0
    );
    println!("Buy&hold (test):   {:+.2}%", bh_return * 100.0);
    println!(
        "Baten buy&hold:    {}/{} semillas",
        beat_bh,
        test_returns.len()
    );
    println!(
        "Retorno positivo:  {}/{} semillas",
        positive,
        test_returns.len()
    );
    println!(
        "Bate B&H intradía: {:.0}% de los días (media entre semillas)",
        avg_daily_beat * 100.0
    );

    let verdict = if beat_bh as f64 >= 0.66 * n && mean > bh_return {
        "Hay indicios de ventaja OOS → vale la pena montar el harness walk-forward completo."
    } else if positive as f64 >= 0.66 * n {
        "Gana dinero pero NO bate al mercado (sería sobre todo beta). Señal débil."
    } else {
        "Sin ventaja OOS: el campeón no generaliza. El trabajo está en features/modelo/costes, NO en el optimizador."
    };
    println!("\nVeredicto: {verdict}");
}
