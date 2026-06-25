# Bitácora de experimentos — búsqueda de edge en mcPato

> Registro honesto de cómo, partiendo de un bot evolutivo que "preservaba capital
> pero no ganaba dinero", se descartaron sistemáticamente los paradigmas de
> predicción y se llegó a la única estrategia con renta real: **el carry de BTC**.
>
> Periodo de pruebas: histórico de Binance, ~720 días (≈2 años) salvo indicación.
> Todos los experimentos son reproducibles vía variables de entorno (ver al final).

---

## 0. El punto de partida y el problema

El sistema original: una red neuronal pequeña (MLP) evolucionada con un
hill-climber, que sobre velas de BTC decide una asignación **long-only** (0–100%).
Tras desplegarlo, el síntoma era claro: **drawdowns bajos pero alpha ≈ 0**. No
perdía mucho, pero tampoco ganaba. La pregunta de toda esta bitácora:

> ¿Se puede sacar **ventaja real** de aquí, y si sí, **cómo**?

---

## 1. Metodología (lo que hizo honesta la búsqueda)

Antes de los resultados, los principios que evitaron el autoengaño:

1. **Out-of-sample siempre.** Entrenar en datos viejos, medir en datos que el
   modelo **nunca vio**. La métrica de entrenamiento (in-sample) no dice nada.
2. **Walk-forward multi-régimen.** No una ventana, sino ~15-20 *folds*
   deslizantes que cubren mercados **alcistas, bajistas y laterales**. Distingue
   habilidad real de "tener suerte siendo defensivo en una caída".
3. **Sondas baratas antes de builds grandes.** Probar la premisa simple en horas
   antes de comprometer semanas. Si la versión cruda no tiene edge, la versión
   con ML tampoco.
4. **Una variable a la vez.** Para poder atribuir cada efecto.
5. **Desconfiar de las métricas que mienten.** El Sharpe es **ciego a las colas
   gordas**; un barrido de parámetros encuentra el config con suerte (cherry-pick).

El criterio de éxito ("Riesgo primero"): un fold **pasa** si el Sharpe OOS > 0 y
el drawdown máximo está bajo un tope (15%).

---

## 2. Fase A — Mejorar el motor (predicción single-asset)

### A.1 Función de fitness rediseñada
El fitness viejo quedaba clavado en ~0.49: la supervivencia (peso 0.5, ≈1.0 para
todos) ahogaba el beneficio (~0.004). Se rediseñó: supervivencia como **filtro**,
+ alpha vs buy&hold + Sharpe + castigo al drawdown, + evaluación multi-ventana.
Resultado: el `best_fitness` pasó de plano (0.49) a tener **cero y signo con
significado** (oscila ±0.08). El motor ya *medía* bien — pero seguía sin edge.

### A.2 Barrido de timeframe y features (walk-forward, ~14-19 folds)

| # | Configuración | Pasan | Sharpe medio | Alpha en alcistas |
|---|---|---|---|---|
| 1 | **5m**, 12 features | 0/19 | −0.05 | −18.7% |
| 2 | **1h**, 12 features | 4/14 | −0.03 | −15.1% |
| 3 | 1h, +tendencia larga (16 feat) | 4/14 | −0.01 | −13.6% |
| 4 | 1h, +volumen y rango (19 feat) + warm-up | 3/14 | −0.03 | −19.4% |
| 5 | 1h, +funding rates (21 feat) | 1/14 | −0.04 | −20.1% |

**Lecturas:**
- **El timeframe importó:** pasar de 5m→1h ayudó de verdad. A 5m, la comisión
  (0.1%/trade) **asfixiaba** la estrategia (el campeón quedaba muerto en cash).
- **Las features no.** Tendencia larga dio una mejora marginal; volumen, rango y
  hasta el funding como feature **empeoraron** (sobreajuste puro).
- **La firma constante en TODOS los folds:** alpha **negativo en alcistas**
  (se pierde las subidas) y **positivo en bajistas** (gana solo por estar en
  cash). Es **defensividad, no habilidad.** El Sharpe nunca cruzó a positivo.

**Conclusión Fase A:** el paradigma "single-asset, long-only, predecir dirección"
está **agotado**. Añadir features fiablemente sobreajusta. El muro es el mercado
eficiente.

---

## 3. Fase B — Cambiar de paradigma: selección cross-sectional

Hipótesis: en vez de cronometrar UN activo (lo más difícil), **rankear muchas
monedas** y quedarse con las fuertes. El ranking relativo es más estable que la
predicción absoluta.

Sonda: universo de 16 monedas líquidas, long equal-weight **top-5 por momentum**,
rebalanceo semanal, con costes. Comparado contra "tener todo el universo
equiponderado" y contra "BTC buy&hold".

**Barrido honesto de lookback (sin cherry-picking):**

| Lookback | Cross-sectional top-5 | Equal-weight univ. | BTC buy&hold |
|---|---|---|---|
| 30 | −20.1% / Sharpe 0.11 | −8.8% / 0.24 | +4.9% / 0.28 |
| **60** | **+7.0% / 0.35** | −16.7% / 0.16 | +4.9% / 0.28 |
| 90 | −13.0% / 0.17 | −20.4% / 0.12 | +4.9% / 0.28 |
| 180 | −58.3% / −0.59 | −61.2% / −0.61 | +4.9% / 0.28 |

