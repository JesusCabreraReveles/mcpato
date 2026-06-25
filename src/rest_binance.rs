//! Backfill: descarga velas históricas recientes desde la API REST de Binance
//! para "calentar" el sistema al arrancar (contexto de indicadores, equity y
//! una generación temprana) sin esperar a que cierren velas en vivo.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use tokio::time::sleep;

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

/// Descarga TODAS las velas cerradas en el rango `[start_ms, end_ms]`, paginando
/// hacia adelante (Binance devuelve máx. 1000 velas por petición). Pensado para
/// el pre-entrenamiento: traer meses/años de histórico. Mete una pausa breve
/// entre páginas para no abusar del rate-limit de la API REST.
pub async fn fetch_klines_range(
    symbol: &str,
    interval: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<Candle>> {
    let now_ms = Utc::now().timestamp_millis();
    let mut all: Vec<Candle> = Vec::new();
    let mut cursor = start_ms;
    let mut pages = 0u32;

    loop {
        let url = format!(
            "https://api.binance.com/api/v3/klines?symbol={}&interval={}&startTime={}&endTime={}&limit=1000",
            symbol.to_uppercase(),
            interval,
            cursor,
            end_ms,
        );
        let body = reqwest::get(&url)
            .await?
            .error_for_status()?
            .text()
            .await?;

        // El último elemento puede ser la vela en formación: la descartamos vía now_ms.
        let batch = parse_klines(&body, now_ms)?;

        // Para avanzar el cursor necesitamos el último openTime crudo, aunque la
        // vela en formación se haya descartado de `batch`.
        let last_open = last_open_time(&body)?;

        let got = batch.len();
        all.extend(batch);
        pages += 1;

        match last_open {
            // Avanza el cursor justo después de la última vela recibida.
            Some(open) if open + 1 > cursor && open < end_ms => {
                cursor = open + 1;
            }
            // Sin avance posible (fin de datos o ya cubrimos el rango): paramos.
            _ => break,
        }

        // Salvaguarda: si una página vino vacía, no hay más histórico.
        if got == 0 {
            break;
        }

        sleep(Duration::from_millis(250)).await;
    }

    if pages > 1 {
        println!(
            "Histórico: {} velas descargadas en {} páginas",
            all.len(),
            pages
        );
    }
    Ok(all)
}

/// `openTime` (campo 0) de la última fila del payload, sin filtrar por estado.
fn last_open_time(body: &str) -> Result<Option<i64>> {
    let rows: Vec<Vec<serde_json::Value>> =
        serde_json::from_str(body).context("respuesta REST de klines no es JSON esperado")?;
    Ok(rows.last().and_then(|r| r.first()).and_then(|v| v.as_i64()))
}

/// Descarga el histórico de funding rates del perpetuo (USDT-M) en `[start, end]`.
/// Devuelve `(fundingTime_ms, rate)` ordenado ascendente. El funding se paga cada
/// 8h, así que son ~3 puntos al día (pocas páginas para años de datos).
pub async fn fetch_funding_rates(
    symbol: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<(i64, f64)>> {
    let mut out: Vec<(i64, f64)> = Vec::new();
    let mut cursor = start_ms;

    loop {
        let url = format!(
            "https://fapi.binance.com/fapi/v1/fundingRate?symbol={}&startTime={}&endTime={}&limit=1000",
            symbol.to_uppercase(),
            cursor,
            end_ms,
        );
        let body = reqwest::get(&url).await?.error_for_status()?.text().await?;
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(&body).context("respuesta REST de funding no es JSON esperado")?;
        if rows.is_empty() {
            break;
        }

        let mut last_time = cursor;
        for r in &rows {
            let t = r.get("fundingTime").and_then(|v| v.as_i64());
            let rate = r
                .get("fundingRate")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok());
            if let (Some(t), Some(rate)) = (t, rate) {
                out.push((t, rate));
                last_time = last_time.max(t);
            }
        }

        if rows.len() < 1000 || last_time + 1 > end_ms {
            break;
        }
        cursor = last_time + 1;
        sleep(Duration::from_millis(250)).await;
    }

    out.sort_by_key(|(t, _)| *t);
    Ok(out)
}

/// Adjunta a cada vela el funding rate vigente (último pago con `fundingTime <=
/// vela.ts`), por forward-fill. Ambas series deben venir ordenadas ascendentes.
pub fn merge_funding(candles: &mut [Candle], funding: &[(i64, f64)]) {
    let mut i = 0usize;
    let mut current = 0.0f64;
    for c in candles.iter_mut() {
        let ts_ms = c.ts.timestamp_millis();
        while i < funding.len() && funding[i].0 <= ts_ms {
            current = funding[i].1;
            i += 1;
        }
        c.funding_rate = current;
    }
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
            funding_rate: 0.0,
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
