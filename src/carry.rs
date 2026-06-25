//! Sonda de carry / cash-and-carry (el experimento #3, el único que NO predice).
//! Modela una posición delta-neutral: long spot + short perpetuo del mismo
//! notional. El precio se cancela entre las dos patas; el PnL es el **funding**
//! que cobra la pata corta cada 8h (positivo cuando los longs pagan a los
//! shorts). No adivina nada — cobra una renta estructural.
//!
//! Modelo conservador: 1x (sin apalancamiento), spot como colateral del corto,
//! así que se cobra el funding sobre el capital completo. Gated por
//! MCPATO_CARRY_CHECK.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Result};
use tokio::time::sleep;

use crate::{config::Config, db::Database, models::CarryState, notify::Notifier, rest_binance};

const DEFAULT_UNIVERSE: &str = "BTCUSDT,ETHUSDT,BNBUSDT,XRPUSDT,ADAUSDT,SOLUSDT,\
DOGEUSDT,LTCUSDT,LINKUSDT,DOTUSDT,AVAXUSDT,TRXUSDT,ATOMUSDT,ETCUSDT,BCHUSDT,XLMUSDT";

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

/// Bot de carry de PRODUCCIÓN (paper, paso 1). Mantiene una posición
/// delta-neutral (long spot + short perp, 1x por defecto) y acumula el funding
/// real de cada liquidación (cada 8h) contra datos en vivo de Binance. El estado
/// se persiste, así que sobrevive a reinicios. Robusto a cortes de red: el
/// funding se lee por REST y se reintenta en el siguiente ciclo.
///
/// Pendiente (pasos 2-3): control de riesgo (salir si el funding se vuelve
/// persistentemente negativo), rebalanceo del hedge, y dashboard.
pub async fn run_bot(cfg: Config) -> Result<()> {
    let db = Database::connect(&cfg.db_path).await?;
    db.init().await?;

    let leverage = env_f64("MCPATO_CARRY_LEVERAGE", 1.0).clamp(0.1, 10.0);
    let poll_secs = env_u32("MCPATO_CARRY_POLL_SECS", 300).max(10) as u64;
    let commission = cfg.commission;
    let notifier = Notifier::from_config(&cfg);

    let mut state = match db.load_carry_state().await? {
        Some(s) => {
            println!(
                "Carry: estado recuperado · equity {:.4} · funding acumulado {:+.4} · {} pagos",
                s.equity, s.accumulated_funding, s.payments
            );
            s
        }
        None => {
            // Abre la posición delta-neutral: coste de entrada de 2 patas.
            let equity = cfg.initial_capital * (1.0 - 2.0 * commission);
            let s = CarryState {
                equity,
                initial_capital: cfg.initial_capital,
                accumulated_funding: 0.0,
                last_settled_ms: chrono::Utc::now().timestamp_millis(),
                position_open: true,
                payments: 0,
            };
            db.upsert_carry_state(&s).await?;
            println!(
                "Carry: posición delta-neutral ABIERTA · capital {:.2} · leverage {}x · símbolo {}",
                equity, leverage, cfg.symbol
            );
            let _ = notifier
                .send_text(&format!(
                    "🟢 Carry abierto\nSímbolo: {}\nCapital: {:.2}\nLeverage: {}x (delta-neutral)",
                    cfg.symbol, equity, leverage
                ))
                .await;
            s
        }
    };

    println!(
        "Carry bot en marcha (poll cada {}s). Notificaciones Telegram: {}.",
        poll_secs,
        if notifier.is_enabled() { "ON" } else { "off" }
    );

    loop {
        let now_ms = chrono::Utc::now().timestamp_millis();
        match rest_binance::fetch_funding_rates(&cfg.symbol, state.last_settled_ms + 1, now_ms).await
        {
            Ok(funding) => {
                let mut new_payments = 0;
                for (t, rate) in funding {
                    if t <= state.last_settled_ms {
                        continue;
                    }
                    // Cobro de la pata corta: +rate * notional (notional = equity*leverage).
                    let notional = state.equity * leverage;
                    let payment = rate * notional;
                    state.equity += payment;
                    state.accumulated_funding += payment;
                    state.last_settled_ms = t;
                    state.payments += 1;
                    new_payments += 1;
                    println!(
                        "Carry funding · rate {:+.5}% · pago {:+.4} · equity {:.4} (Δ {:+.4})",
                        rate * 100.0,
                        payment,
                        state.equity,
                        state.equity - state.initial_capital
                    );
                }
                if new_payments > 0 {
                    db.upsert_carry_state(&state).await?;
                }
            }
            Err(e) => eprintln!("warn: lectura de funding falló (se reintenta): {e:#}"),
        }

        sleep(Duration::from_secs(poll_secs)).await;
    }
}

