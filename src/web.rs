//! Dashboard web (axum): tabla de señales + salud del organismo.
//!
//! Corre en una tarea aparte del stream de mercado. Expone:
//! - `GET  /`                         -> dashboard HTML
//! - `GET  /api/signals`              -> señales recientes (JSON, en español)
//! - `GET  /api/health`              -> salud (equity, vivo, última vela)
//! - `POST /api/signals/{id}/execute` -> marca una señal como ejecutada

use std::net::SocketAddr;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Serialize;

use crate::config::Config;
use crate::db::Database;
use crate::models::Signal;
use crate::signals::{ACTION_BUY, STATUS_EXECUTED, STATUS_EXPIRED, STATUS_PENDING};

const DASHBOARD_HTML: &str = include_str!("dashboard.html");

#[derive(Clone)]
struct AppState {
    db: Database,
    stale_after_secs: i64,
}

/// Arranca el servidor HTTP. Devuelve error solo si no puede bindear el puerto;
/// el caller lo ejecuta en una tarea y loguea sin tumbar el daemon.
pub async fn serve(cfg: Config, db: Database) -> anyhow::Result<()> {
    let state = AppState {
        db,
        stale_after_secs: cfg.stale_after_secs,
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/api/signals", get(api_signals))
        .route("/api/health", get(api_health))
        .route("/api/signals/{id}/execute", post(api_execute))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", cfg.http_bind, cfg.http_port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Dashboard web en http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

#[derive(Serialize)]
struct SignalView {
    id: i64,
    fecha: String,
    instrumento: String,
    accion: String,
    status: String,
    status_code: String,
    price: f64,
}

fn to_view(s: Signal) -> SignalView {
    let accion = if s.action == ACTION_BUY { "comprar" } else { "vender" };
    let status = match s.status.as_str() {
        STATUS_PENDING => "pendiente",
        STATUS_EXECUTED => "ejecutada",
        STATUS_EXPIRED => "expirada",
        other => other,
    };
    SignalView {
        id: s.id,
        fecha: s.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        instrumento: s.instrument,
        accion: accion.to_string(),
        status: status.to_string(),
        status_code: s.status,
        price: s.price,
    }
}

async fn api_signals(State(st): State<AppState>) -> Result<Json<Vec<SignalView>>, AppError> {
    let signals = st.db.recent_signals(100).await?;
    Ok(Json(signals.into_iter().map(to_view).collect()))
}

#[derive(Serialize)]
struct HealthView {
    alive: bool,
    equity: Option<f64>,
    delta_vs_initial: Option<f64>,
    generation_count: i64,
    last_candle_ts: Option<String>,
    last_candle_age_secs: Option<i64>,
    candle_stale: bool,
}

async fn api_health(State(st): State<AppState>) -> Result<Json<HealthView>, AppError> {
    let org = st.db.load_organism_state().await?;
    let generation_count = st.db.load_generation_count().await?;
    let last = st.db.load_recent_candles(1).await?;

    let (last_candle_ts, last_candle_age_secs) = match last.last() {
        Some(c) => (
            Some(c.ts.to_rfc3339()),
            Some((Utc::now() - c.ts).num_seconds()),
        ),
        None => (None, None),
    };
    let candle_stale = last_candle_age_secs
        .map(|a| a > st.stale_after_secs)
        .unwrap_or(false);

    Ok(Json(HealthView {
        alive: org.as_ref().map(|o| o.alive).unwrap_or(true),
        equity: org.as_ref().map(|o| o.equity),
        delta_vs_initial: org.as_ref().map(|o| o.delta_vs_initial),
        generation_count,
        last_candle_ts,
        last_candle_age_secs,
        candle_stale,
    }))
}

#[derive(Serialize)]
struct ExecResult {
    ok: bool,
    changed: bool,
}

async fn api_execute(
    State(st): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ExecResult>, AppError> {
    let changed = st.db.mark_signal_executed(id).await?;
    Ok(Json(ExecResult { ok: true, changed }))
}

/// Adaptador de errores a respuesta HTTP 500 (sin filtrar pánico al daemon).
struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("error: {:#}", self.0),
        )
            .into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
