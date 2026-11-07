use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Sparkline},
    Frame,
};

use crate::app::AppState;
use crate::cpu::{CoreKind, CoreLiveMetric};

// ============================================================================
// Block Gauge & Visual Color Helpers
// ============================================================================

pub(super) fn make_bar(ratio: f64, width: usize, color: Color) -> Line<'static> {
    let clamped = ratio.clamp(0.0, 1.0);
    let filled_len = (clamped * width as f64).round() as usize;
    let empty_len = width.saturating_sub(filled_len);

    let filled = "█".repeat(filled_len);
    let empty = "░".repeat(empty_len);

    Line::from(vec![
        Span::raw("["),
        Span::styled(
            filled,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(empty, Style::default().fg(Color::DarkGray)),
        Span::raw("]"),
    ])
}

pub(super) fn temp_color(c: u32) -> Color {
    if c >= 85 {
        Color::LightRed
    } else if c >= 70 {
        Color::LightYellow
    } else {
        Color::Cyan
    }
}

pub(super) fn power_color(w: f64) -> Color {
    if w >= 45.0 {
        Color::LightRed
    } else if w >= 25.0 {
        Color::LightYellow
    } else {
        Color::Green
    }
}

pub(super) fn load_color(pct: f64, kind: CoreKind) -> Color {
    if pct >= 85.0 {
        Color::LightRed
    } else if pct >= 55.0 {
        Color::LightYellow
    } else {
        match kind {
            CoreKind::Performance => Color::LightMagenta,
            CoreKind::Efficiency => Color::Cyan,
        }
    }
}

// ============================================================================
// 1. Top Header Banner
// ============================================================================

