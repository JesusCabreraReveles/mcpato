# mcpato

`mcpato` es un organismo financiero evolutivo (paper trading) que corre como daemon de larga vida.

Objetivo de esta primera versión (V0):
- conectarse a Binance WebSocket (`BTCUSDT`, velas `5m`, solo velas cerradas),
- operar un campeón en tiempo real sobre una cuenta persistente,
- cobrar una tarifa de supervivencia periódica,
- ejecutar una frontera de generación diaria con evaluación evolutiva,
- persistir todo en SQLite.

## 1) Idea del sistema

El sistema mezcla dos planos:
- **Plano live (campeón):** un único organismo que opera continuamente y cuya cuenta persiste entre reinicios.
- **Plano evolutivo diario:** cada 288 velas cerradas (1 día en 5m), se evalúa una población de 10 genomas sobre ese día y se selecciona el siguiente campeón.

Si el organismo no puede pagar la tarifa de supervivencia, muere y el proceso termina con error.

## 2) Reglas biológicas/financieras implementadas

- Mercado: `BTCUSDT` spot.
- Timeframe: `5m`.
- Episodio diario: `288` velas.
- Población: `10`.
- Solo largo (sin short, sin leverage).
- Tamaño fraccional de posición (`f64`).
- Comisión por trade: `MCPATO_COMMISSION` (default `0.001`).
- Slippage por trade: `MCPATO_SLIPPAGE` (default `0.0002`).
- Tarifa de supervivencia cada 12h: `fee = equity * MCPATO_SURVIVAL_RATE` (default `0.0005`).
  - Si no hay cash, vende posición para cubrir.
  - Si aun así no alcanza, `alive = false`, se persiste estado final y el proceso falla.

## 3) Arquitectura por módulos

- `/Users/jesus/code/mcPato/src/main.rs`
  - Carga configuración, imprime parámetros de arranque y ejecuta runtime.
- `/Users/jesus/code/mcPato/src/config.rs`
  - Lee `.env`/entorno con `dotenvy` y aplica defaults.
- `/Users/jesus/code/mcPato/src/ws_binance.rs`
  - Cliente WebSocket Binance con reconexión y backoff exponencial.
  - Solo procesa kline cerrada (`x == true`).
- `/Users/jesus/code/mcPato/src/db.rs`
  - Conexión SQLite vía `sqlx` y creación automática de tablas.
  - CRUD de estado, velas, trades, generaciones y agentes.
- `/Users/jesus/code/mcPato/src/broker.rs`
  - Broker paper long-only: rebalanceo, fees, slippage, equity y drawdown.
- `/Users/jesus/code/mcPato/src/indicators.rs`
  - Construcción de features numéricas para la red.
- `/Users/jesus/code/mcPato/src/nn.rs`
  - MLP pequeña (12 -> 8 -> 2) y codificación de genoma.
- `/Users/jesus/code/mcPato/src/agent.rs`
  - Simulador de un agente sobre buffer diario.
- `/Users/jesus/code/mcPato/src/evolution.rs`
  - Evaluación poblacional, selección, mutación y rescate de extinción.
- `/Users/jesus/code/mcPato/src/runtime.rs`
  - Bucle principal: ingestión live, trading del campeón, persistencia y frontera de generación.
- `/Users/jesus/code/mcPato/src/models.rs`
  - Estructuras de datos del dominio.

## 4) Flujo de runtime (paso a paso)

1. Carga configuración desde entorno.
2. Abre SQLite y crea tablas (`CREATE TABLE IF NOT EXISTS`).
3. Inserta un registro en `runs`.
4. Recupera `organism_state` (si existe) para continuar cuenta; si no, inicializa capital.
5. Carga histórico reciente de velas para tener contexto inicial de indicadores.
6. Recupera campeón previo (mejor genoma de la última generación), o crea uno aleatorio.
7. Abre WebSocket Binance (`wss://stream.binance.com:9443/ws/btcusdt@kline_5m`).
8. En cada vela cerrada:
   - persiste vela,
   - calcula features,
   - infiere `(signal, risk)` con la red del campeón,
   - convierte señal a asignación objetivo,
   - rebalancea (compra/venta) con costos,
   - cobra survival fee cada 144 velas,
   - persiste estado del organismo,
   - acumula vela en buffer diario.
