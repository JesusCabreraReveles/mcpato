use std::sync::Arc;

use anyhow::Result;
use rand::{rngs::StdRng, SeedableRng};
use tokio::sync::Mutex;

use crate::{
    broker::PaperBroker,
    config::Config,
    db::Database,
    evolution,
    indicators::compute_features,
    models::{Candle, Genome, OrganismState},
    nn,
    notify::Notifier,
    signals::{self, Stance},
    ws_binance,
};

struct RuntimeState {
    broker: PaperBroker,
    historical: Vec<Candle>,
    day_buffer: Vec<Candle>,
    champion: Genome,
    population: Vec<Genome>,
    candles_since_fee: usize,
    generation_counter: usize,
    stance: Stance,
    rng: StdRng,
}

pub async fn run(cfg: Config) -> Result<()> {
    let db = Database::connect(&cfg.db_path).await?;
    db.init().await?;

    let _run_id = db.insert_run(cfg.seed, cfg.initial_capital).await?;

    let mut rng = StdRng::seed_from_u64(cfg.seed as u64);

    let persisted_state = db.load_organism_state().await?;
    let broker = if let Some(state) = persisted_state {
        PaperBroker {
            cash: state.cash,
            position_qty: state.position_qty,
            entry_price: state.entry_price,
            equity: state.equity,
            peak_equity: state.equity.max(state.initial_capital),
            max_drawdown: 0.0,
            trades_count: 0,
        }
    } else {
        PaperBroker::new(cfg.initial_capital)
    };

    let historical = db.load_recent_candles(600).await?;
    let day_buffer = Vec::with_capacity(cfg.candles_per_day);

    let (default_champion, mut default_population) =
        evolution::bootstrap_population(&mut rng, &cfg);
    let champion = db
        .load_best_genome_from_latest_generation()
        .await?
        .unwrap_or(default_champion);
    default_population[0] = champion.clone();

    let generation_counter = db.load_generation_count().await? as usize;

    // Postura inicial derivada de la posición persistida (evita emitir una señal
    // espuria en la primera vela tras un reinicio).
    let stance = signals::stance_from_position(broker.position_qty, cfg.position_epsilon);

    let state = Arc::new(Mutex::new(RuntimeState {
        broker,
        historical,
        day_buffer,
        champion,
        population: default_population,
        candles_since_fee: 0,
        generation_counter,
        stance,
        rng,
    }));

    let notifier = Notifier::from_config(&cfg);
    if notifier.is_enabled() {
        println!("Notificaciones Telegram: ACTIVADAS");
    } else {
        println!("Notificaciones Telegram: desactivadas (sin token/chat_id o MCPATO_NOTIFY_ENABLED=false)");
    }

    // Dashboard web en tarea aparte. Si no puede arrancar, se loguea pero el
    // trading continúa.
    if cfg.http_enabled {
        let web_cfg = cfg.clone();
        let web_db = db.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::web::serve(web_cfg, web_db).await {
                eprintln!("warn: servidor web detenido: {e:#}");
            }
        });
    } else {
        println!("Dashboard web: desactivado (MCPATO_HTTP_ENABLED=false)");
    }

    ws_binance::run_kline_stream(&cfg.symbol, &cfg.interval, {
        let db = db.clone();
        let cfg = cfg.clone();
        let state = Arc::clone(&state);
        let notifier = notifier.clone();

        move |candle| {
            let db = db.clone();
            let cfg = cfg.clone();
            let state = Arc::clone(&state);
            let notifier = notifier.clone();

            async move {
                let mut rt = state.lock().await;

                db.insert_candle(&candle).await?;
                rt.historical.push(candle.clone());
                if rt.historical.len() > 2000 {
                    let drop_n = rt.historical.len() - 2000;
                    rt.historical.drain(0..drop_n);
                }

                let features = compute_features(&rt.historical, &rt.broker);
                let (signal, risk) = nn::forward(&rt.champion, &features);

                let current_alloc = if rt.broker.equity > 1e-12 {
                    (rt.broker.position_value(candle.close) / rt.broker.equity).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let target_alloc = if signal > cfg.signal_threshold {
                    risk
                } else if signal < -cfg.signal_threshold {
                    0.0
                } else {
                    current_alloc
                };

                if let Some(trade) = rt.broker.rebalance_to_allocation(
                    "CHAMPION",
                    None,
                    candle.ts,
                    candle.close,
                    target_alloc,
                    cfg.commission,
                    cfg.slippage,
                ) {
                    db.insert_trade(&trade).await?;
                }

                // Señal discreta: solo si el campeón cambia de postura.
                let next_stance =
                    signals::desired_stance(rt.stance, signal, cfg.signal_threshold);
                if let Some(action) = signals::transition_action(rt.stance, next_stance) {
                    let expires_at = candle.ts
                        + chrono::Duration::seconds(cfg.signal_ttl_secs.max(0));
                    db.insert_signal(
                        &cfg.instrument_name,
                        action,
                        candle.close,
                        candle.ts,
                        expires_at,
                    )
                    .await?;
                    println!(
                        "SIGNAL {} {} @ {:.2} (expira {})",
                        action,
                        cfg.instrument_name,
                        candle.close,
                        expires_at.to_rfc3339()
                    );
                }
                rt.stance = next_stance;

                // Vence las señales pendientes cuyo TTL ya pasó.
                db.expire_stale_signals(chrono::Utc::now()).await?;

                // Notifica las señales pendientes aún no enviadas (reintenta si
                // Telegram estuvo caído). Nunca propaga error al daemon.
                notify_pending_signals(&db, &notifier).await;

                rt.candles_since_fee += 1;
                if rt.candles_since_fee >= cfg.survival_fee_cadence_candles {
                    let (fee_trades, ok) = rt.broker.charge_survival_fee(
                        "CHAMPION",
                        None,
                        candle.ts,
                        candle.close,
                        cfg.survival_rate,
                        cfg.commission,
                        cfg.slippage,
                    );
                    for t in fee_trades {
                        db.insert_trade(&t).await?;
                    }
                    rt.candles_since_fee = 0;

                    if !ok {
                        let final_state = OrganismState {
                            cash: rt.broker.cash,
                            position_qty: rt.broker.position_qty,
                            entry_price: rt.broker.entry_price,
                            equity: rt.broker.equity,
                            initial_capital: cfg.initial_capital,
                            delta_vs_initial: rt.broker.equity - cfg.initial_capital,
                            alive: false,
                        };
                        db.upsert_organism_state(&final_state).await?;
                        return Err(anyhow::anyhow!("organism died: could not pay survival fee"));
                    }
                }

                rt.broker.update_equity(candle.close);
                let state_row = OrganismState {
                    cash: rt.broker.cash,
                    position_qty: rt.broker.position_qty,
                    entry_price: rt.broker.entry_price,
                    equity: rt.broker.equity,
                    initial_capital: cfg.initial_capital,
                    delta_vs_initial: rt.broker.equity - cfg.initial_capital,
                    alive: true,
                };
                db.upsert_organism_state(&state_row).await?;

                rt.day_buffer.push(candle);

                if rt.day_buffer.len() >= cfg.candles_per_day {
                    let day_start = rt.day_buffer.first().map(|c| c.ts).unwrap_or_else(chrono::Utc::now);
                    let day_end = rt.day_buffer.last().map(|c| c.ts).unwrap_or_else(chrono::Utc::now);
                    let day_snapshot = rt.day_buffer.clone();
                    let pop_snapshot = rt.population.clone();

                    let pseudo_generation_id = rt.generation_counter as i64 + 1;
                    let outcome = evolution::evaluate_generation(
                        &mut rt.rng,
                        &day_snapshot,
                        &pop_snapshot,
                        &cfg,
                        pseudo_generation_id,
                    );

                    let generation_id = db
                        .insert_generation(
                            day_start,
                            day_end,
                            outcome.avg_fitness,
                            outcome.best_fitness,
                            outcome.survival_rate,
                            outcome.extinction_happened,
                            rt.broker.equity,
                            rt.broker.equity - cfg.initial_capital,
                            cfg.seed,
                        )
                        .await?;

                    for mut result in outcome.results {
                        db.insert_agent_result(generation_id, &result).await?;
                        for trade in &mut result.trades {
                            trade.generation_id = Some(generation_id);
                            db.insert_trade(trade).await?;
                        }
                    }

                    rt.champion = outcome.next_champion;
                    rt.population = outcome.next_population;
                    rt.generation_counter += 1;

                    println!(
                        "GEN {} | {} -> {} | champion_equity={:.4} delta={:.4} | best_fit={:.6} avg_fit={:.6} survival_rate={:.2}%",
                        rt.generation_counter,
                        day_start.to_rfc3339(),
                        day_end.to_rfc3339(),
                        rt.broker.equity,
                        rt.broker.equity - cfg.initial_capital,
                        outcome.best_fitness,
                        outcome.avg_fitness,
                        outcome.survival_rate * 100.0
                    );

                    rt.day_buffer.clear();

                    if let Some(max_gens) = cfg.max_generations {
                        if rt.generation_counter >= max_gens {
                            println!(
                                "Reached MCPATO_MAX_GENERATIONS={} with champion equity {:.4}",
                                max_gens, rt.broker.equity
                            );
                            std::process::exit(0);
                        }
                    }
                }

                Ok(())
            }
        }
    })
    .await
}

/// Envía por Telegram las señales pendientes no notificadas. Los errores de red
/// se loguean pero no se propagan: una notificación nunca debe tumbar el daemon.
async fn notify_pending_signals(db: &Database, notifier: &Notifier) {
    if !notifier.is_enabled() {
        return;
    }
    let pending = match db.pending_unnotified_signals(20).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warn: no se pudieron leer señales pendientes: {e:#}");
            return;
        }
    };
    for signal in pending {
        match notifier.send_signal(&signal).await {
            Ok(()) => {
                if let Err(e) = db.mark_signal_notified(signal.id).await {
                    eprintln!("warn: no se pudo marcar señal {} como notificada: {e:#}", signal.id);
                }
            }
            Err(e) => {
                eprintln!("warn: fallo notificando señal {} (se reintentará): {e:#}", signal.id);
            }
        }
    }
}
