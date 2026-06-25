use uuid::Uuid;

use crate::{
    broker::PaperBroker,
    config::Config,
    indicators::compute_features,
    models::{Candle, EvalResult, Genome, TradeRecord},
    nn,
};

const INITIAL_CAPITAL: f64 = 100.0;
/// Igual que el límite del bucle en vivo (runtime.rs): suficiente para todos los
/// indicadores y evita el coste cuadrático en simulaciones largas.
const MAX_HISTORY: usize = 2000;

/// Evalúa un genoma. Si `cfg.eval_windows > 1`, lo simula sobre varias
/// sub-ventanas contiguas y promedia el resultado: reduce el sobreajuste de
/// medir sobre un único tramo (un campeón "ganador" de una ventana puede ser
/// suerte). Con `eval_windows <= 1` equivale a una sola simulación.
pub fn simulate_agent(
    genome: Genome,
    candles: &[Candle],
    cfg: &Config,
    generation_id: i64,
) -> EvalResult {
    let windows = cfg.eval_windows.max(1);
    let n = candles.len();

    // Sin partición posible (pocas velas o una sola ventana): simulación directa.
    if windows <= 1 || n < windows * 2 {
        return simulate_window(genome, candles, cfg, generation_id);
    }

    let size = n / windows;
    let mut results = Vec::with_capacity(windows);
    for w in 0..windows {
        let start = w * size;
        let end = if w == windows - 1 { n } else { start + size };
        results.push(simulate_window(
            genome.clone(),
            &candles[start..end],
            cfg,
            generation_id,
        ));
    }
    aggregate_windows(genome, results)
}

