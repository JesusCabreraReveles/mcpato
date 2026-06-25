use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};

use crate::models::{Candle, EvalResult, Genome, OrganismState, Signal, TradeRecord};
use crate::signals::STATUS_PENDING;

#[derive(Clone)]
pub struct Database {
    pool: Pool<Sqlite>,
}

impl Database {
    pub async fn connect(path: &str) -> Result<Self> {
        let url = if path.starts_with("sqlite:") {
            path.to_string()
        } else {
            // Solo creamos directorios para rutas de archivo reales.
            if let Some(parent) = std::path::Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent).await?;
                }
            }
            format!("sqlite://{}?mode=rwc", path)
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await?;
        Ok(Self { pool })
    }

    pub async fn init(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at TEXT NOT NULL,
                seed INTEGER NOT NULL,
                initial_capital REAL NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS organism_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                updated_at TEXT NOT NULL,
                cash REAL NOT NULL,
                position_qty REAL NOT NULL,
                entry_price REAL NOT NULL,
                equity REAL NOT NULL,
                initial_capital REAL NOT NULL,
                delta_vs_initial REAL NOT NULL,
                alive INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS candles (
                ts TEXT PRIMARY KEY,
                open REAL NOT NULL,
                high REAL NOT NULL,
                low REAL NOT NULL,
                close REAL NOT NULL,
                volume REAL NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS generations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                day_start TEXT NOT NULL,
                day_end TEXT NOT NULL,
                avg_fitness REAL,
                best_fitness REAL,
                survival_rate REAL,
                extinction_happened INTEGER NOT NULL,
                champion_equity REAL NOT NULL,
                champion_delta REAL NOT NULL,
                seed INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                generation_id INTEGER NOT NULL,
                fitness REAL NOT NULL,
                equity_final REAL NOT NULL,
                max_drawdown REAL NOT NULL,
                survival_ratio REAL NOT NULL,
                trades_count INTEGER NOT NULL,
                genome BLOB NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                instrument TEXT NOT NULL,
                action TEXT NOT NULL,
                price REAL NOT NULL,
                expires_at TEXT NOT NULL,
                status TEXT NOT NULL,
                notified INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL,
                generation_id INTEGER,
                timestamp TEXT NOT NULL,
                side TEXT NOT NULL,
                price REAL NOT NULL,
                quantity REAL NOT NULL,
                value REAL NOT NULL,
                commission REAL NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn insert_run(&self, seed: i64, initial_capital: f64) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let res =
            sqlx::query("INSERT INTO runs(started_at, seed, initial_capital) VALUES(?, ?, ?)")
                .bind(now)
                .bind(seed)
                .bind(initial_capital)
                .execute(&self.pool)
                .await?;
        Ok(res.last_insert_rowid())
    }

    pub async fn load_organism_state(&self) -> Result<Option<OrganismState>> {
        let row = sqlx::query(
            "SELECT cash, position_qty, entry_price, equity, initial_capital, delta_vs_initial, alive
             FROM organism_state WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| OrganismState {
            cash: r.get::<f64, _>("cash"),
            position_qty: r.get::<f64, _>("position_qty"),
            entry_price: r.get::<f64, _>("entry_price"),
            equity: r.get::<f64, _>("equity"),
            initial_capital: r.get::<f64, _>("initial_capital"),
            delta_vs_initial: r.get::<f64, _>("delta_vs_initial"),
            alive: r.get::<i64, _>("alive") != 0,
        }))
    }

    pub async fn upsert_organism_state(&self, state: &OrganismState) -> Result<()> {
        sqlx::query(
            "INSERT INTO organism_state(
                id, updated_at, cash, position_qty, entry_price, equity, initial_capital, delta_vs_initial, alive
             ) VALUES(1, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                updated_at=excluded.updated_at,
                cash=excluded.cash,
                position_qty=excluded.position_qty,
                entry_price=excluded.entry_price,
                equity=excluded.equity,
                initial_capital=excluded.initial_capital,
                delta_vs_initial=excluded.delta_vs_initial,
                alive=excluded.alive",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(state.cash)
        .bind(state.position_qty)
        .bind(state.entry_price)
        .bind(state.equity)
        .bind(state.initial_capital)
        .bind(state.delta_vs_initial)
        .bind(if state.alive { 1i64 } else { 0i64 })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_candle(&self, candle: &Candle) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO candles(ts, open, high, low, close, volume) VALUES(?, ?, ?, ?, ?, ?)",
        )
        .bind(candle.ts.to_rfc3339())
        .bind(candle.open)
        .bind(candle.high)
        .bind(candle.low)
        .bind(candle.close)
        .bind(candle.volume)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_trade(&self, trade: &TradeRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO trades(agent_id, generation_id, timestamp, side, price, quantity, value, commission)
             VALUES(?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&trade.agent_id)
        .bind(trade.generation_id)
        .bind(trade.timestamp.to_rfc3339())
        .bind(&trade.side)
        .bind(trade.price)
        .bind(trade.quantity)
        .bind(trade.value)
        .bind(trade.commission)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_generation(
        &self,
        day_start: DateTime<Utc>,
        day_end: DateTime<Utc>,
        avg_fitness: f64,
        best_fitness: f64,
        survival_rate: f64,
        extinction_happened: bool,
        champion_equity: f64,
        champion_delta: f64,
        seed: i64,
    ) -> Result<i64> {
        let res = sqlx::query(
            "INSERT INTO generations(
                day_start, day_end, avg_fitness, best_fitness, survival_rate,
                extinction_happened, champion_equity, champion_delta, seed
            ) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(day_start.to_rfc3339())
        .bind(day_end.to_rfc3339())
        .bind(avg_fitness)
        .bind(best_fitness)
        .bind(survival_rate)
        .bind(if extinction_happened { 1i64 } else { 0i64 })
        .bind(champion_equity)
        .bind(champion_delta)
        .bind(seed)
        .execute(&self.pool)
        .await?;

        Ok(res.last_insert_rowid())
    }

    pub async fn insert_agent_result(&self, generation_id: i64, result: &EvalResult) -> Result<()> {
        let genome_blob = serde_json::to_vec(&result.genome)?;
        sqlx::query(
            "INSERT INTO agents(
                id, generation_id, fitness, equity_final, max_drawdown, survival_ratio, trades_count, genome
            ) VALUES(?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&result.agent_id)
        .bind(generation_id)
        .bind(result.fitness)
        .bind(result.equity_final)
        .bind(result.max_drawdown)
        .bind(result.survival_ratio)
        .bind(result.trades_count)
        .bind(genome_blob)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Inserta una nueva señal en estado `PENDING` (sin notificar). Devuelve su id.
    pub async fn insert_signal(
        &self,
        instrument: &str,
        action: &str,
        price: f64,
        created_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<i64> {
        let res = sqlx::query(
            "INSERT INTO signals(created_at, instrument, action, price, expires_at, status, notified)
             VALUES(?, ?, ?, ?, ?, ?, 0)",
        )
        .bind(created_at.to_rfc3339())
        .bind(instrument)
        .bind(action)
        .bind(price)
        .bind(expires_at.to_rfc3339())
        .bind(STATUS_PENDING)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// Marca como `EXPIRED` toda señal `PENDING` cuyo `expires_at` ya pasó.
    /// Devuelve cuántas filas se expiraron.
    pub async fn expire_stale_signals(&self, now: DateTime<Utc>) -> Result<u64> {
        let res = sqlx::query(
            "UPDATE signals SET status = 'EXPIRED'
             WHERE status = 'PENDING' AND expires_at < ?",
        )
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Señales aún `PENDING` que no se han notificado. Permite reintentar el
    /// envío en la siguiente vela si Telegram estuvo caído.
    pub async fn pending_unnotified_signals(&self, limit: i64) -> Result<Vec<Signal>> {
        let rows = sqlx::query(
            "SELECT id, created_at, instrument, action, price, expires_at, status, notified
             FROM signals
             WHERE status = 'PENDING' AND notified = 0
             ORDER BY created_at ASC, id ASC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows_to_signals(rows)
    }

    /// Marca una señal `PENDING` como `EXECUTED` (la acción la dispara el
    /// dashboard). Solo afecta señales pendientes; devuelve si cambió algo.
    pub async fn mark_signal_executed(&self, id: i64) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE signals SET status = 'EXECUTED' WHERE id = ? AND status = 'PENDING'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Marca una señal como ya notificada (idempotente).
    pub async fn mark_signal_notified(&self, id: i64) -> Result<()> {
        sqlx::query("UPDATE signals SET notified = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Devuelve las señales más recientes (orden descendente por fecha).
    /// La consume el dashboard web (Fase 3).
    #[allow(dead_code)]
    pub async fn recent_signals(&self, limit: i64) -> Result<Vec<Signal>> {
        let rows = sqlx::query(
            "SELECT id, created_at, instrument, action, price, expires_at, status, notified
             FROM signals ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows_to_signals(rows)
    }

    pub async fn load_recent_candles(&self, limit: i64) -> Result<Vec<Candle>> {
        let rows = sqlx::query(
            "SELECT ts, open, high, low, close, volume
             FROM candles ORDER BY ts DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut candles = Vec::with_capacity(rows.len());
        for row in rows {
            let ts_raw: String = row.get("ts");
            let ts = chrono::DateTime::parse_from_rfc3339(&ts_raw)?.with_timezone(&Utc);
            candles.push(Candle {
                ts,
                open: row.get("open"),
                high: row.get("high"),
                low: row.get("low"),
                close: row.get("close"),
                volume: row.get("volume"),
                funding_rate: 0.0,
            });
        }
        candles.reverse();
        Ok(candles)
    }

    pub async fn load_best_genome_from_latest_generation(&self) -> Result<Option<Genome>> {
        let latest = sqlx::query("SELECT id FROM generations ORDER BY id DESC LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = latest else {
            return Ok(None);
        };
        let generation_id: i64 = row.get("id");

        let best = sqlx::query(
            "SELECT genome FROM agents WHERE generation_id = ? ORDER BY fitness DESC LIMIT 1",
        )
        .bind(generation_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(best_row) = best else {
            return Ok(None);
        };

        let blob: Vec<u8> = best_row.get("genome");
        let genome: Genome = serde_json::from_slice(&blob)?;
        Ok(Some(genome))
    }

    pub async fn load_generation_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) as c FROM generations")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<i64, _>("c"))
    }
}

/// Convierte filas de la tabla `signals` en structs `Signal`.
fn rows_to_signals(rows: Vec<sqlx::sqlite::SqliteRow>) -> Result<Vec<Signal>> {
    let mut signals = Vec::with_capacity(rows.len());
    for row in rows {
        let created_raw: String = row.get("created_at");
        let expires_raw: String = row.get("expires_at");
        signals.push(Signal {
            id: row.get::<i64, _>("id"),
            created_at: DateTime::parse_from_rfc3339(&created_raw)?.with_timezone(&Utc),
            instrument: row.get("instrument"),
            action: row.get("action"),
            price: row.get("price"),
            expires_at: DateTime::parse_from_rfc3339(&expires_raw)?.with_timezone(&Utc),
            status: row.get("status"),
            notified: row.get::<i64, _>("notified") != 0,
        });
    }
    Ok(signals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{ACTION_BUY, STATUS_EXPIRED};
    use chrono::Duration;
    use uuid::Uuid;

    async fn temp_db() -> Database {
        let path = std::env::temp_dir().join(format!("mcpato_test_{}.sqlite", Uuid::new_v4()));
        let db = Database::connect(path.to_str().unwrap()).await.unwrap();
        db.init().await.unwrap();
        db
    }

    #[tokio::test]
    async fn signal_lifecycle_pending_then_expired() {
        let db = temp_db().await;
        let now = Utc::now();

        // Una señal que ya venció (expires_at en el pasado).
        let id = db
            .insert_signal("bitcoin", ACTION_BUY, 65000.0, now - Duration::seconds(600), now - Duration::seconds(300))
            .await
            .unwrap();
        assert!(id > 0);

        // Recién insertada está PENDING y sin notificar.
        let before = db.recent_signals(10).await.unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].status, STATUS_PENDING);
        assert!(!before[0].notified);

        // Tras expirar las vencidas, queda EXPIRED.
        let affected = db.expire_stale_signals(now).await.unwrap();
        assert_eq!(affected, 1);

        let after = db.recent_signals(10).await.unwrap();
        assert_eq!(after[0].status, STATUS_EXPIRED);
    }

    #[tokio::test]
    async fn execute_only_affects_pending() {
        let db = temp_db().await;
        let now = Utc::now();
        let id = db
            .insert_signal("bitcoin", ACTION_BUY, 65000.0, now, now + Duration::seconds(300))
            .await
            .unwrap();

        // Primera ejecución sobre una PENDING: cambia.
        assert!(db.mark_signal_executed(id).await.unwrap());
        assert_eq!(db.recent_signals(10).await.unwrap()[0].status, "EXECUTED");

        // Segunda vez (ya no está PENDING): no cambia.
        assert!(!db.mark_signal_executed(id).await.unwrap());
    }

    #[tokio::test]
    async fn fresh_signal_does_not_expire() {
        let db = temp_db().await;
        let now = Utc::now();

        // Señal vigente: expira en el futuro.
        db.insert_signal("bitcoin", ACTION_BUY, 65000.0, now, now + Duration::seconds(300))
            .await
            .unwrap();

        let affected = db.expire_stale_signals(now).await.unwrap();
        assert_eq!(affected, 0);
        assert_eq!(db.recent_signals(10).await.unwrap()[0].status, STATUS_PENDING);
    }
}
