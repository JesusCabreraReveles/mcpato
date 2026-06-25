//! Sonda mínima cross-sectional (el primer paso de #1, des-riesgado). En vez de
//! cronometrar UN activo, rankea un universo de monedas líquidas por momentum y
//! mantiene equal-weight las top-k, rebalanceando periódicamente, con costes
//! realistas. Pregunta barata antes del rediseño grande: ¿seleccionar por
//! momentum bate a (a) BTC buy&hold y (b) tener TODO el universo equiponderado?
//!
//! Regla simple a propósito (sin red neuronal todavía): si ni la premisa simple
//! tiene edge, la versión con ML tampoco lo tendrá. Gated por MCPATO_XS_CHECK.

use std::collections::HashMap;

use anyhow::{bail, Result};

use crate::{config::Config, rest_binance};

const DEFAULT_UNIVERSE: &str = "BTCUSDT,ETHUSDT,BNBUSDT,XRPUSDT,ADAUSDT,SOLUSDT,\
DOGEUSDT,LTCUSDT,LINKUSDT,DOTUSDT,AVAXUSDT,TRXUSDT,ATOMUSDT,ETCUSDT,BCHUSDT,XLMUSDT";

pub async fn run(cfg: Config) -> Result<()> {
    let universe: Vec<String> = std::env::var("MCPATO_XS_SYMBOLS")
        .unwrap_or_else(|_| DEFAULT_UNIVERSE.to_string())
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    let days = env_u32("MCPATO_XS_DAYS", 720);
    let interval = std::env::var("MCPATO_XS_INTERVAL").unwrap_or_else(|_| "1d".to_string());
    let lookback = env_usize("MCPATO_XS_LOOKBACK", 30); // pasos (días si interval=1d)
    let top_k = env_usize("MCPATO_XS_TOPK", 5);
    let rebalance = env_usize("MCPATO_XS_REBALANCE", 7).max(1);
    let commission = cfg.commission;

    println!(
        "\n=== CROSS-SECTIONAL MOMENTUM · {} monedas · {} días · {} ===",
        universe.len(),
        days,
        interval
    );
    println!(
        "Regla: long equal-weight top-{} por momentum a {} pasos · rebalanceo cada {} · comisión {:.2}%\n",
        top_k, lookback, rebalance, commission * 100.0
    );

    let end = chrono::Utc::now();
    let start = end - chrono::Duration::days(days as i64);
    let start_ms = start.timestamp_millis();
    let end_ms = end.timestamp_millis();

    // Descarga cada símbolo y construye un mapa ts->close.
    let mut series: Vec<(String, HashMap<i64, f64>)> = Vec::new();
    let mut grid_ts: Vec<i64> = Vec::new(); // rejilla temporal de referencia (la del 1er símbolo válido)
    for sym in &universe {
        match rest_binance::fetch_klines_range(sym, &interval, start_ms, end_ms).await {
            Ok(candles) if !candles.is_empty() => {
                if grid_ts.is_empty() {
                    grid_ts = candles.iter().map(|c| c.ts.timestamp_millis()).collect();
                }
                let map: HashMap<i64, f64> =
                    candles.iter().map(|c| (c.ts.timestamp_millis(), c.close)).collect();
                println!("  {:<10} {} velas", sym, map.len());
                series.push((sym.clone(), map));
            }
            Ok(_) => eprintln!("  {sym:<10} sin datos, se omite"),
            Err(e) => eprintln!("  {sym:<10} error, se omite: {e:#}"),
        }
    }

    if series.len() < top_k + 1 || grid_ts.len() < lookback + rebalance + 1 {
        bail!("datos insuficientes: {} símbolos, {} pasos", series.len(), grid_ts.len());
    }
    grid_ts.sort_unstable();
    let n = grid_ts.len();

    // Precios alineados a la rejilla, forward-fill. None hasta el primer listado.
    let mut prices: Vec<Vec<Option<f64>>> = Vec::with_capacity(series.len());
    for (_, map) in &series {
        let mut col = Vec::with_capacity(n);
        let mut last: Option<f64> = None;
        for &ts in &grid_ts {
            if let Some(&p) = map.get(&ts) {
                last = Some(p);
            }
            col.push(last);
        }
        prices.push(col);
    }

    // Estrategia y baselines.
    let strat = simulate(&prices, n, lookback, rebalance, commission, Selector::TopKMomentum(top_k));
    let eqw = simulate(&prices, n, lookback, rebalance, commission, Selector::EqualAll);
    let btc = btc_buy_hold(&series, &prices, &universe);

    report("Cross-sectional top-k", &strat);
    report("Equal-weight universo", &eqw);
    if let Some(btc) = &btc {
        report("BTC buy&hold", btc);
    }

    verdict(&strat, &eqw, btc.as_deref());
    Ok(())
}

enum Selector {
    TopKMomentum(usize),
    EqualAll,
}