9. Cuando buffer diario llega a 288 velas:
   - evalúa población de 10 genomas en ese mismo día,
   - calcula fitness por agente,
   - inserta `generation` + `agents` + trades de evaluación,
   - elige nuevo campeón y crea nueva población mutada,
   - imprime resumen de generación,
   - limpia buffer diario.
10. Si `MCPATO_MAX_GENERATIONS` está definido y se alcanzó, termina en forma controlada (`exit(0)`).

## 5) Datos de mercado (sin CSV)

No se usa CSV.

Fuente live:
- Binance WebSocket Kline Spot.
- Stream: `btcusdt@kline_5m`.
- Solo velas cerradas (`x == true`).

Backfill al arrancar (calentamiento):
- Binance REST `GET /api/v3/klines` (`MCPATO_BACKFILL_LIMIT` velas).
- Descarta la vela en formación (solo cerradas).
- Puebla contexto de indicadores, equity y buffer diario para no esperar a las
  primeras velas en vivo. Se controla con `MCPATO_BACKFILL_ENABLED`.

Reconexión:
- Si se cae el socket, reconecta con backoff exponencial de 1s a 30s.

## 6) Features de entrada (12)

Las features están acotadas con `clamp[-3, 3]`:

1. `ret_1` (log-return 1 vela)
2. `ret_5` (log-return 5 velas)
3. `ret_20` (log-return 20 velas)
4. `dist_ema_fast = (close - ema9)/close`
5. `dist_ema_slow = (close - ema21)/close`
6. `ema_cross = (ema9 - ema21)/close`
7. `rsi_norm = (RSI14 - 50)/50`
8. `volatility_20` (std de log-returns)
9. `atr_rel = ATR14/close`
10. `pos_frac = position_value/equity`
11. `unrealized_pnl_frac = unrealized_pnl/equity`
12. `drawdown_frac = max_drawdown`

## 7) Red neuronal y genoma

Arquitectura:
- Entradas: `12`
- Oculta: `8` (activación `tanh`)
- Salidas: `2`
  - `signal = tanh(out0)` en `[-1, 1]`
  - `risk = sigmoid(out1)` en `[0, 1]`

Codificación:
- `Genome { weights: Vec<f64> }`
- Longitud fija:
  - `12*8 + 8 + 8*2 + 2 = 122` pesos.

Interpretación de salida:
- Si `signal > threshold` -> `target_alloc = risk`
- Si `signal < -threshold` -> `target_alloc = 0`
- Si está entre umbrales -> mantiene asignación actual.

`threshold` configurable por `MCPATO_SIGNAL_THRESHOLD`.

## 8) Broker paper (long-only)

El broker hace rebalanceo por valor hacia asignación objetivo:
- `target_position_value = equity * target_alloc`
- `delta_value = target - current`

Si `delta_value > 0` compra; si `< 0` vende.

Costos:
- Compra: precio ejecutado `price * (1 + slippage)`, comisión sobre valor negociado.
- Venta: precio ejecutado `price * (1 - slippage)`, comisión sobre valor bruto vendido.

Supervivencia:
- Cada 144 velas (12h), calcula `fee_due = equity * survival_rate`.
- Si cash no alcanza, intenta vender la posición para cubrir.
- Si no alcanza tras vender, retorna fallo (muerte).

## 9) Evolución diaria

Cada generación (1 día de 288 velas):

1. Simula 10 agentes (cada uno con capital inicial `100.0`) sobre el mismo buffer diario.
2. Cada agente opera con misma lógica de broker + survival fee.
3. Métrica de fitness:

`fitness = 0.5*survival_ratio + 0.4*ln(equity_growth) - 0.1*max_drawdown`

Donde:
- `survival_ratio = lived_candles / 288`
- `equity_growth = equity_final / 100.0`
- `max_drawdown` en `[0, 1]`

Selección y diversidad:
- Se toma el mejor por fitness como base del próximo campeón.
- Nueva población: el mejor + variantes mutadas.

Mutación:
- Cada peso muta con probabilidad `p_mut`.
- Ruido multinivel (small/med/big) según `p_med/p_big` y `sigma_*`.
- Clamp final de pesos en `[-3, 3]`.

