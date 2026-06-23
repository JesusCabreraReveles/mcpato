use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tungstenite::Message;

use crate::models::Candle;

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

pub async fn run_kline_stream<F, Fut>(symbol: &str, interval: &str, mut on_candle: F) -> Result<()>
where
    F: FnMut(Candle) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let stream_name = format!("{}@kline_{}", symbol.to_lowercase(), interval);
    let url = format!("wss://stream.binance.com:9443/ws/{}", stream_name);

    let mut backoff_secs = 1u64;

    loop {
        match connect_async(&url).await {
            Ok((ws_stream, _)) => {
                backoff_secs = 1;
                let (_, mut read) = ws_stream.split();

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Some(candle) = parse_candle(&text)? {
                                on_candle(candle).await?;
                            }
                        }
                        Ok(Message::Binary(bin)) => {
                            if let Ok(text) = String::from_utf8(bin.to_vec()) {
                                if let Some(candle) = parse_candle(&text)? {
                                    on_candle(candle).await?;
                                }
                            }
                        }
                        Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(_) => break,
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
