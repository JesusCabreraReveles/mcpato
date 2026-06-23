mod agent;
mod banner;
mod broker;
mod config;
mod db;
mod evolution;
mod indicators;
mod models;
mod nn;
mod notify;
mod runtime;
mod signals;
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

    runtime::run(cfg).await
}
