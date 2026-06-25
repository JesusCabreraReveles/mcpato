use crate::{broker::PaperBroker, models::Candle, nn::INPUT_SIZE};

pub fn compute_features(history: &[Candle], broker: &PaperBroker) -> [f64; INPUT_SIZE] {
    let mut f = [0.0f64; INPUT_SIZE];
    if history.is_empty() {
        return f;
    }

    let closes: Vec<f64> = history.iter().map(|c| c.close).collect();
    let last_close = *closes.last().unwrap_or(&0.0);

    f[0] = clamp(ret_n(&closes, 1), -3.0, 3.0);
    f[1] = clamp(ret_n(&closes, 5), -3.0, 3.0);
    f[2] = clamp(ret_n(&closes, 20), -3.0, 3.0);

    let ema_fast = ema(&closes, 9);
    let ema_slow = ema(&closes, 21);
    if last_close > 0.0 {
        f[3] = clamp((last_close - ema_fast) / last_close, -3.0, 3.0);
        f[4] = clamp((last_close - ema_slow) / last_close, -3.0, 3.0);
        f[5] = clamp((ema_fast - ema_slow) / last_close, -3.0, 3.0);
    }

    f[6] = clamp((rsi(&closes, 14) - 50.0) / 50.0, -3.0, 3.0);
    f[7] = clamp(volatility(&closes, 20), -3.0, 3.0);

    if last_close > 0.0 {
        f[8] = clamp(atr(history, 14) / last_close, -3.0, 3.0);
    }

    // Tendencia de horizonte largo: momentum a 50/100/200 velas y distancia a una
    // EMA larga. Le dan al modelo contexto de la tendencia GRANDE (de qué carecía:
    // por eso se perdía los mercados alcistas). Devuelven 0 si no hay histórico
    // suficiente, así que degradan de forma neutral.
    f[9] = clamp(ret_n(&closes, 50), -3.0, 3.0);
    f[10] = clamp(ret_n(&closes, 100), -3.0, 3.0);
    f[11] = clamp(ret_n(&closes, 200), -3.0, 3.0);
    let ema_long = ema(&closes, 100);
    if last_close > 0.0 {
        f[12] = clamp((last_close - ema_long) / last_close, -3.0, 3.0);
    }

    let equity = broker.equity.max(1e-12);
    let pos_val = broker.position_value(last_close);
    let unrealized = broker.unrealized_pnl(last_close);

    f[13] = clamp(pos_val / equity, -3.0, 3.0);
    f[14] = clamp(unrealized / equity, -3.0, 3.0);
    f[15] = clamp(broker.max_drawdown, -3.0, 3.0);

    f
}

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.clamp(lo, hi)
}

fn ret_n(closes: &[f64], n: usize) -> f64 {
    if closes.len() <= n {
        return 0.0;
    }
    let cur = closes[closes.len() - 1];
    let prev = closes[closes.len() - 1 - n];
    if cur > 0.0 && prev > 0.0 {
        (cur / prev).ln()
    } else {
        0.0
    }
}

fn ema(values: &[f64], period: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let alpha = 2.0 / (period as f64 + 1.0);
    let mut out = values[0];
    for v in values.iter().skip(1) {
        out = alpha * *v + (1.0 - alpha) * out;
    }
    out
}

fn rsi(closes: &[f64], period: usize) -> f64 {
    if closes.len() <= period {
        return 50.0;
    }
    let mut gain = 0.0;
    let mut loss = 0.0;
    for i in closes.len() - period..closes.len() {
        if i == 0 {
            continue;
        }
        let diff = closes[i] - closes[i - 1];
        if diff >= 0.0 {
            gain += diff;
        } else {
            loss += -diff;
        }
    }
    if loss == 0.0 {
        return 100.0;
    }
    let rs = (gain / period as f64) / (loss / period as f64);
    100.0 - (100.0 / (1.0 + rs))
}

fn volatility(closes: &[f64], period: usize) -> f64 {
    if closes.len() <= period {
        return 0.0;
    }
    let mut rets = Vec::with_capacity(period);
    for i in closes.len() - period..closes.len() {
        if i == 0 {
            continue;
        }
        let prev = closes[i - 1];
        let cur = closes[i];
        if prev > 0.0 && cur > 0.0 {
            rets.push((cur / prev).ln());
        }
    }
    if rets.is_empty() {
        return 0.0;
    }
    let mean = rets.iter().sum::<f64>() / rets.len() as f64;
    let var = rets
        .iter()
        .map(|r| {
            let d = r - mean;
            d * d
        })
        .sum::<f64>()
        / rets.len() as f64;
    var.sqrt()
}

fn atr(history: &[Candle], period: usize) -> f64 {
    if history.len() < 2 {
        return 0.0;
    }
    let start = history.len().saturating_sub(period);
    let mut trs = Vec::with_capacity(period);
    for i in start..history.len() {
        let c = &history[i];
        let prev_close = if i == 0 {
            c.close
        } else {
            history[i - 1].close
        };
        let tr = (c.high - c.low)
            .max((c.high - prev_close).abs())
            .max((c.low - prev_close).abs());
        trs.push(tr);
    }
    if trs.is_empty() {
        return 0.0;
    }
    trs.iter().sum::<f64>() / trs.len() as f64
}