Extinción:
- Si nadie sobrevive el día, activa rescate:
  - elige padre ponderado por tiempo de vida,
  - genera población con mutación más agresiva (“radiation”).

## 10) Persistencia en SQLite

DB por defecto: `./data/mcpato.sqlite`

Tablas creadas automáticamente:
- `runs`
- `organism_state`
- `candles`
- `generations`
- `agents`
- `trades`

Detalles clave:
- `candles.ts` es `PRIMARY KEY` (insert con `INSERT OR IGNORE`).
- `organism_state.id=1` mantiene snapshot único de la cuenta live.
- `agents.genome` guarda el genoma serializado JSON en `BLOB`.
- trades del campeón usan `agent_id = "CHAMPION"`.

## 11) Variables de entorno

Referencia completa en:
- `/Users/jesus/code/mcPato/.env.example`

Principales:
- `MCPATO_DB`
- `MCPATO_SYMBOL`
- `MCPATO_INTERVAL`
- `MCPATO_INITIAL_CAPITAL`
- `MCPATO_P_MUT`, `MCPATO_SIGMA_*`, `MCPATO_P_*`
- `MCPATO_SIGNAL_THRESHOLD`
- `MCPATO_COMMISSION`
- `MCPATO_SLIPPAGE`
- `MCPATO_SURVIVAL_RATE`
- `MCPATO_INSTRUMENT_NAME` (nombre legible del instrumento en la tabla de señales)
- `MCPATO_SIGNAL_TTL_SECS` (vida de una señal `PENDING` antes de expirar)
- `MCPATO_POSITION_EPSILON` (umbral de posición para considerarla LONG)
- `MCPATO_NOTIFY_ENABLED` (activa/desactiva notificaciones Telegram)
- `MCPATO_TELEGRAM_TOKEN` (token del bot de @BotFather)
- `MCPATO_TELEGRAM_CHAT_ID` (chat destino de las notificaciones)
- `MCPATO_NOTIFY_TIMEOUT_SECS` (timeout por llamada a la API de Telegram)
- `MCPATO_HTTP_ENABLED` (activa/desactiva el dashboard web)
- `MCPATO_HTTP_BIND` (IP de escucha: `127.0.0.1` local, `0.0.0.0` red)
- `MCPATO_HTTP_PORT` (puerto del dashboard)
- `MCPATO_STALE_AFTER_SECS` (umbral de "datos atrasados" en el indicador de salud)
- `MCPATO_BACKFILL_ENABLED` (descarga histórico al arrancar)
- `MCPATO_BACKFILL_LIMIT` (cuántas velas históricas traer, máx 1000)
- `MCPATO_MAX_GENERATIONS` (opcional)

## 12) Ejecutar local

```bash
cd /Users/jesus/code/mcPato
cp .env.example .env
cargo run
```

Salida esperada al inicio:
- ruta de DB,
- símbolo/intervalo,
- capital inicial,
- survival rate,
- límite de generaciones (o `unbounded`).

Por cada generación diaria completada imprime:
- rango temporal del día,
- equity y delta del campeón,
- best fitness, avg fitness y survival rate.

## 12-bis) Señales de acción

Desde la Fase 1, `mcpato` emite **señales discretas** de compra/venta cuando el
campeón **cambia de postura** (no en cada vela):

- estaba fuera del mercado y decide entrar -> señal **COMPRAR**,
- estaba comprado y decide salir -> señal **VENDER**.

Cada señal se persiste en la tabla `signals` con un ciclo de vida pensado para
ti (el humano que ejecuta en tu exchange):

| status | significado |
|--------|-------------|
| `PENDING` | recién emitida, válida hasta `expires_at` (TTL = `MCPATO_SIGNAL_TTL_SECS`) |
| `EXECUTED` | la marcaste como ejecutada (lo hará el dashboard de la Fase 3) |
| `EXPIRED` | venció el TTL sin que actuaras |

El paper-broker sigue simulando por dentro (mide desempeño); las señales son el
canal "accionable" hacia ti.

## 12-ter) Notificaciones por Telegram (paso a paso)

Cuando nace una señal, `mcpato` te avisa por Telegram. Sigue estos pasos **una
sola vez** para dejarlo configurado.