/// Simula un genoma sobre un único tramo de velas y calcula su fitness.
fn simulate_window(
    genome: Genome,
    candles: &[Candle],
    cfg: &Config,
    generation_id: i64,
) -> EvalResult {
    let agent_id = Uuid::new_v4().to_string();
    let mut broker = PaperBroker::new(INITIAL_CAPITAL);
    let mut history: Vec<Candle> = Vec::with_capacity(candles.len());
    let mut trades: Vec<TradeRecord> = Vec::new();
    let mut survived = true;
    let mut fee_counter = 0usize;

    // Acumuladores para el Sharpe: media y desviación de los retornos por vela.
    let mut prev_equity = INITIAL_CAPITAL;
    let mut ret_sum = 0.0f64;
    let mut ret_sumsq = 0.0f64;
    let mut ret_n = 0u32;

    let first_close = candles.first().map(|c| c.close).unwrap_or(0.0);
    let mut last_close = first_close;

    for (idx, candle) in candles.iter().enumerate() {
        last_close = candle.close;
        history.push(candle.clone());
        // Capa el historial igual que el bucle en vivo (runtime.rs), para que la
        // simulación sea fiel y no degenere a O(n²) en ventanas largas.
        if history.len() > MAX_HISTORY {
            let drop_n = history.len() - MAX_HISTORY;
            history.drain(0..drop_n);
        }
        let features = compute_features(&history, &broker);
        let (signal, risk) = nn::forward(&genome, &features);
        let current_alloc = if broker.equity > 1e-12 {
            (broker.position_value(candle.close) / broker.equity).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let target_alloc =
            signal_to_target_alloc(signal, risk, current_alloc, cfg.signal_threshold);

        if let Some(trade) = broker.rebalance_to_allocation(
            &agent_id,
            Some(generation_id),
            candle.ts,
            candle.close,
            target_alloc,
            cfg.commission,
            cfg.slippage,
        ) {
            trades.push(trade);
        }

        fee_counter += 1;
        if fee_counter >= cfg.survival_fee_cadence_candles {
            let (fee_trades, ok) = broker.charge_survival_fee(
                &agent_id,
                Some(generation_id),
                candle.ts,
                candle.close,
                cfg.survival_rate,
                cfg.commission,
                cfg.slippage,
            );
            trades.extend(fee_trades);
            fee_counter = 0;
            if !ok {
                survived = false;
                accumulate_return(&mut prev_equity, broker.equity, &mut ret_sum, &mut ret_sumsq, &mut ret_n);
                return eval_result(
                    agent_id, genome, broker, trades, idx + 1, candles.len(), survived,
                    first_close, last_close, ret_sum, ret_sumsq, ret_n, cfg,
                );
            }
        }

        broker.update_equity(candle.close);
        accumulate_return(&mut prev_equity, broker.equity, &mut ret_sum, &mut ret_sumsq, &mut ret_n);

        if broker.equity <= 1e-9 {
            survived = false;
            return eval_result(
                agent_id, genome, broker, trades, idx + 1, candles.len(), survived,
                first_close, last_close, ret_sum, ret_sumsq, ret_n, cfg,
            );
        }
    }

    eval_result(
        agent_id, genome, broker, trades, candles.len(), candles.len(), survived,
        first_close, last_close, ret_sum, ret_sumsq, ret_n, cfg,
    )
}

/// Acumula el log-retorno por vela del equity (para media/desviación → Sharpe).
fn accumulate_return(
    prev_equity: &mut f64,
    equity: f64,
    sum: &mut f64,
    sumsq: &mut f64,
    n: &mut u32,
) {
    if *prev_equity > 1e-12 && equity > 1e-12 {
        let r = (equity / *prev_equity).ln();
        *sum += r;
        *sumsq += r * r;
        *n += 1;
    }
    *prev_equity = equity;
}

fn signal_to_target_alloc(signal: f64, risk: f64, current_alloc: f64, threshold: f64) -> f64 {
    if signal > threshold {
        risk.clamp(0.0, 1.0)
    } else if signal < -threshold {
        0.0
    } else {
        current_alloc
    }
}

/// Construye el `EvalResult` calculando el fitness rediseñado:
/// - Si murió: fitness = penalización + un pequeño bonus por haber vivido más
///   (morir tarde es menos malo que morir pronto). Así la supervivencia es un
///   *filtro*, no un término que ahogue al resto.
/// - Si sobrevivió: combinación ponderada (todos los pesos son configurables) de
///   `alpha` (batir al buy&hold), retorno absoluto, Sharpe (consistencia) y un
///   castigo al drawdown.
#[allow(clippy::too_many_arguments)]
fn eval_result(
    agent_id: String,
    genome: Genome,
    broker: PaperBroker,
    trades: Vec<TradeRecord>,
    lived_candles: usize,
    total_candles: usize,
    survived: bool,
    first_close: f64,
    last_close: f64,
    ret_sum: f64,
    ret_sumsq: f64,
    ret_n: u32,
    cfg: &Config,
) -> EvalResult {
    let survival_ratio = (lived_candles as f64 / total_candles.max(1) as f64).clamp(0.0, 1.0);
    let sharpe = sharpe_ratio(ret_sum, ret_sumsq, ret_n);

    let fitness = if !survived {
        // Filtro: cualquier muerte queda por debajo de cualquier superviviente.
        cfg.fit_death_penalty + 0.1 * survival_ratio
    } else {
        let agent_return = broker.equity / INITIAL_CAPITAL - 1.0;
        let bench_return = if first_close > 0.0 {
            last_close / first_close - 1.0
        } else {
            0.0
        };
        let alpha = agent_return - bench_return;

        cfg.fit_w_alpha * alpha
            + cfg.fit_w_absolute * agent_return
            + cfg.fit_w_sharpe * sharpe.tanh()
            - cfg.fit_w_dd * broker.max_drawdown
    };

    EvalResult {
        agent_id,
        genome,
        fitness,
        equity_final: broker.equity,
        max_drawdown: broker.max_drawdown,
        sharpe,
        survival_ratio: if survived { 1.0 } else { survival_ratio },
        trades_count: broker.trades_count,
        lived_candles,
        trades,
    }
}

/// Sharpe por vela = media / desviación de los log-retornos. Sin anualizar; el
/// `tanh` posterior lo acota, así que solo importa como señal relativa entre
/// genomas evaluados sobre la misma ventana.
fn sharpe_ratio(sum: f64, sumsq: f64, n: u32) -> f64 {
    if n < 2 {
        return 0.0;
    }
    let nf = n as f64;
    let mean = sum / nf;
    let var = (sumsq / nf - mean * mean).max(0.0);
    let std = var.sqrt();
    if std < 1e-12 {
        0.0
    } else {
        mean / std
    }
}

/// Promedia los resultados de varias sub-ventanas en un único `EvalResult`:
/// fitness y equity como media, drawdown como el peor caso, trades concatenados.
fn aggregate_windows(genome: Genome, results: Vec<EvalResult>) -> EvalResult {
    let k = results.len().max(1) as f64;
    let fitness = results.iter().map(|r| r.fitness).sum::<f64>() / k;
    let equity_final = results.iter().map(|r| r.equity_final).sum::<f64>() / k;
    let max_drawdown = results
        .iter()
        .map(|r| r.max_drawdown)
        .fold(0.0f64, f64::max);
    let sharpe = results.iter().map(|r| r.sharpe).sum::<f64>() / k;
    let survival_ratio = results.iter().map(|r| r.survival_ratio).sum::<f64>() / k;
    let trades_count = results.iter().map(|r| r.trades_count).sum();
    let lived_candles = results.iter().map(|r| r.lived_candles).sum();
    let agent_id = results
        .first()
        .map(|r| r.agent_id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let trades = results.into_iter().flat_map(|r| r.trades).collect();

    EvalResult {
        agent_id,
        genome,
        fitness,
        equity_final,
        max_drawdown,
        sharpe,
        survival_ratio,
        trades_count,
        lived_candles,
        trades,
    }
}
