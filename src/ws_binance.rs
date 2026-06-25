use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::connect_async;
use tungstenite::Message;

use crate::models::Candle;
use crate::rest_binance;

/// Si no llega ningún mensaje (ni dato de vela ni ping del servidor) en este
/// tiempo, consideramos la conexión muerta y reconectamos. Binance envía un
/// frame ping cada ~3 min, así que cualquier silencio mayor indica un socket
/// zombi (típico al despertar el laptop o tras una caída de red/NAT), que de
/// otro modo dejaría `read.next()` bloqueado para siempre.
const READ_TIMEOUT: Duration = Duration::from_secs(190);

/// Cuántas velas pedir por REST al (re)conectar para rellenar el hueco de
/// inactividad. Binance limita a 1000 por petición.
const RESYNC_LIMIT: u32 = 1000;

#[derive(Debug, Deserialize)]
struct WsEvent {
    k: WsKline,
}

#[derive(Debug, Deserialize)]
struct WsKline {
    #[serde(rename = "T")]
    close_time: i64,
    #[serde(rename = "o")]
    open: String,
    #[serde(rename = "h")]
    high: String,
    #[serde(rename = "l")]
    low: String,
    #[serde(rename = "c")]
    close: String,
    #[serde(rename = "v")]
    volume: String,
    #[serde(rename = "x")]
    is_closed: bool,
}

/// Ejecuta el stream de velas con reconexión robusta:
/// - Detecta conexiones muertas con un timeout de lectura (`READ_TIMEOUT`) y
///   reconecta en vez de quedarse colgado tras dormir/perder red.
/// - Responde a los `Ping` del servidor para mantener viva la conexión.
/// - Tras cada (re)conexión, rellena por REST las velas perdidas durante el
///   hueco de inactividad (`initial_last_ts` marca la última ya procesada).
/// - Los mensajes malformados se ignoran (no tumban el daemon).
pub async fn run_kline_stream<F, Fut>(
    symbol: &str,
    interval: &str,
    initial_last_ts: Option<DateTime<Utc>>,
    mut on_candle: F,
) -> Result<()>
where
    F: FnMut(Candle) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let stream_name = format!("{}@kline_{}", symbol.to_lowercase(), interval);
    let url = format!("wss://stream.binance.com:9443/ws/{}", stream_name);

    let mut backoff_secs = 1u64;
    let mut last_ts = initial_last_ts;

    loop {
        match connect_async(&url).await {
            Ok((ws_stream, _)) => {
                backoff_secs = 1;
                let (mut write, mut read) = ws_stream.split();

                // Recupera las velas cerradas durante el hueco de inactividad
                // antes de reanudar el stream en vivo. Propaga errores de
                // `on_candle` (p. ej. muerte del organismo); solo los fallos de
                // red del propio REST se tragan dentro de `resync`.
                resync(symbol, interval, &mut last_ts, &mut on_candle).await?;

                loop {
                    match timeout(READ_TIMEOUT, read.next()).await {
                        // Sin tráfico en READ_TIMEOUT: conexión muerta.
                        Err(_elapsed) => {
                            eprintln!(
                                "warn: WS sin datos en {}s; reconectando",
                                READ_TIMEOUT.as_secs()
                            );
                            break;
                        }
                        // El stream terminó.
                        Ok(None) => break,
                        Ok(Some(msg)) => match msg {
                            Ok(Message::Text(text)) => {
                                if let Some(candle) = parse_candle_lenient(&text) {
                                    last_ts = Some(candle.ts);
                                    on_candle(candle).await?;
                                }
                            }
                            Ok(Message::Binary(bin)) => {
                                if let Ok(text) = String::from_utf8(bin.to_vec()) {
                                    if let Some(candle) = parse_candle_lenient(&text) {
                                        last_ts = Some(candle.ts);
                                        on_candle(candle).await?;
                                    }
                                }
                            }
                            // Responde al ping para mantener viva la conexión
                            // (tras split() no hay pong automático).
                            Ok(Message::Ping(payload)) => {
                                let _ = write.send(Message::Pong(payload)).await;
                            }
                            Ok(Message::Pong(_)) => {}
                            Ok(Message::Close(_)) => break,
                            Ok(_) => {}
                            Err(_) => break,
                        },
                    }
                }
            }
            Err(_) => {
                sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(30);
            }
        }
    }
}

/// Tras (re)conectar, pide las velas recientes por REST y alimenta a
/// `on_candle` únicamente las cerradas posteriores a `last_ts`, recuperando el
/// hueco de inactividad. Un fallo de red del REST no es fatal (se reanuda con
/// el stream en vivo); los errores de `on_candle` sí se propagan.
async fn resync<F, Fut>(
    symbol: &str,
    interval: &str,
    last_ts: &mut Option<DateTime<Utc>>,
    on_candle: &mut F,
) -> Result<()>
where
    F: FnMut(Candle) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let klines = match rest_binance::fetch_recent_klines(symbol, interval, RESYNC_LIMIT).await {
        Ok(k) => k,
        Err(e) => {
            eprintln!("warn: resync REST falló (se continúa con el stream en vivo): {e:#}");
            return Ok(());
        }
    };

    let mut recovered = 0usize;
    for candle in klines {
        if last_ts.map_or(true, |t| candle.ts > t) {
            *last_ts = Some(candle.ts);
            on_candle(candle).await?;
            recovered += 1;
        }
    }
    if recovered > 0 {
        println!("Resync: {recovered} velas recuperadas tras (re)conexión");
    }
    Ok(())
}

/// Variante tolerante: ante un payload malformado, registra y devuelve `None`
/// en lugar de propagar el error (un mensaje corrupto no debe tumbar el daemon).
fn parse_candle_lenient(payload: &str) -> Option<Candle> {
    match parse_candle(payload) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("warn: payload WS ignorado (parseo falló): {e:#}");
            None
        }
    }
}

fn parse_candle(payload: &str) -> Result<Option<Candle>> {
    let evt: WsEvent =
        serde_json::from_str(payload).context("failed to parse Binance WS payload")?;
    if !evt.k.is_closed {
        return Ok(None);
    }

    let ts = Utc
        .timestamp_millis_opt(evt.k.close_time)
        .single()
        .ok_or_else(|| anyhow::anyhow!("invalid close time"))?;

    Ok(Some(Candle {
        ts,
        open: evt.k.open.parse()?,
        high: evt.k.high.parse()?,
        low: evt.k.low.parse()?,
        close: evt.k.close.parse()?,
        volume: evt.k.volume.parse()?,
    }))
}
