mod app;
mod commands;
mod cpu;
mod ffi;
mod telemetry;
mod tui;

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");

    match cmd {
        "" | "tui" | "monitor" | "watch" => {
            if let Err(e) = tui::run_tui() {
                eprintln!("Error running TUI: {}", e);
            }
        }
        "status" | "info" => commands::print_status(),
        "cores" | "cpu" => commands::print_cores(),
        "mode" | "set" => {
            if let Some(m) = args.get(2) {
                commands::handle_mode_command(m);
            } else {
                eprintln!("Usage: thinkflow mode <auto|cool|quiet|balance|performance>");
                std::process::exit(1);
            }
        }
        "-h" | "--help" | "help" => commands::print_help(),
        _ => {
            commands::print_help();
            std::process::exit(1);
        }
    }
}
