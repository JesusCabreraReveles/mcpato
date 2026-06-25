//! Sonda de carry / cash-and-carry (el experimento #3, el único que NO predice).
//! Modela una posición delta-neutral: long spot + short perpetuo del mismo
//! notional. El precio se cancela entre las dos patas; el PnL es el **funding**
//! que cobra la pata corta cada 8h (positivo cuando los longs pagan a los
//! shorts). No adivina nada — cobra una renta estructural.
//!
//! Modelo conservador: 1x (sin apalancamiento), spot como colateral del corto,
//! así que se cobra el funding sobre el capital completo. Gated por
//! MCPATO_CARRY_CHECK.

use anyhow::{bail, Result};

use crate::{config::Config, rest_binance};

pub async fn run(cfg: Config) -> Result<()> {
    let days = env_u32("MCPATO_CARRY_DAYS", 720);
    let commission = cfg.commission;

    println!(
        "\n=== CARRY (delta-neutral) · {} · {} días ===",
        cfg.symbol, days
    );
    println!(
        "Modelo: long spot + short perp (1x). Cobra funding cada 8h. Comisión {:.2}%/pata.\n",
        commission * 100.0
    );

    let end = chrono::Utc::now();
    let start = end - chrono::Duration::days(days as i64);
    let start_ms = start.timestamp_millis();
    let end_ms = end.timestamp_millis();

    let funding = rest_binance::fetch_funding_rates(&cfg.symbol, start_ms, end_ms).await?;
    if funding.len() < 10 {
        bail!("funding insuficiente: {} pagos", funding.len());
    }
    println!("Funding: {} pagos (cada 8h)\n", funding.len());

    // Curva de equity del carry: en cada pago, equity *= (1 + funding_rate).
    // (corto cobra +rate cuando es positivo, paga cuando es negativo).
    let mut equity = 1.0f64;
    let mut curve = Vec::with_capacity(funding.len() + 1);
    curve.push(equity);
    let mut rates = Vec::with_capacity(funding.len());
    for &(_, rate) in &funding {
        equity *= 1.0 + rate;
        curve.push(equity);
        rates.push(rate);
    }
    // Coste de entrada + salida: 2 patas a la entrada, 2 a la salida.
    let roundtrip_cost = 4.0 * commission;
    let equity_net = equity * (1.0 - roundtrip_cost);

    // Estadísticas del funding.
    let n = rates.len() as f64;
    let pct_pos = rates.iter().filter(|&&r| r > 0.0).count() as f64 / n * 100.0;
    let mean_rate = rates.iter().sum::<f64>() / n;
    let ann_funding = mean_rate * 3.0 * 365.0; // 3 pagos/día

    // Métricas del carry.
    let total_net = equity_net - 1.0;
    let ann_return = equity_net.powf(365.0 / days as f64) - 1.0;
    let sharpe = ann_sharpe_per_event(&rates, 3.0 * 365.0);
    let max_dd = max_drawdown(&curve);

    // Baseline: BTC buy&hold en el mismo periodo.
    let btc = btc_buy_hold(&cfg, start_ms, end_ms).await;

    println!("--- Funding ---");
    println!("Positivo el {:.0}% del tiempo · media {:.5}%/8h · ~{:+.1}%/año bruto", pct_pos, mean_rate * 100.0, ann_funding * 100.0);

    println!("\n--- Carry (neto de costes) ---");
    println!(
        "Retorno total {:+.1}%  ·  anualizado {:+.1}%  ·  Sharpe {:+.2}  ·  maxDD {:.1}%",
        total_net * 100.0,
        ann_return * 100.0,
        sharpe,
        max_dd * 100.0
    );

    if let Some((btc_ret, btc_sharpe, btc_dd)) = btc {
        println!("\n--- BTC buy&hold (referencia) ---");
        println!(
            "Retorno total {:+.1}%  ·  Sharpe {:+.2}  ·  maxDD {:.1}%",
            btc_ret * 100.0,
            btc_sharpe,
            btc_dd * 100.0
        );
    }

    verdict(ann_return, sharpe, max_dd);
    Ok(())
}

fn verdict(ann_return: f64, sharpe: f64, max_dd: f64) {
    println!("\nVeredicto:");
    if ann_return > 0.0 && sharpe > 1.0 {
        println!("  Renta estructural REAL: retorno positivo con Sharpe alto ({sharpe:+.2}) y poco");
        println!("  riesgo (maxDD {:.1}%). Es el camino más sólido a dinero real de toda la sesión.", max_dd * 100.0);
        println!("  -> Vale la pena construir el bot de carry (long spot + short perp automatizado).");
    } else if ann_return > 0.0 {
        println!("  Gana dinero ({:+.1}%/año) pero el Sharpe es modesto ({sharpe:+.2}).", ann_return * 100.0);
        println!("  -> Viable pero poco emocionante; valorar si compensa la complejidad operativa.");
    } else {
        println!("  El funding neto NO fue rentable en este periodo. Sin edge de carry aquí.");
    }
    println!("\n  Aviso honesto: un backtest de carry SUBESTIMA riesgos reales — apalancamiento");
    println!("  /liquidación de la pata corta, coste de rebalancear el hedge, y riesgo de");
    println!("  exchange. El número real en vivo será algo menor.");
}

/// Sharpe anualizado a partir de los retornos por evento (cada funding).
fn ann_sharpe_per_event(rates: &[f64], events_per_year: f64) -> f64 {
    if rates.len() < 2 {
        return 0.0;
    }
    let n = rates.len() as f64;
    let mean = rates.iter().sum::<f64>() / n;
    let var = rates.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
    let std = var.sqrt();
    if std < 1e-15 {
        0.0
    } else {
        (mean / std) * events_per_year.sqrt()
    }
}

fn max_drawdown(curve: &[f64]) -> f64 {
    let mut peak = f64::MIN;
    let mut dd = 0.0;
    for &e in curve {
        if e > peak {
            peak = e;
        }
        if peak > 0.0 {
            let d = (peak - e) / peak;
            if d > dd {
                dd = d;
            }
        }
    }
    dd
}

/// Retorno, Sharpe (diario anualizado) y maxDD de BTC buy&hold en el periodo.
async fn btc_buy_hold(cfg: &Config, start_ms: i64, end_ms: i64) -> Option<(f64, f64, f64)> {
    let candles = rest_binance::fetch_klines_range(&cfg.symbol, "1d", start_ms, end_ms)
        .await
        .ok()?;
    if candles.len() < 2 {
        return None;
    }
    let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
    let total = closes.last()? / closes.first()? - 1.0;
    let mut rets = Vec::with_capacity(closes.len());
    for w in closes.windows(2) {
        if w[0] > 0.0 {
            rets.push(w[1] / w[0] - 1.0);
        }
    }
    let sharpe = ann_sharpe_per_event(&rets, 365.0);
    let curve: Vec<f64> = {
        let first = closes[0];
        closes.iter().map(|c| c / first).collect()
    };
    Some((total, sharpe, max_drawdown(&curve)))
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}