/// Carry multi-moneda: cobra funding en una cesta de perpetuos. Compara la cesta
/// equiponderada y un top-k por funding reciente contra el carry de solo BTC.
pub async fn run_multi(cfg: Config) -> Result<()> {
    let days = env_u32("MCPATO_CARRY_DAYS", 720);
    let universe: Vec<String> = std::env::var("MCPATO_CARRY_SYMBOLS")
        .unwrap_or_else(|_| DEFAULT_UNIVERSE.to_string())
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    let top_k = env_usize("MCPATO_CARRY_TOPK", 5);
    let lookback = env_usize("MCPATO_CARRY_LOOKBACK", 3); // eventos (3 = 1 día)
    let rebalance = env_usize("MCPATO_CARRY_REBALANCE", 3).max(1);
    let commission = cfg.commission;

    println!("\n=== CARRY MULTI-MONEDA · {} perps · {} días ===", universe.len(), days);
    println!(
        "Cesta delta-neutral cobrando funding. Top-{} por funding de las últimas {} ventanas, rebalanceo cada {} eventos.\n",
        top_k, lookback, rebalance
    );

    let end = chrono::Utc::now();
    let start = end - chrono::Duration::days(days as i64);
    let (start_ms, end_ms) = (start.timestamp_millis(), end.timestamp_millis());

    // Descarga funding de cada símbolo -> mapa fundingTime->rate.
    let mut maps: Vec<(String, HashMap<i64, f64>)> = Vec::new();
    let mut grid: Vec<i64> = Vec::new();
    for sym in &universe {
        match rest_binance::fetch_funding_rates(sym, start_ms, end_ms).await {
            Ok(f) if f.len() >= 10 => {
                for (t, _) in &f {
                    grid.push(*t);
                }
                maps.push((sym.clone(), f.into_iter().collect()));
            }
            _ => eprintln!("  {sym:<10} sin funding, se omite"),
        }
    }
    if maps.len() < top_k + 1 {
        bail!("perps insuficientes con funding: {}", maps.len());
    }
    grid.sort_unstable();
    grid.dedup();
    let n = grid.len();

    // Tasa alineada por símbolo (None si esa moneda no tenía funding en ese evento).
    let aligned: Vec<Vec<Option<f64>>> = maps
        .iter()
        .map(|(_, m)| grid.iter().map(|t| m.get(t).copied()).collect())
        .collect();

    // Estrategia A: cesta equiponderada de TODAS las disponibles cada evento.
    let mut eq_all = 1.0;
    let mut curve_all = vec![1.0];
    let mut rets_all = Vec::with_capacity(n);
    for i in 0..n {
        let rates: Vec<f64> = aligned.iter().filter_map(|c| c[i]).collect();
        let r = if rates.is_empty() { 0.0 } else { rates.iter().sum::<f64>() / rates.len() as f64 };
        eq_all *= 1.0 + r;
        curve_all.push(eq_all);
        rets_all.push(r);
    }
    eq_all *= 1.0 - 4.0 * commission; // entrada+salida, 2 patas

    // Estrategia B: top-k por funding medio reciente, rebalanceado.
    let mut eq_k = 1.0;
    let mut curve_k = vec![1.0];
    let mut rets_k = Vec::with_capacity(n);
    let mut selected: Vec<usize> = Vec::new();
    for i in 0..n {
        if i >= lookback && (i - lookback) % rebalance == 0 {
            let mut scored: Vec<(usize, f64)> = Vec::new();
            for (s, col) in aligned.iter().enumerate() {
                let window: Vec<f64> = col[i - lookback..i].iter().filter_map(|x| *x).collect();
                if !window.is_empty() {
                    scored.push((s, window.iter().sum::<f64>() / window.len() as f64));
                }
            }
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            let new_sel: Vec<usize> = scored.into_iter().take(top_k).map(|(s, _)| s).collect();
            // Coste de rotar la cesta (fracción cambiada, 2 patas ida/vuelta).
            if !selected.is_empty() {
                let changed = new_sel.iter().filter(|s| !selected.contains(s)).count();
                let frac = changed as f64 / top_k as f64;
                eq_k *= 1.0 - frac * 4.0 * commission;
            }
            selected = new_sel;
        }
        let rates: Vec<f64> = selected.iter().filter_map(|&s| aligned[s][i]).collect();
        let r = if rates.is_empty() { 0.0 } else { rates.iter().sum::<f64>() / rates.len() as f64 };
        eq_k *= 1.0 + r;
        curve_k.push(eq_k);
        rets_k.push(r);
    }
    eq_k *= 1.0 - 4.0 * commission;

    // BTC-only (referencia) desde su propia serie.
    let btc_idx = maps.iter().position(|(s, _)| s == "BTCUSDT");
    let (btc_ret, btc_sharpe, btc_dd) = if let Some(bi) = btc_idx {
        let rates: Vec<f64> = aligned[bi].iter().filter_map(|x| *x).collect();
        let mut e = 1.0;
        let mut curve = vec![1.0];
        for r in &rates {
            e *= 1.0 + r;
            curve.push(e);
        }
        e *= 1.0 - 4.0 * commission;
        (e - 1.0, ann_sharpe_per_event(&rates, 3.0 * 365.0), max_drawdown(&curve))
    } else {
        (0.0, 0.0, 0.0)
    };

    let ann = |eq: f64| eq.powf(365.0 / days as f64) - 1.0;
    println!("{:<26} retorno {:+6.1}%  anual {:+5.1}%  Sharpe {:+7.2}  maxDD {:.2}%",
        "Cesta equiponderada", (eq_all - 1.0) * 100.0, ann(eq_all) * 100.0,
        ann_sharpe_per_event(&rets_all, 3.0 * 365.0), max_drawdown(&curve_all) * 100.0);
    println!("{:<26} retorno {:+6.1}%  anual {:+5.1}%  Sharpe {:+7.2}  maxDD {:.2}%",
        format!("Top-{top_k} por funding"), (eq_k - 1.0) * 100.0, ann(eq_k) * 100.0,
        ann_sharpe_per_event(&rets_k, 3.0 * 365.0), max_drawdown(&curve_k) * 100.0);
    println!("{:<26} retorno {:+6.1}%  anual {:+5.1}%  Sharpe {:+7.2}  maxDD {:.2}%",
        "Solo BTC", btc_ret * 100.0, ann(btc_ret + 1.0) * 100.0, btc_sharpe, btc_dd * 100.0);

    println!("\nVeredicto:");
    let best_basket = ann(eq_all).max(ann(eq_k));
    if best_basket > ann(btc_ret + 1.0) * 1.1 {
        println!("  Diversificar la cesta SUBE el yield frente a solo BTC ({:+.1}% vs {:+.1}%/año).",
            best_basket * 100.0, ann(btc_ret + 1.0) * 100.0);
        println!("  -> El carry multi-moneda es la base del bot de producción.");
    } else {
        println!("  La cesta no mejora claramente sobre solo BTC; BTC carry ya captura casi todo.");
    }
    println!("\n  (Mismo aviso: el backtest subestima basis risk, rebalanceo y riesgo de exchange.)");
    Ok(())
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

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}