### Paso 1 — Crear el bot con @BotFather

1. Abre Telegram y busca el contacto oficial **@BotFather** (tiene el check azul).
2. Inicia el chat y envía el comando:
   ```
   /newbot
   ```
3. BotFather te pedirá dos cosas:
   - un **nombre** visible (ej. `mcpato alerts`),
   - un **username** que debe terminar en `bot` (ej. `mcpato_alerts_bot`).
4. Al terminar, BotFather te responde con un **token** parecido a:
   ```
   123456789:AAH... (una cadena larga)
   ```
   Ese token es la línea `MCPATO_TELEGRAM_TOKEN`. **Trátalo como una contraseña:
   no lo subas a git ni lo compartas.**

### Paso 2 — Iniciar una conversación con tu bot

Telegram **no permite** que un bot te escriba si tú no le hablaste primero.

1. Busca tu bot por su username (ej. `@mcpato_alerts_bot`).
2. Abre el chat y pulsa **Start** (o envía cualquier mensaje, ej. `hola`).

> Si quieres recibir las alertas en un **grupo**, crea el grupo, agrega el bot
> como miembro y envía un mensaje cualquiera en el grupo.

### Paso 3 — Obtener tu `chat_id`

El `chat_id` identifica a dónde se envían los mensajes. Dos formas:

**Opción A — con @userinfobot (la más simple, para chat personal):**

1. En Telegram busca **@userinfobot** y pulsa Start.
2. Te responde con tu `Id` numérico (ej. `987654321`).
3. Ese número es `MCPATO_TELEGRAM_CHAT_ID`.

**Opción B — con la API (sirve también para grupos):**

1. Con el bot ya iniciado (Paso 2), abre en el navegador (reemplaza `<TOKEN>`):
   ```
   https://api.telegram.org/bot<TOKEN>/getUpdates
   ```
2. Busca en el JSON el campo `"chat":{"id":...}`:
   - para chat personal es un número positivo (ej. `987654321`),
   - para un grupo suele ser **negativo** (ej. `-1001234567890`).
3. Ese valor es `MCPATO_TELEGRAM_CHAT_ID`.

> Si `getUpdates` devuelve `result: []`, envíale primero un mensaje al bot
> (Paso 2) y recarga la URL.

### Paso 4 — Configurar el `.env`

En tu archivo `.env` (copiado de `.env.example`):

```bash
MCPATO_NOTIFY_ENABLED=true
MCPATO_TELEGRAM_TOKEN=123456789:AAH...    # el del Paso 1
MCPATO_TELEGRAM_CHAT_ID=987654321         # el del Paso 3
MCPATO_NOTIFY_TIMEOUT_SECS=10
```

### Paso 5 — Verificar que funciona

**Prueba rápida sin esperar una señal** (envía un mensaje manual con `curl`):

```bash
curl -s "https://api.telegram.org/bot<TOKEN>/sendMessage" \
  -d chat_id=<CHAT_ID> \
  -d text="prueba mcpato ✅"
```

Si te llega el mensaje, las credenciales están bien.

**Con el daemon:** al arrancar (`cargo run`) verás en consola:

```
Notificaciones Telegram: ACTIVADAS
```

Cuando el campeón genere una transición de postura recibirás un mensaje como:

```
🟢 COMPRAR bitcoin
Precio: 65000.00
Emitida: 2026-06-23 12:56:39 UTC
Expira: 2026-06-23 13:01:39 UTC
```

### Comportamiento y confiabilidad

- **Sin spam:** solo se notifica el cambio de postura (COMPRAR/VENDER), no cada
  rebalanceo.
- **Una vez por señal:** cada señal lleva un flag `notified`; no se reenvía.
- **Tolerante a fallos:** si Telegram está caído, la notificación **no tumba el
  daemon**; la señal queda pendiente y se **reintenta** en la siguiente vela
  (mientras no haya expirado).
- **Desactivar:** pon `MCPATO_NOTIFY_ENABLED=false` o deja vacíos el token /
  chat_id. En consola verás `Notificaciones Telegram: desactivadas`.
- **Seguridad:** el token vive solo en `.env` (que no se sube a git). Si se
  filtra, revócalo en @BotFather con `/revoke` y genera uno nuevo.

