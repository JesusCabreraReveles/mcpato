use chrono::{DateTime, Utc};

use crate::models::TradeRecord;

#[derive(Debug, Clone)]
pub struct PaperBroker {
    pub cash: f64,
    pub position_qty: f64,
    pub entry_price: f64,
    pub equity: f64,
    pub peak_equity: f64,
    pub max_drawdown: f64,
    pub trades_count: i64,
}

impl PaperBroker {
    pub fn new(initial_capital: f64) -> Self {
        Self {
            cash: initial_capital,
            position_qty: 0.0,
            entry_price: 0.0,
            equity: initial_capital,
            peak_equity: initial_capital,
            max_drawdown: 0.0,
            trades_count: 0,
        }
    }

    pub fn unrealized_pnl(&self, price: f64) -> f64 {
        (price - self.entry_price) * self.position_qty
    }

    pub fn update_equity(&mut self, price: f64) {
        self.equity = self.cash + self.position_qty * price;
        if self.equity > self.peak_equity {
            self.peak_equity = self.equity;
        }
        if self.peak_equity > 0.0 {
            let dd = ((self.peak_equity - self.equity) / self.peak_equity).clamp(0.0, 1.0);
            if dd > self.max_drawdown {
                self.max_drawdown = dd;
            }
        }
    }

    pub fn rebalance_to_allocation(
        &mut self,
        agent_id: &str,
        generation_id: Option<i64>,
        ts: DateTime<Utc>,
        price: f64,
        target_allocation: f64,
        commission: f64,
        slippage: f64,
    ) -> Option<TradeRecord> {
        self.update_equity(price);

        let safe_equity = self.equity.max(0.0);
        let current_position_value = self.position_qty * price;
        let target_position_value = safe_equity * target_allocation.clamp(0.0, 1.0);
        let delta_value = target_position_value - current_position_value;

        if delta_value.abs() < 1e-8 {
            return None;
        }

        if delta_value > 0.0 {
            self.buy_value(
                agent_id,
                generation_id,
                ts,
                price,
                delta_value,
                commission,
                slippage,
            )
        } else {
            self.sell_value(
                agent_id,
                generation_id,
                ts,
                price,
                -delta_value,
                commission,
                slippage,
            )
        }
    }

    fn buy_value(
        &mut self,
        agent_id: &str,
        generation_id: Option<i64>,
        ts: DateTime<Utc>,
        price: f64,
        requested_value: f64,
        commission: f64,
        slippage: f64,
    ) -> Option<TradeRecord> {
        let exec_price = price * (1.0 + slippage.max(0.0));
        let max_affordable_value = self.cash / (1.0 + commission.max(0.0));
        let trade_value = requested_value.min(max_affordable_value).max(0.0);

        if trade_value <= 1e-8 {
            return None;
        }

        let qty = trade_value / exec_price;
        let fee = trade_value * commission;
        self.cash -= trade_value + fee;

        let prev_value = self.position_qty * self.entry_price;
        self.position_qty += qty;
        let new_value = prev_value + qty * exec_price;
        if self.position_qty > 1e-12 {
            self.entry_price = new_value / self.position_qty;
        }

        self.trades_count += 1;
        self.update_equity(price);

        Some(TradeRecord {
            agent_id: agent_id.to_string(),
            generation_id,
            timestamp: ts,
            side: "BUY".to_string(),
            price: exec_price,
            quantity: qty,
            value: trade_value,
            commission: fee,
        })
    }

    fn sell_value(
        &mut self,
        agent_id: &str,
        generation_id: Option<i64>,
        ts: DateTime<Utc>,
        price: f64,
        requested_value: f64,
        commission: f64,
        slippage: f64,
    ) -> Option<TradeRecord> {
        let exec_price = price * (1.0 - slippage.max(0.0));
        let max_value = self.position_qty * exec_price;
        let trade_value = requested_value.min(max_value).max(0.0);

        if trade_value <= 1e-8 || exec_price <= 0.0 {
            return None;
        }

        let qty = (trade_value / exec_price).min(self.position_qty);
        let gross = qty * exec_price;
        let fee = gross * commission;

        self.position_qty -= qty;
        if self.position_qty <= 1e-12 {
            self.position_qty = 0.0;
            self.entry_price = 0.0;
        }
        self.cash += gross - fee;

        self.trades_count += 1;
        self.update_equity(price);

        Some(TradeRecord {
            agent_id: agent_id.to_string(),
            generation_id,
            timestamp: ts,
            side: "SELL".to_string(),
            price: exec_price,
            quantity: qty,
            value: gross,
            commission: fee,
        })
    }

    pub fn charge_survival_fee(
        &mut self,
        agent_id: &str,
        generation_id: Option<i64>,
        ts: DateTime<Utc>,
        price: f64,
        survival_rate: f64,
        commission: f64,
        slippage: f64,
    ) -> (Vec<TradeRecord>, bool) {
        self.update_equity(price);
        let fee_due = self.equity.max(0.0) * survival_rate.max(0.0);
        let mut trades = Vec::new();

        if self.cash < fee_due {
            let needed = fee_due - self.cash;
            if let Some(trade) = self.sell_value(
                agent_id,
                generation_id,
                ts,
                price,
                needed,
                commission,
                slippage,
            ) {
                trades.push(trade);
            }
        }

        if self.cash >= fee_due {
            self.cash -= fee_due;
            self.update_equity(price);
            (trades, true)
        } else {
            self.update_equity(price);
            (trades, false)
        }
    }

    pub fn position_value(&self, price: f64) -> f64 {
        self.position_qty * price
    }
}
