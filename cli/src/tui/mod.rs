mod widgets;

use std::io::stdout;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Frame, Terminal,
};
use thinkflow_core::ThinkFlowPolicy;

use crate::app::{AppState, CoreViewMode};
use crate::cpu::CoreKind;

// ============================================================================
// Responsive TUI Layout Orchestration
// ============================================================================

pub(crate) fn draw_ui(frame: &mut Frame, state: &AppState) {
    let size = frame.area();

    let p_cnt = state
        .core_metrics
        .iter()
        .filter(|c| c.kind == CoreKind::Performance)
        .count();
    let e_cnt = state
        .core_metrics
        .iter()
        .filter(|c| c.kind == CoreKind::Efficiency)
        .count();
    let max_group = p_cnt.max(e_cnt).max(1);
    let expanded_cores_h = (max_group as u16) + 2;
    let compact_cores_h = (((max_group + 1) / 2) as u16) + 2;

    // Fixed height of Header (3) + Telemetry/Fan (9) + ACPI/EC/HW (5) + Nav (1) = 18 rows
    let fixed_h: u16 = 18;
    let (show_cores, compact_cores, show_spark) = match state.core_view_mode {
        CoreViewMode::TrendOnly => (false, false, true),
        CoreViewMode::Expanded => {
            let with_spark = size.height >= fixed_h + expanded_cores_h + 5;
            (true, false, with_spark)
        }
        CoreViewMode::Auto => {
            if size.height >= fixed_h + expanded_cores_h + 5 {
                (true, false, true)
            } else if size.height >= fixed_h + compact_cores_h + 5 {
                (true, true, true)
            } else {
                (true, true, false)
            }
        }
    };

    let cores_target_h = if compact_cores {
        compact_cores_h
    } else {
        expanded_cores_h
    };

    let mut constraints = vec![
        Constraint::Length(3), // 0: Header Banner
        Constraint::Length(9), // 1: CPU Telemetry & EC Fan Cards
    ];
    if show_cores && show_spark {
        constraints.push(Constraint::Length(cores_target_h)); // 2: Per-Core P/E Cards
        constraints.push(Constraint::Min(5)); // 3: Sparkline Trend
    } else if show_cores {
        constraints.push(Constraint::Min(cores_target_h.min(6))); // 2: Per-Core P/E Cards
    } else {
        constraints.push(Constraint::Min(5)); // 2: Sparkline Trend
    }
    constraints.push(Constraint::Length(5)); // Bottom 3-Column ACPI/EC Cards
    constraints.push(Constraint::Length(1)); // Key Navigation Bar

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(size);

    widgets::draw_header_banner(frame, chunks[0], state);
    widgets::draw_telemetry_and_fan_row(frame, chunks[1], state);

    let bot_idx = if show_cores && show_spark {
        widgets::draw_cores_section(frame, chunks[2], state, compact_cores);
        widgets::draw_sparkline_section(frame, chunks[3], state);
        4
    } else if show_cores {
        widgets::draw_cores_section(frame, chunks[2], state, compact_cores);
        3
    } else {
        widgets::draw_sparkline_section(frame, chunks[2], state);
        3
    };

    widgets::draw_bottom_diagnostic_cards(frame, chunks[bot_idx], state);
    widgets::draw_nav_bar(frame, chunks[bot_idx + 1], state);
}

// ============================================================================
// Interactive TUI Event Loop
// ============================================================================

pub(crate) fn run_tui() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = AppState::new();
    let mut last_sample = Instant::now();
    let mut initial_warmup = true;

    loop {
        terminal.draw(|f| draw_ui(f, &state))?;

        let poll_timeout = if initial_warmup {
            Duration::from_millis(180)
        } else {
            Duration::from_millis(250)
        };

        // Handle keyboard input events
        if event::poll(poll_timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                            break;
                        }
                        KeyCode::Char('m') | KeyCode::Char('M') => {
                            state.cycle_policy();
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            state.set_policy(ThinkFlowPolicy::Auto);
                            state.set_status("Restored Default (Auto) mode");
                        }
                        KeyCode::Char('f') | KeyCode::Char('F') => {
                            state.set_policy(ThinkFlowPolicy::Performance);
                            state.set_status("Forced Performance / Beast mode!");
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            state.cycle_core_view();
                        }
                        KeyCode::Char('r') | KeyCode::Char('R') => {
                            state.reset_counters();
                        }
                        _ => {}
                    }
                }
            }
        }

        // Fast first delta (~180ms) for immediate per-core % & GHz, then 1s cadence
        let sample_interval = if initial_warmup {
            Duration::from_millis(180)
        } else {
            Duration::from_millis(1000)
        };
        if last_sample.elapsed() >= sample_interval {
            state.update_telemetry();
            last_sample = Instant::now();
            initial_warmup = false;

            // Clear status message back to NOMINAL after 4 seconds
            if state.status_time.elapsed() >= Duration::from_secs(4)
                && state.status_message != "NOMINAL"
            {
                state.status_message = "NOMINAL".into();
            }
        }
    }

    // Clean up and restore terminal state
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
