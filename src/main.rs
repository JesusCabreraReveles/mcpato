mod agent;
mod banner;
mod broker;
mod carry;
mod config;
mod crossmomentum;
mod db;
mod evolution;
mod indicators;
mod models;
mod nn;
mod notify;
mod oos;
mod rest_binance;
mod runtime;
mod signals;
mod walkforward;
mod web;
mod ws_binance;

use anyhow::Result;
use config::Config;

#[tokio::main]
async fn main() {
    if let Err(e) = real_main().await {
        eprintln!("fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn real_main() -> Result<()> {
    let cfg = Config::from_env()?;

    banner::print_startup(&cfg);

    // Modo experimento: medición out-of-sample y salida (no arranca el daemon).
    if std::env::var("MCPATO_OOS_CHECK").map(|v| v == "true" || v == "1").unwrap_or(false) {
        return oos::run(cfg).await;
    }
    // Modo experimento: harness walk-forward multi-régimen y salida.
    if std::env::var("MCPATO_WF_CHECK").map(|v| v == "true" || v == "1").unwrap_or(false) {
        return walkforward::run(cfg).await;
    }
    // Modo experimento: sonda cross-sectional momentum (multi-moneda) y salida.
    if std::env::var("MCPATO_XS_CHECK").map(|v| v == "true" || v == "1").unwrap_or(false) {
        return crossmomentum::run(cfg).await;
    }
    // Modo experimento: sonda de carry (delta-neutral, cobra funding) y salida.
    if std::env::var("MCPATO_CARRY_CHECK").map(|v| v == "true" || v == "1").unwrap_or(false) {
        return carry::run(cfg).await;
    }
    // Modo experimento: carry multi-moneda (cesta de perps) y salida.
    if std::env::var("MCPATO_CARRY_MULTI_CHECK").map(|v| v == "true" || v == "1").unwrap_or(false) {
        return carry::run_multi(cfg).await;
    }

    runtime::run(cfg).await
}
