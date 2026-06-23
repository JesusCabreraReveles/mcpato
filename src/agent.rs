use uuid::Uuid;

use crate::{
    broker::PaperBroker,
    config::Config,
    indicators::compute_features,
    models::{Candle, EvalResult, Genome, TradeRecord},
    nn,
};

pub fn simulate_agent(
    genome: Genome,
    candles: &[Candle],
    cfg: &Config,
    generation_id: i64,
) -> EvalResult {
    let agent_id = Uuid::new_v4().to_string();
    let mut broker = PaperBroker::new(100.0);
    let mut history: Vec<Candle> = Vec::with_capacity(candles.len());
    let mut trades: Vec<TradeRecord> = Vec::new();
    let mut survived = true;
    let mut fee_counter = 0usize;

    for (idx, candle) in candles.iter().enumerate() {
        history.push(candle.clone());
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
                return eval_result(
                    agent_id,
                    genome,
                    broker,
                    trades,
                    idx + 1,
                    candles.len(),
                    survived,
                );
            }
        }

        broker.update_equity(candle.close);
        if broker.equity <= 1e-9 {
            survived = false;
            return eval_result(
                agent_id,
                genome,
                broker,
                trades,
                idx + 1,
                candles.len(),
                survived,
            );
        }
    }

    eval_result(
        agent_id,
        genome,
        broker,
        trades,
        candles.len(),
        candles.len(),
        survived,
    )
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

fn eval_result(
    agent_id: String,
    genome: Genome,
    broker: PaperBroker,
    trades: Vec<TradeRecord>,
    lived_candles: usize,
    total_candles: usize,
    survived: bool,
) -> EvalResult {
    let survival_ratio = (lived_candles as f64 / total_candles as f64).clamp(0.0, 1.0);
    let equity_growth = (broker.equity / 100.0).max(1e-9);
    let fitness = 0.5 * survival_ratio + 0.4 * equity_growth.ln() - 0.1 * broker.max_drawdown;

    EvalResult {
        agent_id,
        genome,
        fitness,
        equity_final: broker.equity,
        max_drawdown: broker.max_drawdown,
        survival_ratio: if survived { 1.0 } else { survival_ratio },
        trades_count: broker.trades_count,
        lived_candles,
        trades,
    }
}
