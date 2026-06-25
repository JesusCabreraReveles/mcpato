# Bot de carry (delta-neutral) — diseño y operación

La conclusión accionable de la [bitácora de experimentos](./EXPERIMENTOS.md): la
única estrategia con renta real es el **carry de BTC**. Este es el bot que la
implementa (paper trading).

## Qué hace

Mantiene una posición **delta-neutral**: long spot + short perpetuo del mismo
notional (1x por defecto). El movimiento de precio se cancela entre las dos
patas; el PnL es el **funding** que cobra la pata corta cada 8h (positivo cuando
los longs pagan a los shorts). **No predice nada** — cobra una renta estructural.

- **Acumulación de funding:** en cada liquidación (00/08/16 UTC) lee el funding
  real por REST y lo acumula (`equity += rate × notional`).
- **Control de riesgo (histéresis):** media móvil del funding sobre una ventana
  (9 liquidaciones ≈ 3 días). Si cae bajo `EXIT_THR` se **aplana a CASH** (deja
  de cobrar/pagar); re-entra al superar `ENTRY_THR`. La banda muerta
  (`EXIT_THR < ENTRY_THR`) evita el *churn*: cada salida+entrada cuesta ~0.4%, que
  en periodos de funding flojo se comería la renta. Por eso reacciona a
  **regímenes** sostenidos, no a prints negativos sueltos.
- **Persistencia:** estado en la tabla `carry_state` → sobrevive reinicios.
- **Robustez de red:** el funding se lee por REST con reintento (sin sockets
  frágiles); un corte no tumba el bot.
- **Visibilidad:** dashboard web (`/` y `/api/carry`) y resumen diario + avisos de
  transición por Telegram.

## Cómo se ejecuta

```bash
MCPATO_CARRY_BOT=true ./target/release/mcpato
```

### Variables de entorno

| Variable | Default | Qué hace |
|---|---|---|
| `MCPATO_CARRY_BOT` | — | `true` activa el bot (modo daemon) |
| `MCPATO_SYMBOL` | `BTCUSDT` | Símbolo (spot + perp) |
| `MCPATO_INITIAL_CAPITAL` | `100` | Capital inicial (paper) |
| `MCPATO_CARRY_LEVERAGE` | `1.0` | Apalancamiento (1x = sin riesgo de liquidación) |
| `MCPATO_CARRY_POLL_SECS` | `300` | Cada cuánto comprueba nuevas liquidaciones |
| `MCPATO_CARRY_NEG_WINDOW` | `9` | Liquidaciones para la media móvil del riesgo |
| `MCPATO_CARRY_EXIT_THR` | `-0.00005` | Media de funding bajo la cual se va a cash |
| `MCPATO_CARRY_ENTRY_THR` | `0.00005` | Media de funding sobre la cual re-entra |
| `MCPATO_HTTP_*` | (ver DEPLOY) | Dashboard web |
| `MCPATO_TELEGRAM_*` | — | Notificaciones |

## Expectativas honestas

- **Retorno modesto:** ~**5%/año a 1x** (el funding de BTC fue positivo el 80% del
  tiempo en el backtest de 2 años). Con apalancamiento sube el retorno **y** el
  riesgo de liquidación.
- **El backtest sobreestima:** el modelo asume neutralidad perfecta. En vivo hay
  *basis risk* (spot y perp no se mueven idénticos), coste de rebalancear el hedge
  y riesgo de exchange. El Sharpe real estará **muy por debajo** del backtest.
- Pero **gana dinero de verdad y con riesgo mínimo**, que es más de lo que logró
  cualquier enfoque predictivo (ver bitácora).

## Estado y siguientes pasos

Implementado (paper): posición delta-neutral, acumulación de funding, control de
riesgo con histéresis, persistencia, dashboard y notificaciones.

Refinamientos hacia el realismo (pendientes):
1. Modelar las **dos patas con precios separados** (spot y mark del perp) para
   capturar el *basis* y el coste real de rebalancear el hedge.
2. **Órdenes reales** (paso deliberado, con más salvaguardas) — hoy es paper.
3. Despliegue en la Raspberry como servicio (ver [DEPLOY.md](../DEPLOY.md)); nota:
   convive o sustituye al daemon del organismo según se decida.
