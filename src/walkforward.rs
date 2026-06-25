//! Harness walk-forward (la medición honesta multi-régimen). Recorre ~2 años de
//! histórico en folds deslizantes: entrena en una ventana y mide en la SIGUIENTE
//! (intocada), luego rueda hacia adelante. Cada fold cae en un régimen distinto
//! (alcista/bajista/lateral), así que distingue habilidad real de "ser defensivo
//! con suerte". Criterio de éxito: "Riesgo primero" (Sharpe OOS > 0 y drawdown
//! bajo un tope).
//!
//! No forma parte del daemon: se dispara con MCPATO_WF_CHECK=true y termina.

use anyhow::{bail, Result};
use rand::{rngs::StdRng, SeedableRng};

use crate::{
    agent::simulate_agent_warmup,
    config::Config,
    evolution,
    models::{Candle, Genome},
    rest_binance,
};

/// Velas de calentamiento antes de cada ventana, para que las features de
/// horizonte largo (momentum 200, EMA 100) no salgan frías.
const WARMUP: usize = 200;

struct FoldReport {
    regime: &'static str,
    champ_return: f64,
    alpha: f64,
    sharpe: f64,
    max_dd: f64,
    passed: bool,
}

pub async fn run(cfg: Config) -> Result<()> {
    let total_days = env_u32("MCPATO_WF_DAYS", 720);
    let train_days = env_u32("MCPATO_WF_TRAIN_DAYS", 120) as usize;
    let test_days = env_u32("MCPATO_WF_TEST_DAYS", 30) as usize;
    let n_seeds = env_u32("MCPATO_WF_SEEDS", 3) as u64;
    let dd_cap = env_f64("MCPATO_WF_DD_CAP", 0.15);
    let regime_band = 0.05; // ±5% en el test define alcista/bajista/lateral.

    println!(
        "\n=== WALK-FORWARD · {} días · train {}d / test {}d · {} semillas/fold ===",
        total_days, train_days, test_days, n_seeds
    );
    println!(
        "Criterio 'Riesgo primero': PASA un fold si Sharpe_OOS > 0 y maxDD <= {:.0}%\n",
        dd_cap * 100.0
    );

    let end = chrono::Utc::now();
    let start = end - chrono::Duration::days(total_days as i64);
    let candles = rest_binance::fetch_klines_range(
        &cfg.symbol,
        &cfg.interval,
        start.timestamp_millis(),
        end.timestamp_millis(),
    )
    .await?;

    let day = cfg.candles_per_day.max(1);
    let train_len = train_days * day;
    let test_len = test_days * day;
    if candles.len() < train_len + test_len {
        bail!(
            "histórico insuficiente: {} velas (se necesitan al menos {})",
            candles.len(),
            train_len + test_len
        );
    }

    // Config para la evaluación OOS: una sola ventana continua (como en vivo).
    let mut cfg_eval = cfg.clone();
    cfg_eval.eval_windows = 1;

    let mut folds: Vec<FoldReport> = Vec::new();
    let mut fold_idx = 0usize;
    let mut test_start = train_len;

    while test_start + test_len <= candles.len() {
        let train = &candles[test_start - train_len..test_start];
        let test = &candles[test_start..test_start + test_len];
        fold_idx += 1;

        let bh_return = test.last().unwrap().close / test.first().unwrap().close - 1.0;
        let regime = if bh_return > regime_band {
            "alcista"
        } else if bh_return < -regime_band {
            "bajista"
        } else {
            "lateral"
        };

        // Promedio entre semillas para no concluir desde una corrida con suerte.
        let mut ret_acc = 0.0;
        let mut sharpe_acc = 0.0;
        let mut dd_acc = 0.0;
        // Warm-up para la evaluación OOS: la cola del train, justo antes del test.
        let oos_warmup = &candles[test_start.saturating_sub(WARMUP)..test_start];
        for seed in 1..=n_seeds {
            let mut rng = StdRng::seed_from_u64(seed + fold_idx as u64 * 1000);
            let champion = train_champion(&mut rng, train, &cfg);
            let res = simulate_agent_warmup(champion, oos_warmup, test, &cfg_eval, 0);
            ret_acc += res.equity_final / 100.0 - 1.0;
            sharpe_acc += res.sharpe;
            dd_acc += res.max_drawdown;
        }
        let nf = n_seeds as f64;
        let champ_return = ret_acc / nf;
        let sharpe = sharpe_acc / nf;
        let max_dd = dd_acc / nf;
        let passed = sharpe > 0.0 && max_dd <= dd_cap;

        println!(
            "fold {:>2} [{:^8}]  B&H {:+6.1}%  champion {:+6.1}%  alpha {:+6.1}%  Sharpe {:+5.2}  maxDD {:4.1}%  -> {}",
            fold_idx,
            regime,
            bh_return * 100.0,
            champ_return * 100.0,
            (champ_return - bh_return) * 100.0,
            sharpe,
            max_dd * 100.0,
            if passed { "PASA" } else { "falla" }
        );

        folds.push(FoldReport {
            regime,
            champ_return,
            alpha: champ_return - bh_return,
            sharpe,
            max_dd,
            passed,
        });

        test_start += test_len;
    }

    summarize(&folds, dd_cap);
    Ok(())
}

