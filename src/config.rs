use anyhow::Result;

#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: String,
    pub symbol: String,
    pub interval: String,
    pub initial_capital: f64,
    pub p_mut: f64,
    pub sigma_small: f64,
    pub sigma_med: f64,
    pub sigma_big: f64,
    pub p_big: f64,
    pub p_med: f64,
    pub signal_threshold: f64,
    pub commission: f64,
    pub slippage: f64,
    pub survival_rate: f64,
    pub instrument_name: String,
    pub signal_ttl_secs: i64,
    pub position_epsilon: f64,
    pub notify_enabled: bool,
    pub telegram_token: String,
    pub telegram_chat_id: String,
    pub notify_timeout_secs: u64,
    pub http_enabled: bool,
    pub http_bind: String,
    pub http_port: u16,
    pub stale_after_secs: i64,
    pub backfill_enabled: bool,
    pub backfill_limit: u32,
    pub max_generations: Option<usize>,
    pub population_size: usize,
    pub candles_per_day: usize,
    pub survival_fee_cadence_candles: usize,
    pub seed: i64,
    pub pretrain_enabled: bool,
    pub pretrain_days: u32,
    pub eval_windows: usize,
    pub fit_w_alpha: f64,
    pub fit_w_absolute: f64,
    pub fit_w_sharpe: f64,
    pub fit_w_dd: f64,
    pub fit_death_penalty: f64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Self {
            db_path: std::env::var("MCPATO_DB")
                .unwrap_or_else(|_| "./data/mcpato.sqlite".to_string()),
            symbol: std::env::var("MCPATO_SYMBOL").unwrap_or_else(|_| "BTCUSDT".to_string()),
            interval: std::env::var("MCPATO_INTERVAL").unwrap_or_else(|_| "5m".to_string()),
            initial_capital: parse_or_default("MCPATO_INITIAL_CAPITAL", 100.0)?,
            p_mut: parse_or_default("MCPATO_P_MUT", 0.05)?,
            sigma_small: parse_or_default("MCPATO_SIGMA_SMALL", 0.02)?,
            sigma_med: parse_or_default("MCPATO_SIGMA_MED", 0.08)?,
            sigma_big: parse_or_default("MCPATO_SIGMA_BIG", 0.25)?,
            p_big: parse_or_default("MCPATO_P_BIG", 0.01)?,
            p_med: parse_or_default("MCPATO_P_MED", 0.09)?,
            signal_threshold: parse_or_default("MCPATO_SIGNAL_THRESHOLD", 0.2)?,
            commission: parse_or_default("MCPATO_COMMISSION", 0.001)?,
            slippage: parse_or_default("MCPATO_SLIPPAGE", 0.0002)?,
            survival_rate: parse_or_default("MCPATO_SURVIVAL_RATE", 0.0005)?,
            instrument_name: std::env::var("MCPATO_INSTRUMENT_NAME")
                .unwrap_or_else(|_| "bitcoin".to_string()),
            signal_ttl_secs: parse_or_default("MCPATO_SIGNAL_TTL_SECS", 300)?,
            position_epsilon: parse_or_default("MCPATO_POSITION_EPSILON", 1e-9)?,
            notify_enabled: parse_or_default("MCPATO_NOTIFY_ENABLED", true)?,
            telegram_token: std::env::var("MCPATO_TELEGRAM_TOKEN").unwrap_or_default(),
            telegram_chat_id: std::env::var("MCPATO_TELEGRAM_CHAT_ID").unwrap_or_default(),
            notify_timeout_secs: parse_or_default("MCPATO_NOTIFY_TIMEOUT_SECS", 10)?,
            http_enabled: parse_or_default("MCPATO_HTTP_ENABLED", true)?,
            http_bind: std::env::var("MCPATO_HTTP_BIND").unwrap_or_else(|_| "127.0.0.1".to_string()),
            http_port: parse_or_default("MCPATO_HTTP_PORT", 8080u16)?,
            stale_after_secs: parse_or_default("MCPATO_STALE_AFTER_SECS", 900)?,
            backfill_enabled: parse_or_default("MCPATO_BACKFILL_ENABLED", true)?,
            backfill_limit: parse_or_default("MCPATO_BACKFILL_LIMIT", 300u32)?,
            max_generations: parse_optional("MCPATO_MAX_GENERATIONS")?,
            population_size: parse_or_default("MCPATO_POPULATION_SIZE", 10usize)?.max(2),
            candles_per_day: parse_or_default("MCPATO_CANDLES_PER_DAY", 288usize)?.max(1),
            survival_fee_cadence_candles: 144,
            seed: chrono::Utc::now().timestamp(),
            pretrain_enabled: parse_or_default("MCPATO_PRETRAIN_ENABLED", false)?,
            pretrain_days: parse_or_default("MCPATO_PRETRAIN_DAYS", 90u32)?,
            eval_windows: parse_or_default("MCPATO_EVAL_WINDOWS", 1usize)?.max(1),
            fit_w_alpha: parse_or_default("MCPATO_FIT_W_ALPHA", 1.0)?,
            fit_w_absolute: parse_or_default("MCPATO_FIT_W_ABSOLUTE", 0.3)?,
            fit_w_sharpe: parse_or_default("MCPATO_FIT_W_SHARPE", 0.5)?,
            fit_w_dd: parse_or_default("MCPATO_FIT_W_DD", 0.3)?,
            fit_death_penalty: parse_or_default("MCPATO_FIT_DEATH_PENALTY", -1.0)?,
        })
    }
}

#[cfg(test)]
impl Config {
    /// Config mínima para tests (sin leer entorno).
    pub fn test_default() -> Self {
        Self {
            db_path: "sqlite::memory:".to_string(),
            symbol: "BTCUSDT".to_string(),
            interval: "5m".to_string(),
            initial_capital: 100.0,
            p_mut: 0.05,
            sigma_small: 0.02,
            sigma_med: 0.08,
            sigma_big: 0.25,
            p_big: 0.01,
            p_med: 0.09,
            signal_threshold: 0.2,
            commission: 0.001,
            slippage: 0.0002,
            survival_rate: 0.0005,
            instrument_name: "bitcoin".to_string(),
            signal_ttl_secs: 300,
            position_epsilon: 1e-9,
            notify_enabled: false,
            telegram_token: String::new(),
            telegram_chat_id: String::new(),
            notify_timeout_secs: 10,
            http_enabled: false,
            http_bind: "127.0.0.1".to_string(),
            http_port: 8080,
            stale_after_secs: 900,
            backfill_enabled: false,
            backfill_limit: 300,
            max_generations: None,
            population_size: 10,
            candles_per_day: 288,
            survival_fee_cadence_candles: 144,
            seed: 0,
            pretrain_enabled: false,
            pretrain_days: 90,
            eval_windows: 1,
            fit_w_alpha: 1.0,
            fit_w_absolute: 0.3,
            fit_w_sharpe: 0.5,
            fit_w_dd: 0.3,
            fit_death_penalty: -1.0,
        }
    }
}

fn parse_or_default<T: std::str::FromStr>(key: &str, default: T) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(raw) => raw
            .parse::<T>()
            .map_err(|e| anyhow::anyhow!("invalid {}: {}", key, e)),
        Err(_) => Ok(default),
    }
}

fn parse_optional<T: std::str::FromStr>(key: &str) -> Result<Option<T>>
where
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(raw) if raw.trim().is_empty() => Ok(None),
        Ok(raw) => raw
            .parse::<T>()
            .map(Some)
            .map_err(|e| anyhow::anyhow!("invalid {}: {}", key, e)),
        Err(_) => Ok(None),
    }
}
