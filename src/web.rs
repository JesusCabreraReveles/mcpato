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

// ----------------------------------------------------------------------------
// Dashboard del bot de carry (servidor aparte, reusa el patrón de arriba).
// ----------------------------------------------------------------------------

const CARRY_HTML: &str = r#"<!doctype html><html lang="es"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>mcPato · Carry</title>
<style>
 body{font-family:system-ui,sans-serif;background:#0d1117;color:#e6edf3;margin:0;padding:2rem;}
 h1{font-weight:600;font-size:1.4rem} .muted{color:#8b949e}
 .grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:1rem;margin-top:1.5rem;max-width:760px}
 .card{background:#161b22;border:1px solid #30363d;border-radius:10px;padding:1rem 1.2rem}
 .card .lbl{color:#8b949e;font-size:.8rem;text-transform:uppercase;letter-spacing:.04em}
 .card .val{font-size:1.6rem;font-weight:600;margin-top:.3rem}
 .pos{color:#3fb950}.neg{color:#f85149}
 .pill{display:inline-block;padding:.2rem .7rem;border-radius:999px;font-size:.85rem}
 .open{background:#1a3326;color:#3fb950}.flat{background:#3a2a1a;color:#e3b341}
</style></head><body>
<h1>🦆 mcPato · Carry <span class="muted" id="sym"></span></h1>
<div id="state" class="muted">cargando…</div>
<div class="grid">
 <div class="card"><div class="lbl">Equity</div><div class="val" id="eq">—</div></div>
 <div class="card"><div class="lbl">Δ vs inicial</div><div class="val" id="delta">—</div></div>
 <div class="card"><div class="lbl">Funding acumulado</div><div class="val" id="fund">—</div></div>
 <div class="card"><div class="lbl">Retorno anualizado (est.)</div><div class="val" id="ann">—</div></div>
 <div class="card"><div class="lbl">Pagos cobrados</div><div class="val" id="pay">—</div></div>
 <div class="card"><div class="lbl">Posición</div><div class="val" id="pos">—</div></div>
</div>
<p class="muted" style="margin-top:2rem;font-size:.8rem">Paper trading · delta-neutral · actualiza cada 10s</p>
<script>
 const pct=x=>(x>=0?'+':'')+(x*100).toFixed(2)+'%';
 const cls=x=>x>=0?'pos':'neg';
 async function tick(){
  try{const r=await fetch('/api/carry');const d=await r.json();
   document.getElementById('sym').textContent=d.symbol||'';
   if(!d.exists){document.getElementById('state').textContent='sin estado todavía';return;}
   document.getElementById('state').textContent='';
   const eq=document.getElementById('eq');eq.textContent=d.equity.toFixed(2);
   const dl=document.getElementById('delta');dl.textContent=(d.delta_vs_initial>=0?'+':'')+d.delta_vs_initial.toFixed(2);dl.className='val '+cls(d.delta_vs_initial);
   const fu=document.getElementById('fund');fu.textContent=(d.accumulated_funding>=0?'+':'')+d.accumulated_funding.toFixed(2);fu.className='val '+cls(d.accumulated_funding);
   const an=document.getElementById('ann');if(d.ann_return_est!=null){an.textContent=pct(d.ann_return_est);an.className='val '+cls(d.ann_return_est);}else an.textContent='—';
   document.getElementById('pay').textContent=d.payments;
   const p=document.getElementById('pos');p.innerHTML=d.position_open?'<span class="pill open">ABIERTA</span>':'<span class="pill flat">EN CASH</span>';
  }catch(e){document.getElementById('state').textContent='error de conexión';}
 }
 tick();setInterval(tick,10000);
</script></body></html>"#;

#[derive(Clone)]
struct CarryAppState {
    db: Database,
    symbol: String,
}

/// Arranca el dashboard del bot de carry. Reusa el patrón de `serve`.
pub async fn serve_carry(cfg: Config, db: Database) -> anyhow::Result<()> {
    let state = CarryAppState {
        db,
        symbol: cfg.symbol.clone(),
    };
    let app = Router::new()
        .route("/", get(|| async { Html(CARRY_HTML) }))
        .route("/api/carry", get(api_carry))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", cfg.http_bind, cfg.http_port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Dashboard de carry en http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Serialize)]
struct CarryView {
    exists: bool,
    symbol: String,
    equity: f64,
    initial_capital: f64,
    delta_vs_initial: f64,
    accumulated_funding: f64,
    payments: i64,
    position_open: bool,
    ann_return_est: Option<f64>,
}

async fn api_carry(State(st): State<CarryAppState>) -> Result<Json<CarryView>, AppError> {
    let c = st.db.load_carry_state().await?;
    let view = match c {
        None => CarryView {
            exists: false,
            symbol: st.symbol,
            equity: 0.0,
            initial_capital: 0.0,
            delta_vs_initial: 0.0,
            accumulated_funding: 0.0,
            payments: 0,
            position_open: false,
            ann_return_est: None,
        },
        Some(c) => {
            // Estimación de retorno anualizado: cada pago ~8h => payments/3 días.
            let days = c.payments as f64 / 3.0;
            let ann = if days > 0.5 && c.initial_capital > 0.0 {
                Some((c.equity / c.initial_capital).powf(365.0 / days) - 1.0)
            } else {
                None
            };
            CarryView {
                exists: true,
                symbol: st.symbol,
                equity: c.equity,
                initial_capital: c.initial_capital,
                delta_vs_initial: c.equity - c.initial_capital,
                accumulated_funding: c.accumulated_funding,
                payments: c.payments,
                position_open: c.position_open,
                ann_return_est: ann,
            }
        }
    };
    Ok(Json(view))
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