/// Entrena el motor evolutivo sobre `train` (ventanas de `candles_per_day`, cada
/// una precalentada con las velas previas) y devuelve el campeón final.
fn train_champion(rng: &mut StdRng, train: &[Candle], cfg: &Config) -> Genome {
    let (mut champion, mut population) = evolution::bootstrap_population(rng, cfg);
    let day = cfg.candles_per_day.max(1);
    let mut gen = 0i64;
    let mut start = 0usize;
    while start + day <= train.len() {
        let warmup = &train[start.saturating_sub(WARMUP)..start];
        let scored = &train[start..start + day];
        gen += 1;
        let outcome =
            evolution::evaluate_generation_warmup(rng, warmup, scored, &population, cfg, gen);
        champion = outcome.next_champion.clone();
        population = outcome.next_population;
        start += day;
    }
    champion
}

fn summarize(folds: &[FoldReport], dd_cap: f64) {
    if folds.is_empty() {
        println!("\nSin folds evaluados.");
        return;
    }
    let n = folds.len();
    let passed = folds.iter().filter(|f| f.passed).count();
    let beat_bh = folds.iter().filter(|f| f.alpha > 0.0).count();
    let positive = folds.iter().filter(|f| f.champ_return > 0.0).count();
    let avg_sharpe = folds.iter().map(|f| f.sharpe).sum::<f64>() / n as f64;
    let avg_dd = folds.iter().map(|f| f.max_dd).sum::<f64>() / n as f64;

    println!("\n=== RESUMEN WALK-FORWARD ({n} folds) ===");
    println!("PASAN criterio (Sharpe>0 y maxDD<={:.0}%): {}/{}", dd_cap * 100.0, passed, n);
    println!("Baten buy&hold (alpha>0):                 {}/{}", beat_bh, n);
    println!("Retorno OOS positivo:                     {}/{}", positive, n);
    println!("Sharpe OOS medio: {:+.2}   ·   maxDD medio: {:.1}%", avg_sharpe, avg_dd * 100.0);

    // Desglose por régimen: ¿gana solo en bajistas (defensivo) o también en alcistas (skill)?
    println!("\nPor régimen (pasan / total · alpha medio):");
    for regime in ["alcista", "lateral", "bajista"] {
        let group: Vec<&FoldReport> = folds.iter().filter(|f| f.regime == regime).collect();
        if group.is_empty() {
            continue;
        }
        let g_pass = group.iter().filter(|f| f.passed).count();
        let g_alpha = group.iter().map(|f| f.alpha).sum::<f64>() / group.len() as f64;
        println!(
            "  {:^8}: {}/{}   alpha medio {:+.1}%",
            regime,
            g_pass,
            group.len(),
            g_alpha * 100.0
        );
    }

    // Veredicto honesto, centrado en la pregunta clave: ¿habilidad o suerte defensiva?
    let alcista: Vec<&FoldReport> = folds.iter().filter(|f| f.regime == "alcista").collect();
    let alpha_alcista_ok = !alcista.is_empty()
        && alcista.iter().filter(|f| f.alpha > 0.0).count() as f64 >= 0.5 * alcista.len() as f64;
    let pass_rate = passed as f64 / n as f64;

    println!("\nVeredicto:");
    if pass_rate >= 0.6 && alpha_alcista_ok {
        println!("  Indicios de HABILIDAD real: cumple 'Riesgo primero' en la mayoría de folds");
        println!("  y bate al mercado TAMBIÉN en alcistas (no solo por ser defensivo).");
        println!("  -> Vale la pena invertir en CMA-ES para explotar la ventaja.");
    } else if pass_rate >= 0.6 {
        println!("  Cumple 'Riesgo primero' (preserva capital) pero NO bate al mercado en");
        println!("  alcistas: la 'ventaja' es sobre todo ser defensivo, no predecir.");
        println!("  -> Mejorar features/modelo antes que el optimizador.");
    } else {
        println!("  NO cumple el criterio de riesgo de forma consistente.");
        println!("  -> El trabajo está en features/modelo/costes, no en el optimizador.");
    }
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}
