//! Notificaciones por Telegram para señales de compra/venta.
//!
//! Diseño de confiabilidad:
//! - El notificador NUNCA tumba el daemon: si Telegram falla, se loguea y se
//!   sigue operando. La señal queda con `notified = 0` y se reintenta en la
//!   siguiente vela (ver `runtime`).
//! - Si no hay token/chat_id (o `MCPATO_NOTIFY_ENABLED=false`), queda
//!   deshabilitado silenciosamente.

use std::time::Duration;

use crate::config::Config;
use crate::models::Signal;
use crate::signals::ACTION_BUY;

#[derive(Clone)]
pub struct Notifier {
    client: reqwest::Client,
    token: String,
    chat_id: String,
    enabled: bool,
}

impl Notifier {
    pub fn from_config(cfg: &Config) -> Self {
        let enabled = cfg.notify_enabled
            && !cfg.telegram_token.trim().is_empty()
            && !cfg.telegram_chat_id.trim().is_empty();

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.notify_timeout_secs.max(1)))
            .build()
            .unwrap_or_default();

        Self {
            client,
            token: cfg.telegram_token.trim().to_string(),
            chat_id: cfg.telegram_chat_id.trim().to_string(),
            enabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Envía la señal por Telegram. Devuelve `Ok(())` si se entregó; `Err` si
    /// falló (el caller decide solo loguear, sin propagar el error al daemon).
    pub async fn send_signal(&self, signal: &Signal) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.token);
        let text = format_signal_message(signal);

        let resp = self
            .client
            .post(&url)
            .form(&[
                ("chat_id", self.chat_id.as_str()),
                ("text", text.as_str()),
                ("parse_mode", "HTML"),
                ("disable_web_page_preview", "true"),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("telegram respondió {status}: {body}");
        }

        Ok(())
    }
}

/// Construye el texto del mensaje (HTML) para una señal. Función pura: testeable
/// sin red.
pub fn format_signal_message(signal: &Signal) -> String {
    let (emoji, verbo) = if signal.action == ACTION_BUY {
        ("\u{1F7E2}", "COMPRAR") // círculo verde
    } else {
        ("\u{1F534}", "VENDER") // círculo rojo
    };

    format!(
        "{emoji} <b>{verbo} {instrument}</b>\n\
         Precio: <b>{price:.2}</b>\n\
         Emitida: {created}\n\
         Expira: {expires}",
        instrument = signal.instrument,
        price = signal.price,
        created = signal.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
        expires = signal.expires_at.format("%Y-%m-%d %H:%M:%S UTC"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{ACTION_BUY, ACTION_SELL, STATUS_PENDING};
    use chrono::{TimeZone, Utc};

    fn sample(action: &str) -> Signal {
        Signal {
            id: 1,
            created_at: Utc.with_ymd_and_hms(2026, 6, 23, 12, 56, 39).unwrap(),
            instrument: "bitcoin".to_string(),
            action: action.to_string(),
            price: 65000.12345,
            expires_at: Utc.with_ymd_and_hms(2026, 6, 23, 13, 1, 39).unwrap(),
            status: STATUS_PENDING.to_string(),
            notified: false,
        }
    }

    #[test]
    fn buy_message_has_buy_verb_and_price() {
        let msg = format_signal_message(&sample(ACTION_BUY));
        assert!(msg.contains("COMPRAR"));
        assert!(msg.contains("bitcoin"));
        assert!(msg.contains("65000.12"));
        assert!(msg.contains("2026-06-23 12:56:39"));
    }

    #[test]
    fn sell_message_has_sell_verb() {
        let msg = format_signal_message(&sample(ACTION_SELL));
        assert!(msg.contains("VENDER"));
        assert!(!msg.contains("COMPRAR"));
    }

    #[test]
    fn notifier_disabled_without_credentials() {
        let mut cfg = crate::config::Config::test_default();
        cfg.notify_enabled = true;
        cfg.telegram_token = "".to_string();
        cfg.telegram_chat_id = "".to_string();
        assert!(!Notifier::from_config(&cfg).is_enabled());

        cfg.telegram_token = "123:abc".to_string();
        cfg.telegram_chat_id = "999".to_string();
        assert!(Notifier::from_config(&cfg).is_enabled());

        cfg.notify_enabled = false;
        assert!(!Notifier::from_config(&cfg).is_enabled());
    }
}
