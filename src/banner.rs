//! Banner de arranque. Imprime el logo (pato ASCII) y una cabecera con estilo
//! sobrio de aplicación financiera.

use crate::config::Config;

const LOGO: &str = include_str!("logo.txt");

// Colores ANSI (256). Se desactivan si la salida no es una TTY o NO_COLOR está set.
const GOLD: &str = "\x1b[38;5;220m";
const DIM_GOLD: &str = "\x1b[38;5;136m";
const CYAN: &str = "\x1b[38;5;37m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

fn colors_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

fn paint(code: &str, text: &str) -> String {
    if colors_enabled() {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

/// Imprime el banner completo: logo + cabecera + parámetros de arranque.
pub fn print_startup(cfg: &Config) {
    let version = env!("CARGO_PKG_VERSION");

    println!();
    println!("{}", paint(GOLD, LOGO.trim_end_matches('\n')));
    println!();
    println!(
        "{}",
        paint(BOLD, "   M C P A T O").to_string() + &paint(DIM_GOLD, &format!("   v{version}"))
    );
    println!(
        "   {}",
        paint(DIM, "Organismo financiero evolutivo · paper trading")
    );
    println!("   {}", paint(DIM, "────────────────────────────────────────────"));

    let max_gens = cfg
        .max_generations
        .map(|n| n.to_string())
        .unwrap_or_else(|| "unbounded".to_string());

    print_row("Instrumento", &format!("{} ({})", cfg.instrument_name, cfg.symbol));
    print_row("Timeframe", &cfg.interval);
    print_row("Base de datos", &cfg.db_path);
    print_row("Capital inicial", &format!("{:.4}", cfg.initial_capital));
    print_row("Survival fee", &format!("{:.6}", cfg.survival_rate));
    print_row("TTL de señal", &format!("{}s", cfg.signal_ttl_secs));
    print_row("Máx. generaciones", &max_gens);
    println!();
}

fn print_row(label: &str, value: &str) {
    println!(
        "   {}  {}",
        paint(CYAN, &format!("{label:<18}")),
        paint(BOLD, value)
    );
}