/// Simula la cartera sobre la rejilla y devuelve la curva de equity (normalizada
/// a 1.0). Mark-to-market diario; rebalanceo cada `rebalance` pasos a equal-weight
/// del conjunto elegido, aplicando comisión sobre el turnover.
fn simulate(
    prices: &[Vec<Option<f64>>],
    n: usize,
    lookback: usize,
    rebalance: usize,
    commission: f64,
    selector: Selector,
) -> Vec<f64> {
    let m = prices.len();
    let mut qty = vec![0.0f64; m]; // unidades por símbolo
    let mut cash = 1.0f64;
    let mut equity_curve = Vec::with_capacity(n);

    for i in 0..n {
        // Mark-to-market.
        let mut eq = cash;
        for s in 0..m {
            if qty[s] != 0.0 {
                if let Some(p) = prices[s][i] {
                    eq += qty[s] * p;
                }
            }
        }
        equity_curve.push(eq);

        // ¿Toca rebalancear?
        if i >= lookback && (i - lookback) % rebalance == 0 {
            let selected = match &selector {
                Selector::TopKMomentum(k) => top_k_by_momentum(prices, i, lookback, *k),
                Selector::EqualAll => available(prices, i),
            };
            if selected.is_empty() {
                continue;
            }

            // Valor actual por símbolo y peso objetivo (equal-weight de `selected`).
            let mut cur_val = vec![0.0f64; m];
            for s in 0..m {
                if let Some(p) = prices[s][i] {
                    cur_val[s] = qty[s] * p;
                }
            }
            let target_each = eq / selected.len() as f64;
            let mut turnover = 0.0;
            for s in 0..m {
                let target = if selected.contains(&s) { target_each } else { 0.0 };
                turnover += (target - cur_val[s]).abs();
            }
            let cost = commission * turnover;
            let eq_after = (eq - cost).max(0.0);
            let target_each_after = eq_after / selected.len() as f64;

            // Aplica: solo los seleccionados (con precio) quedan en cartera.
            qty = vec![0.0; m];
            cash = 0.0;
            for &s in &selected {
                if let Some(p) = prices[s][i] {
                    if p > 0.0 {
                        qty[s] = target_each_after / p;
                    }
                }
            }
        }
    }

    equity_curve
}

/// Símbolos con precio disponible en `i`.
fn available(prices: &[Vec<Option<f64>>], i: usize) -> Vec<usize> {
    (0..prices.len()).filter(|&s| prices[s][i].is_some()).collect()
}

/// Top-k por momentum = retorno entre `i-lookback` e `i` (solo símbolos con ambos).
fn top_k_by_momentum(prices: &[Vec<Option<f64>>], i: usize, lookback: usize, k: usize) -> Vec<usize> {
    let mut moms: Vec<(usize, f64)> = Vec::new();
    for s in 0..prices.len() {
        if let (Some(now), Some(then)) = (prices[s][i], prices[s][i - lookback]) {
            if then > 0.0 {
                moms.push((s, now / then - 1.0));
            }
        }
    }
    moms.sort_by(|a, b| b.1.total_cmp(&a.1));
    moms.into_iter().take(k).map(|(s, _)| s).collect()
}

/// Curva de equity de BTC buy&hold (normalizada).
fn btc_buy_hold(
    series: &[(String, HashMap<i64, f64>)],
    prices: &[Vec<Option<f64>>],
    _universe: &[String],
) -> Option<Vec<f64>> {
    let idx = series.iter().position(|(s, _)| s == "BTCUSDT")?;
    let col = &prices[idx];
    let first = col.iter().flatten().next().copied()?;
    Some(col.iter().map(|p| p.unwrap_or(first) / first).collect())
}

struct Metrics {
    total_return: f64,
    ann_sharpe: f64,
    max_dd: f64,
}

fn metrics(equity: &[f64]) -> Metrics {
    let total_return = equity.last().copied().unwrap_or(1.0) - 1.0;

    // Sharpe anualizado desde retornos por paso (asumiendo pasos ~diarios).
    let mut rets = Vec::with_capacity(equity.len());
    for w in equity.windows(2) {
        if w[0] > 1e-12 {
            rets.push(w[1] / w[0] - 1.0);
        }
    }
    let ann_sharpe = if rets.len() > 1 {
        let mean = rets.iter().sum::<f64>() / rets.len() as f64;
        let var = rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / rets.len() as f64;
        let std = var.sqrt();
        if std > 1e-12 {
            (mean / std) * (365.0f64).sqrt()
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Máximo drawdown.
    let mut peak = f64::MIN;
    let mut max_dd = 0.0;
    for &e in equity {
        if e > peak {
            peak = e;
        }
        if peak > 0.0 {
            let dd = (peak - e) / peak;
            if dd > max_dd {
                max_dd = dd;
            }
        }
    }

    Metrics { total_return, ann_sharpe, max_dd }
}

fn report(name: &str, equity: &[f64]) {
    let m = metrics(equity);
    println!(
        "{:<24} retorno {:+8.1}%   Sharpe(anual) {:+5.2}   maxDD {:5.1}%",
        name,
        m.total_return * 100.0,
        m.ann_sharpe,
        m.max_dd * 100.0
    );
}

fn verdict(strat: &[f64], eqw: &[f64], btc: Option<&[f64]>) {
    let s = metrics(strat);
    let e = metrics(eqw);
    println!("\nVeredicto:");
    let beats_eqw = s.ann_sharpe > e.ann_sharpe && s.total_return > e.total_return;
    let beats_btc = btc.map(|b| {
        let bm = metrics(b);
        s.ann_sharpe > bm.ann_sharpe
    }).unwrap_or(true);

    if s.ann_sharpe > 0.5 && beats_eqw {
        println!("  La SELECCIÓN por momentum aporta edge (Sharpe {:+.2}, bate al equal-weight).", s.ann_sharpe);
        println!("  -> Vale la pena el rediseño cross-sectional completo (con ML y más monedas).");
    } else if beats_eqw && s.ann_sharpe > 0.0 {
        println!("  Señal débil: la selección bate al equal-weight pero con Sharpe modesto ({:+.2}).", s.ann_sharpe);
        println!("  -> Quizá merezca afinarse (lookback/k/rebalanceo) antes de comprometerse.");
    } else {
        println!("  La selección por momentum NO aporta sobre tener el universo equiponderado.");
        println!("  -> El edge cross-sectional simple no está; replantear antes del build grande.");
    }
    let _ = beats_btc;
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}
