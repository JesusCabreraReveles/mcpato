//! Lógica de "señales" de acción (compra/venta) derivadas de la postura del campeón.
//!
//! Una señal es un evento discreto: nace solo cuando el campeón cambia de
//! postura (FLAT -> LONG => COMPRAR, LONG -> FLAT => VENDER). Esto evita el
//! spam de los rebalanceos fraccionados por vela y hace el flujo confiable.

/// Postura del organismo respecto al mercado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    /// Sin posición (fuera del mercado).
    Flat,
    /// Comprado (long).
    Long,
}

/// Acciones posibles de una señal (almacenadas tal cual en SQLite).
pub const ACTION_BUY: &str = "BUY";
pub const ACTION_SELL: &str = "SELL";

/// Estados del ciclo de vida de una señal (almacenados tal cual en SQLite).
pub const STATUS_PENDING: &str = "PENDING";
/// Usado por el dashboard (Fase 3) para marcar una señal como ejecutada.
#[allow(dead_code)]
pub const STATUS_EXECUTED: &str = "EXECUTED";
/// Estado al que pasa una señal vencida (la fija `expire_stale_signals`).
#[allow(dead_code)]
pub const STATUS_EXPIRED: &str = "EXPIRED";

/// Deriva la postura a partir del valor de la posición actual.
///
/// `epsilon` es el umbral mínimo (en moneda de cotización) por debajo del cual
/// se considera que no hay posición efectiva.
pub fn stance_from_position(position_value: f64, epsilon: f64) -> Stance {
    if position_value > epsilon.max(0.0) {
        Stance::Long
    } else {
        Stance::Flat
    }
}

/// Calcula la postura deseada por el campeón según su `signal` y el `threshold`.
///
/// - `signal > threshold`  -> quiere estar LONG
/// - `signal < -threshold` -> quiere estar FLAT
/// - en la banda intermedia -> mantiene la postura previa (`prev`)
pub fn desired_stance(prev: Stance, signal: f64, threshold: f64) -> Stance {
    let t = threshold.abs();
    if signal > t {
        Stance::Long
    } else if signal < -t {
        Stance::Flat
    } else {
        prev
    }
}

/// Devuelve la acción a emitir cuando se pasa de `prev` a `next`, o `None` si no
/// hubo transición (misma postura => no se emite señal).
pub fn transition_action(prev: Stance, next: Stance) -> Option<&'static str> {
    match (prev, next) {
        (Stance::Flat, Stance::Long) => Some(ACTION_BUY),
        (Stance::Long, Stance::Flat) => Some(ACTION_SELL),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stance_from_position_respects_epsilon() {
        assert_eq!(stance_from_position(0.0, 1e-9), Stance::Flat);
        assert_eq!(stance_from_position(1e-12, 1e-9), Stance::Flat);
        assert_eq!(stance_from_position(5.0, 1e-9), Stance::Long);
    }

    #[test]
    fn desired_stance_crosses_thresholds() {
        // Por encima del umbral -> LONG
        assert_eq!(desired_stance(Stance::Flat, 0.5, 0.2), Stance::Long);
        // Por debajo del umbral negativo -> FLAT
        assert_eq!(desired_stance(Stance::Long, -0.5, 0.2), Stance::Flat);
    }

    #[test]
    fn desired_stance_holds_in_neutral_band() {
        // En la banda intermedia mantiene la postura previa
        assert_eq!(desired_stance(Stance::Long, 0.1, 0.2), Stance::Long);
        assert_eq!(desired_stance(Stance::Flat, -0.1, 0.2), Stance::Flat);
    }

    #[test]
    fn transition_emits_only_on_change() {
        assert_eq!(transition_action(Stance::Flat, Stance::Long), Some(ACTION_BUY));
        assert_eq!(transition_action(Stance::Long, Stance::Flat), Some(ACTION_SELL));
        assert_eq!(transition_action(Stance::Flat, Stance::Flat), None);
        assert_eq!(transition_action(Stance::Long, Stance::Long), None);
    }
}
