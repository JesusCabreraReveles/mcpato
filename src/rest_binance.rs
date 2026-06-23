//! Backfill: descarga velas históricas recientes desde la API REST de Binance
//! para "calentar" el sistema al arrancar (contexto de indicadores, equity y
//! una generación temprana) sin esperar a que cierren velas en vivo.

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};

use crate::models::Candle;

/// Descarga las últimas `limit` velas cerradas de Binance (spot).
pub async fn fetch_recent_klines(symbol: &str, interval: &str, limit: u32) -> Result<Vec<Candle>> {
    let url = format!(
        "https://api.binance.com/api/v3/klines?symbol={}&interval={}&limit={}",
        symbol.to_uppercase(),
        interval,
        limit.clamp(1, 1000),
    );
    let body = reqwest::get(&url)
        .await?
        .error_for_status()?
        .text()
        .await?;
    parse_klines(&body, Utc::now().timestamp_millis())
}

/// Parsea la respuesta REST de klines. Descarta la última vela si aún se está
/// formando (su `closeTime` está en el futuro respecto a `now_ms`).
pub fn parse_klines(body: &str, now_ms: i64) -> Result<Vec<Candle>> {
    let rows: Vec<Vec<serde_json::Value>> =
        serde_json::from_str(body).context("respuesta REST de klines no es JSON esperado")?;

    let num = |v: &serde_json::Value| -> Result<f64> {
        v.as_str()
            .context("campo numérico no es string")?
            .parse::<f64>()
            .context("campo numérico inválido")
    };

    let mut candles = Vec::with_capacity(rows.len());
    for r in rows {
        if r.len() < 7 {
            continue;
        }
        let close_time = r[6].as_i64().context("closeTime inválido")?;
        if close_time > now_ms {
            // Vela aún formándose: la ignoramos (solo velas cerradas).
            continue;
        }
        let ts = Utc
            .timestamp_millis_opt(close_time)
            .single()
            .context("closeTime fuera de rango")?;
        candles.push(Candle {
            ts,
            open: num(&r[1])?,
            high: num(&r[2])?,
            low: num(&r[3])?,
            close: num(&r[4])?,
            volume: num(&r[5])?,
        });
    }
    Ok(candles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_closed_and_drops_forming() {
        // closeTime 1000 (pasado) y 9_000_000_000_000 (futuro lejano).
        let body = r#"[
            [0,"100.0","110.0","90.0","105.0","12.5",1000,"0",0,"0","0","0"],
            [1000,"105.0","120.0","100.0","115.0","8.0",9000000000000,"0",0,"0","0","0"]
        ]"#;
        let candles = parse_klines(body, 5000).unwrap();
        assert_eq!(candles.len(), 1, "debe descartar la vela en formación");
        assert_eq!(candles[0].close, 105.0);
        assert_eq!(candles[0].high, 110.0);
    }

    #[test]
    fn empty_array_is_ok() {
        assert!(parse_klines("[]", 5000).unwrap().is_empty());
    }
}
