use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Candle {
    pub ts: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Clone)]
pub struct OrganismState {
    pub cash: f64,
    pub position_qty: f64,
    pub entry_price: f64,
    pub equity: f64,
    pub initial_capital: f64,
    pub delta_vs_initial: f64,
    pub alive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Genome {
    pub weights: Vec<f64>,
}

/// Señal de acción (compra/venta) con ciclo de vida orientado al humano:
/// `PENDING` -> `EXECUTED` (la marcaste a tiempo) o `EXPIRED` (venció el TTL).
/// Los campos los lee/serializa el dashboard web (Fase 3).
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub instrument: String,
    pub action: String,
    pub price: f64,
    pub expires_at: DateTime<Utc>,
    pub status: String,
    pub notified: bool,
}

#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub agent_id: String,
    pub generation_id: Option<i64>,
    pub timestamp: DateTime<Utc>,
    pub side: String,
    pub price: f64,
    pub quantity: f64,
    pub value: f64,
    pub commission: f64,
}

#[derive(Debug, Clone)]
pub struct EvalResult {
    pub agent_id: String,
    pub genome: Genome,
    pub fitness: f64,
    pub equity_final: f64,
    pub max_drawdown: f64,
    pub sharpe: f64,
    pub survival_ratio: f64,
    pub trades_count: i64,
    pub lived_candles: usize,
    pub trades: Vec<TradeRecord>,
}