## 12-quater) Dashboard web

Desde la Fase 3, `mcpato` levanta un **dashboard web** en una tarea aparte del
stream de mercado. Es la pantalla visual con la tabla de acciones.

### Acceder

Con el daemon corriendo (`cargo run`), abre en el navegador:

```
http://127.0.0.1:8080
```

Al arrancar verás en consola:

```
Dashboard web en http://127.0.0.1:8080
```

La página se **auto-refresca cada 3 segundos** (no hace falta recargar).

### Qué muestra

- **Cabecera de salud:**
  - Estado (Vivo / Vivo con datos atrasados / Muerto),
  - Equity actual y delta vs. capital inicial,
  - Número de generaciones,
  - Antigüedad de la última vela (si el WebSocket murió, el indicador se pone
    en ámbar pasados `MCPATO_STALE_AFTER_SECS`).
- **Tabla de señales:** `Fecha | Instrumento | Acción | Status`, con un botón
  **"Marcar ejecutada"** en las señales `pendiente`.

| Fecha | Instrumento | Acción | Status |
|-------|-------------|--------|--------|
| 2026-06-23 12:56:39 | bitcoin | comprar | expirado |
| 2026-06-23 12:51:10 | bitcoin | vender  | ejecutada |
| 2026-06-23 12:40:00 | bitcoin | comprar | pendiente |

### Endpoints (JSON)

El dashboard consume una pequeña API que también puedes usar tú:

- `GET  /api/signals` — últimas 100 señales (campos en español).
- `GET  /api/health` — `{alive, equity, delta_vs_initial, generation_count,
  last_candle_ts, last_candle_age_secs, candle_stale}`.
- `POST /api/signals/{id}/execute` — marca la señal como `ejecutada` (solo si
  estaba `pendiente`).

### Configuración

Todo en `.env`:

```bash
MCPATO_HTTP_ENABLED=true     # pon false para no levantar el servidor
MCPATO_HTTP_BIND=127.0.0.1   # 0.0.0.0 para exponerlo en la red local
MCPATO_HTTP_PORT=8080
MCPATO_STALE_AFTER_SECS=900  # 15 min sin vela => indicador "datos atrasados"
```

### Notas

- **Resiliencia:** si el puerto está ocupado, el servidor loguea el error pero
  el trading **sigue corriendo**.
- **Seguridad:** con `MCPATO_HTTP_BIND=0.0.0.0` el dashboard queda accesible a
  cualquiera en tu red (no tiene autenticación). Para exponerlo a internet, usa
  un proxy con TLS y auth por delante.

## 13) Despliegue sugerido (Docker/EC2)

Este proyecto está diseñado para ejecutarse como proceso único de larga vida.

Recomendaciones:
- montar volumen persistente para `./data` (evitar perder DB),
- reinicio automático del contenedor/proceso,
- variables de entorno por secretos/config de infraestructura,
- logs hacia CloudWatch o similar,
- healthcheck externo (proceso vivo + crecimiento de velas en DB).

## 14) Limitaciones actuales de V0

- No hay métricas Prometheus (sí hay dashboard HTTP, ver sección 12-quater).
- No hay ejecución multi-símbolo.
- No hay migraciones versionadas; usa `CREATE TABLE IF NOT EXISTS` directo.
- El seed se basa en timestamp de arranque.
- Con `MCPATO_BACKFILL_ENABLED=true` (por defecto) el buffer diario se precarga
  con histórico REST, así que la primera generación se evalúa pronto y el
  dashboard muestra datos desde el arranque. Con backfill desactivado, el buffer
  empieza vacío (comportamiento V0).

## 15) Checklist rápido de validación

1. `cargo check` / `cargo run` compila y ejecuta.
2. Se insertan velas cerradas en `candles`.
3. Se actualiza `organism_state` con cada vela.
4. Aparecen trades `CHAMPION` cuando rebalancea o paga survival fee.
5. Tras 288 velas nuevas, se inserta una fila en `generations` y 10 en `agents`.

---

Si quieres, en el siguiente paso te preparo también un `Dockerfile` + `docker-compose.yml` mínimo para correrlo en EC2 con volumen persistente y reinicio automático.