**Lectura:** el único config bueno (lookback=60) está **rodeado de malos** →
inconsistente entre vecinos = **ruido, no edge**. Si fuera real, 30/60/90 darían
algo parecido. Quedarse con el 60 sería *cherry-picking*. Y aun así, su 66% de
drawdown es peor que tener BTC (51%). **BTC buy&hold, lo más tonto, le gana a casi
todo con menos riesgo.**

**Conclusión Fase B:** la selección cross-sectional por momentum tampoco tiene
edge robusto.

---

## 4. Fase C — El edge real: carry (delta-neutral)

Tras descartar **predecir** y **seleccionar**, el único camino que queda es un
edge **estructural/mecánico** que no adivina nada: el **carry**. Long spot +
short perpetuo del mismo notional → el precio se cancela; cobras el **funding**
cada 8h (los longs pagan a los shorts cuando es positivo).

### C.1 Carry de BTC (720 días, 2160 pagos de funding)

| | Retorno total | Anualizado | Sharpe | maxDD |
|---|---|---|---|---|
| **Carry BTC (1x, neto)** | **+9.6%** | +4.7% | +24.69 | **0.4%** |
| BTC buy&hold | +4.9% | — | +0.28 | 51.2% |

El funding fue **positivo el 80% del tiempo** (~+4.8%/año bruto). El carry ganó
más que BTC **con drawdown prácticamente nulo**, sin predecir nada.

> ⚠️ El **Sharpe +24.69 NO es realista** — es artefacto de un modelo demasiado
> limpio (asume neutralidad perfecta, sin *basis risk* ni coste de rebalancear el
> hedge). El número real en vivo será **mucho menor**. Lo sólido es el **retorno
> modesto (~5%/año a 1x) con riesgo mínimo**.

### C.2 Carry multi-moneda (¿diversificar sube el yield?)

| Estrategia | Anualizado | Sharpe | maxDD |
|---|---|---|---|
| Cesta equiponderada (16 perps) | +1.6% | +7.25 | 1.7% |
| Top-5 por funding | **−47.0%** | +26.36 | **71%** |
| **Solo BTC** | **+4.7%** | +24.69 | 0.4% |

**Dos lecciones grandes:**
1. **Diversificar HUNDIÓ el yield** (cesta +1.6% vs BTC +4.7%). El funding de BTC
   fue el más fiable; los alts lo arrastraron hacia abajo. Contraintuitivo: aquí,
   más monedas = peor.
2. **Perseguir el funding alto es una trampa** (−47%/año). El top-k te concentra
   en monedas *frothy* justo antes de que su funding se voltee negativo. Su
   **Sharpe +26 con retorno −47%** es la prueba perfecta de que **el Sharpe es
   ciego a las colas gordas**.

**Conclusión Fase C:** el carry de **solo BTC** es el ganador — y, felizmente, el
**más simple**. El bot de producción no necesita gestionar un universo.

---

## 5. Conclusión final

> **Predecir / seleccionar a partir del precio → no hay edge** (confirmado por
> ~7 vías independientes: 5 de features, 1 cross-sectional con barrido, costes).
> **Carry de BTC → renta real**, modesta (~5%/año a 1x), con riesgo mínimo, sin
> adivinar nada. Es la respuesta, y es ideal para un sistema **autónomo basado en
> reglas** que corra solo sin supervisión — la meta original del proyecto.

**Decisión:** construir el **bot de carry de producción** (long-spot BTC +
short-perp, cobrar funding, controles de riesgo). Ver el diseño en
[`CARRY_BOT.md`](./CARRY_BOT.md) cuando exista.

**Expectativa honesta:** ~5%/año a 1x (con apalancamiento sube retorno *y* riesgo
de liquidación). El backtest subestima *basis risk*, coste de rebalanceo del
hedge y riesgo de exchange; el Sharpe real estará muy por debajo de los números
del backtest. Pero **gana dinero de verdad y con poco riesgo** — que es más de lo
que logró cualquier enfoque predictivo.

---

## 6. Reproducir los experimentos

Todos son modos del binario, activados por variable de entorno; **no tocan el
daemon** y terminan al acabar. Compilar con `cargo build --release` y correr con
las variables delante. Ejemplos:

```bash
# Walk-forward multi-régimen (Fase A)
MCPATO_WF_CHECK=true MCPATO_INTERVAL=1h MCPATO_WF_DAYS=960 \
MCPATO_WF_TRAIN_DAYS=20 MCPATO_WF_TEST_DAYS=4 MCPATO_EVAL_WINDOWS=4 \
./target/release/mcpato

# Cross-sectional momentum (Fase B)
MCPATO_XS_CHECK=true MCPATO_XS_DAYS=720 MCPATO_XS_LOOKBACK=30 \
MCPATO_XS_TOPK=5 ./target/release/mcpato

# Carry de BTC (Fase C)
MCPATO_CARRY_CHECK=true MCPATO_CARRY_DAYS=720 ./target/release/mcpato

# Carry multi-moneda (Fase C)
MCPATO_CARRY_MULTI_CHECK=true MCPATO_CARRY_DAYS=720 MCPATO_CARRY_TOPK=5 \
./target/release/mcpato

# Medición OOS rápida (single-asset)
MCPATO_OOS_CHECK=true MCPATO_OOS_DAYS=180 ./target/release/mcpato
```

Módulos correspondientes: `src/walkforward.rs`, `src/crossmomentum.rs`,
`src/carry.rs`, `src/oos.rs`. El pipeline de funding (reutilizado por carry) está
en `src/rest_binance.rs` (`fetch_funding_rates`, `merge_funding`).