pub(super) fn draw_header_banner(frame: &mut Frame, area: Rect, state: &AppState) {
    let status_tag = if state.telemetry.loaded {
        Span::styled(
            " [ ACTIVE: LenovoThinkFlow.kext ] ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " [ STANDBY: Direct EC Mode ] ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    };

    let header_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Blue))
        .title(Line::from(vec![
            Span::styled(
                concat!(" LenovoThinkFlow v", env!("CARGO_PKG_VERSION"), " "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            status_tag,
        ]));

    let header_line = Line::from(vec![
        Span::raw(" Host: "),
        Span::styled(
            &state.hostname,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ CPU: "),
        Span::styled(
            &state.cpu_topo_summary,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ Ticks: "),
        Span::styled(
            format!("{}", state.telemetry.total_ticks),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" │ Hyst: "),
        Span::styled(
            format!("{}t", state.telemetry.hysteresis_counter),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let header_p = Paragraph::new(header_line).block(header_block);
    frame.render_widget(header_p, area);
}

// ============================================================================
// 2. CPU Telemetry & EC Fan / Policy Row
// ============================================================================

pub(super) fn draw_telemetry_and_fan_row(frame: &mut Frame, area: Rect, state: &AppState) {
    let mid_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Left Card: CPU Telemetry & Throttle
    let left_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " CPU Telemetry & Throttle ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let t_c = state.telemetry.cpu_temp_c;
    let tjmax = state.telemetry.tjmax_c.max(80);
    let t_pct = ((t_c as f64 / tjmax as f64) * 100.0).round().min(100.0) as u32;
    let t_color = temp_color(t_c);

    let pkg_w = state.telemetry.pkg_power_w;
    let pl1_ref_w = if state.telemetry.effective_pl1_w > 1.0 {
        state.telemetry.effective_pl1_w
    } else if state.telemetry.target_pl1_w > 1.0 {
        state.telemetry.target_pl1_w
    } else {
        65.0
    };
    let p_pct = ((pkg_w / pl1_ref_w) * 100.0).round().min(100.0) as u32;
    let p_color = power_color(pkg_w);

    let bar_width = 30;
    let t_bar = make_bar(t_c as f64 / tjmax as f64, bar_width, t_color);
    let p_bar = make_bar(pkg_w / pl1_ref_w, bar_width, p_color);

    let power_suffix = if state.telemetry.target_pl1_w > 0.5 {
        Span::styled(
            format!(" / {:.0}W Cap", state.telemetry.target_pl1_w),
            Style::default().fg(Color::Cyan),
        )
    } else if state.telemetry.effective_pl1_w > 0.5 {
        Span::styled(
            format!(" (PL1 {:.0}W)", state.telemetry.effective_pl1_w),
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::styled(" (Package)", Style::default().fg(Color::DarkGray))
    };

    let throttle_span = if state.telemetry.is_throttling {
        Span::styled(
            "[YES]",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("[NO]", Style::default().fg(Color::Green))
    };

    let cpu_lines = vec![
        Line::from(vec![
            Span::raw(" Temp:  "),
            Span::styled(
                format!("{}°C", t_c),
                Style::default().fg(t_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" / {}°C TjMax", tjmax),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            t_bar.spans[0].clone(),
            t_bar.spans[1].clone(),
            t_bar.spans[2].clone(),
            t_bar.spans[3].clone(),
            Span::styled(format!("  {:>2}%", t_pct), Style::default().fg(t_color)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::raw(" Power: "),
            Span::styled(
                format!("{:.2} W", pkg_w),
                Style::default().fg(p_color).add_modifier(Modifier::BOLD),
            ),
            power_suffix,
        ]),
        Line::from(vec![
            p_bar.spans[0].clone(),
            p_bar.spans[1].clone(),
            p_bar.spans[2].clone(),
            p_bar.spans[3].clone(),
            Span::styled(format!("  {:>2}%", p_pct), Style::default().fg(p_color)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::raw(" HWP:   Max "),
            Span::styled(
                format!("{}x", state.telemetry.hwp_max_ratio),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" │ EPP: "),
            Span::styled(
                format!("0x{:02X}", state.telemetry.hwp_epp),
                Style::default().fg(Color::White),
            ),
            Span::raw(" │ Throt: "),
            throttle_span,
        ]),
    ];

    let cpu_p = Paragraph::new(cpu_lines).block(left_block);
    frame.render_widget(cpu_p, mid_chunks[0]);

    // Right Card: EC Fan & Policy Control
    let right_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green))
        .title(Span::styled(
            " EC Fan & Policy Control ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));

    let mode_str = match state.telemetry.policy_mode {
        1 => "[ QUIET ] Fan Off + 12W Cap",
        2 => "[ BALANCE ] Manual Force",
        3 => "[ PERFORMANCE ] Manual Force",
        4 => "[ COOL ] Auto Fan + 15W Cap",
        _ => "[ AUTO ] Adaptive Scheduler",
    };

    let profile_badge = match state.telemetry.active_profile {
        1 => Span::styled(
            "[ QUIET / SILENT ]",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        2 => Span::styled(
            "[ BALANCE / AUTO ]",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        3 => Span::styled(
            "[ BEAST / PERFORMANCE ]",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        4 => {
            if state.telemetry.ec_fan_target == 3 {
                Span::styled(
                    "[ COOL / FAN BOOST ]",
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(
                    "[ COOL / BREEZY ]",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            }
        }
        _ => Span::styled("[ UNKNOWN ]", Style::default().fg(Color::Yellow)),
    };

    let raw_fan = state.telemetry.ec_fan_raw;
    let fan_pct = raw_fan.min(100);
    let fan_bar = make_bar(fan_pct as f64 / 100.0, 24, Color::Cyan);

    let lpm_status = if state.telemetry.is_lpm_active {
        Span::styled(
            "ON  (machdep.xcpm.lpm = 1)",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            "OFF (machdep.xcpm.lpm = 0)",
            Style::default().fg(Color::DarkGray),
        )
    };

    let fan_lines = vec![
        Line::from(vec![
            Span::raw(" Mode:      "),
            Span::styled(
                mode_str,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![Span::raw(" Profile:   "), profile_badge]),
        Line::raw(""),
        Line::from(vec![
            Span::raw(" Fan Speed: "),
            Span::styled(
                format!("0x{:02X} ({}%)", raw_fan, fan_pct),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            fan_bar.spans[0].clone(),
            fan_bar.spans[1].clone(),
            fan_bar.spans[2].clone(),
            fan_bar.spans[3].clone(),
            Span::styled(
                format!(" {:>4} RPM", state.telemetry.ec_fan_rpm),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![Span::raw(" Low Power: "), lpm_status]),
    ];

    let fan_p = Paragraph::new(fan_lines).block(right_block);
    frame.render_widget(fan_p, mid_chunks[1]);
}

// ============================================================================
// 3. Per-Core P/E Cards & Sparkline Trend Sections
// ============================================================================

fn format_compact_core_spans(m: &CoreLiveMetric, bar_w: usize) -> Vec<Span<'static>> {
    let pct = m.usage_pct.clamp(0.0, 100.0);
    let filled = ((pct / 100.0) * bar_w as f64).round() as usize;
    let empty = bar_w.saturating_sub(filled);
    let c_color = load_color(pct, m.kind);
    let badge_color = match m.kind {
        CoreKind::Performance => Color::Magenta,
        CoreKind::Efficiency => Color::Cyan,
    };

    let freq_str = if m.freq_ghz > 0.05 {
        format!(" {:>3.1}G", m.freq_ghz)
    } else {
        "  -- ".to_string()
    };

    vec![
        Span::styled(
            format!("{:<2} ", m.label),
            Style::default().fg(badge_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "█".repeat(filled),
            Style::default().fg(c_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("░".repeat(empty), Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" {:>3.0}%", pct), Style::default().fg(c_color)),
        Span::styled(freq_str, Style::default().fg(Color::White)),
    ]
}

fn format_expanded_core_line(m: &CoreLiveMetric, inner_w: usize) -> Line<'static> {
    let pct = m.usage_pct.clamp(0.0, 100.0);
    let c_color = load_color(pct, m.kind);
    let badge_color = match m.kind {
        CoreKind::Performance => Color::Magenta,
        CoreKind::Efficiency => Color::Cyan,
    };

    let base_extra = inner_w.saturating_sub(38).min(18);
    let bar_w = 8 + base_extra;
    let filled = ((pct / 100.0) * bar_w as f64).round() as usize;
    let empty = bar_w.saturating_sub(filled);

    let freq_str = if m.freq_ghz > 0.05 {
        format!(" {:>4.2}G", m.freq_ghz)
    } else {
        "  --  ".to_string()
    };

    let thread_note = if m.thread_usages.len() >= 2 {
        format!(" T:{:>3.0}/{:<3.0}", m.thread_usages[0], m.thread_usages[1])
    } else if let Some(&tid) = m.threads.first() {
        format!(" (CPU{:>2})", tid)
    } else {
        String::new()
    };

    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{:<2}", m.label),
            Style::default().fg(badge_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ["),
        Span::styled(
            "█".repeat(filled),
            Style::default().fg(c_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("░".repeat(empty), Style::default().fg(Color::DarkGray)),
        Span::raw("]"),
        Span::styled(
            format!(" {:>3.0}%", pct),
            Style::default().fg(c_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(freq_str, Style::default().fg(Color::White)),
        Span::styled(thread_note, Style::default().fg(Color::DarkGray)),
    ])
}

fn draw_core_group_card(
    frame: &mut Frame,
    area: Rect,
    title: Line<'static>,
    border_color: Color,
    cores: &[&CoreLiveMetric],
    compact: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(title);

    let inner = block.inner(area);
    let inner_w = inner.width as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();
    if compact {
        // 2 physical cores per row: fits 6P in 3 rows and 8E in 4 rows cleanly on 80x24
        let extra_per_col = inner_w.saturating_sub(38) / 2;
        let bar_w = (5 + extra_per_col).min(12);
        for pair in cores.chunks(2) {
            let mut spans = vec![Span::raw(" ")];
            spans.extend(format_compact_core_spans(pair[0], bar_w));
            if let Some(second) = pair.get(1) {
                spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
                spans.extend(format_compact_core_spans(second, bar_w));
            }
            lines.push(Line::from(spans));
        }
    } else {
        for c in cores {
            lines.push(format_expanded_core_line(c, inner_w));
        }
    }

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

pub(super) fn draw_cores_section(frame: &mut Frame, area: Rect, state: &AppState, compact: bool) {
    let p_cores: Vec<&CoreLiveMetric> = state
        .core_metrics
        .iter()
        .filter(|c| c.kind == CoreKind::Performance)
        .collect();
    let e_cores: Vec<&CoreLiveMetric> = state
        .core_metrics
        .iter()
        .filter(|c| c.kind == CoreKind::Efficiency)
        .collect();

    let p_threads: usize = p_cores.iter().map(|c| c.threads.len()).sum();
    let e_threads: usize = e_cores.iter().map(|c| c.threads.len()).sum();

    if e_cores.is_empty() {
        let title = Line::from(vec![Span::styled(
            format!(
                " CPU Cores ({}C/{}T · {:.2}GHz) ",
                p_cores.len(),
                p_threads,
                state.telemetry.p_freq_ghz
            ),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )]);
        draw_core_group_card(frame, area, title, Color::Magenta, &p_cores, compact);
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let p_title = Line::from(vec![
        Span::styled(
            " P-Cores ",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("[P·性能] ", Style::default().fg(Color::Magenta)),
        Span::styled(
            format!(
                "{}C/{}T · {:.2}GHz ",
                p_cores.len(),
                p_threads,
                state.telemetry.p_freq_ghz
            ),
            Style::default().fg(Color::White),
        ),
    ]);

    let e_title = Line::from(vec![
        Span::styled(
            " E-Cores ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("[E·能效] ", Style::default().fg(Color::LightCyan)),
        Span::styled(
            format!(
                "{}C/{}T · {:.2}GHz ",
                e_cores.len(),
                e_threads,
                state.telemetry.e_freq_ghz
            ),
            Style::default().fg(Color::White),
        ),
    ]);

    draw_core_group_card(frame, cols[0], p_title, Color::Magenta, &p_cores, compact);
    draw_core_group_card(frame, cols[1], e_title, Color::Cyan, &e_cores, compact);
}

pub(super) fn draw_sparkline_section(frame: &mut Frame, area: Rect, state: &AppState) {
    let trend_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            " Temperature Trend (Last 60s) ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));

    let spark_inner = trend_block.inner(area);
    frame.render_widget(trend_block, area);

    let spark_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(8), Constraint::Min(10)])
        .split(spark_inner);

    let all_labels = [
        (" 85°C ┤", Color::LightRed),
        (" 80°C ┤", Color::LightYellow),
        (" 75°C ┤", Color::Cyan),
        (" 70°C ┤", Color::Blue),
        (" 65°C ┤", Color::DarkGray),
    ];
    let rows_avail = (spark_inner.height as usize).max(1);
    let y_axis: Vec<Line<'static>> = if rows_avail >= 5 {
        all_labels
            .iter()
            .map(|(lbl, col)| Line::from(Span::styled(*lbl, Style::default().fg(*col))))
            .collect()
    } else if rows_avail == 4 {
        [all_labels[0], all_labels[1], all_labels[2], all_labels[4]]
            .iter()
            .map(|(lbl, col)| Line::from(Span::styled(*lbl, Style::default().fg(*col))))
            .collect()
    } else if rows_avail == 3 {
        [all_labels[0], all_labels[2], all_labels[4]]
            .iter()
            .map(|(lbl, col)| Line::from(Span::styled(*lbl, Style::default().fg(*col))))
            .collect()
    } else {
        all_labels
            .iter()
            .take(rows_avail)
            .map(|(lbl, col)| Line::from(Span::styled(*lbl, Style::default().fg(*col))))
            .collect()
    };
    let y_p = Paragraph::new(y_axis);
    frame.render_widget(y_p, spark_layout[0]);

    let data_vec: Vec<u64> = state.temp_history.iter().copied().collect();
    let sparkline = Sparkline::default()
        .data(&data_vec)
        .max(100)
        .style(Style::default().fg(temp_color(state.telemetry.cpu_temp_c)));
    frame.render_widget(sparkline, spark_layout[1]);
}

// ============================================================================
// 4. Bottom 3-Column Diagnostic Cards & Key Navigation Bar
// ============================================================================

pub(super) fn draw_bottom_diagnostic_cards(frame: &mut Frame, area: Rect, state: &AppState) {
    let bot_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(area);

    // Col 1: ACPI Nodes
    let acpi_b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Blue))
        .title(Span::styled(
            " ACPI Nodes ",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ));

    let node_dot = |ok: bool| {
        if ok {
            Span::styled("● OK", Style::default().fg(Color::Green))
        } else {
            Span::styled("● Missing", Style::default().fg(Color::Red))
        }
    };

    let acpi_lines = vec![
        Line::from(vec![
            Span::raw(" VPC0 (VPC2004)    "),
            node_dot(state.telemetry.vpc0_ok),
        ]),
        Line::from(vec![
            Span::raw(" TZS0 / TZS1       "),
            node_dot(state.telemetry.tzs0_ok),
        ]),
        Line::from(vec![
            Span::raw(" IETM Driver       "),
            node_dot(state.telemetry.ietm_ok),
        ]),
    ];
    let acpi_p = Paragraph::new(acpi_lines).block(acpi_b);
    frame.render_widget(acpi_p, bot_cols[0]);

    // Col 2: EC Raw Registers
    let ec_b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(
            " EC Raw Registers ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));

    let ec_lines = vec![
        Line::from(vec![
            Span::raw(" THS0 (CPU Die)      "),
            Span::styled(
                format!("{}°C", state.telemetry.ec_temp0_c),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw(" THS1 (GPU/Skin)     "),
            Span::styled(
                format!("{}°C", state.telemetry.ec_temp1_c),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::raw(" PFMM / ITSM         "),
            Span::styled(
                format!("{} / {}", state.telemetry.ec_pfmm, state.telemetry.ec_itsm),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    let ec_p = Paragraph::new(ec_lines).block(ec_b);
    frame.render_widget(ec_p, bot_cols[1]);

    // Col 3: Hardware Flags
    let hw_b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Hardware Flags ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));

    let ssdt_badge = if state.telemetry.ssdt_present {
        Span::styled(
            "LOADED",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("DIRECT-EC", Style::default().fg(Color::Yellow))
    };

    let lxos_active = state.telemetry.ec_lxos == 1 || (state.telemetry.ec_lxos & 0x40) != 0;
    let lxos_color = if lxos_active { Color::Green } else { Color::Yellow };

    let hw_lines = vec![
        Line::from(vec![
            Span::raw(" LXOS Status         "),
            Span::styled(
                format!("0x{:02X}", state.telemetry.ec_lxos),
                Style::default().fg(lxos_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw(" TPA1 Flag           "),
            Span::styled(
                format!("{}", state.telemetry.ec_tpa1),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![Span::raw(" SSDT-THINKFLOW      "), ssdt_badge]),
    ];
    let hw_p = Paragraph::new(hw_lines).block(hw_b);
    frame.render_widget(hw_p, bot_cols[2]);
}

pub(super) fn draw_nav_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let nav_line = Line::from(vec![
        Span::styled(
            " [Q]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Quit │ "),
        Span::styled(
            "[M]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Mode │ "),
        Span::styled(
            "[D]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Auto │ "),
        Span::styled(
            "[F]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Max │ "),
        Span::styled(
            "[C]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Cores │ "),
        Span::styled(
            "[R]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Reset │ "),
        Span::styled(
            &state.status_message,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let nav_p = Paragraph::new(nav_line);
    frame.render_widget(nav_p, area);
}
