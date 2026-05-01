//! `solvela watch` — live ratatui dashboard for spend, models, payments, health.
//!
//! Mirrors Franklin's `franklin insights` panel pattern (see
//! https://github.com/BlockRunAI/Franklin `src/panel/server.ts`).
//!
//! Polls two gateway endpoints on an interval and renders a 2x2 panel grid:
//! - top-left:    spend totals (1d / 7d / total)
//! - top-right:   top models leaderboard
//! - bottom-left: recent payments (placeholder — no endpoint yet)
//! - bottom-right: health (gateway, db, redis, providers, solana_rpc)
//!
//! Keybindings:
//! - `q` or Ctrl-C: quit cleanly
//! - `r`:           force refresh
//! - `↑` / `↓`:     scroll payments panel
//! - `d`:           toggle details modal (full payment record)

use std::io;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap};
use ratatui::{Frame, Terminal};
use serde::Deserialize;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────────────────────
// Public entry point
// ───────────────────────────────────────────────────────────────────────────

/// Run the `solvela watch` TUI against the configured gateway URL.
///
/// `interval_secs` is clamped to a minimum of 1.
pub async fn run(gateway_url: String, interval_secs: u64) -> Result<()> {
    let interval = Duration::from_secs(interval_secs.max(1));
    let admin_token = std::env::var("SOLVELA_ADMIN_TOKEN").ok();

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()
        .context("build reqwest client")?;

    // Set up terminal.
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("init terminal")?;

    let result = run_app(&mut terminal, gateway_url, admin_token, client, interval).await;

    // Always restore terminal, even on error.
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result
}

// ───────────────────────────────────────────────────────────────────────────
// App state and event loop
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct App {
    gateway_url: String,
    snapshot: Option<Snapshot>,
    last_refresh: Option<Instant>,
    payments_scroll: usize,
    details_open: bool,
    error: Option<String>,
}

impl App {
    fn new(gateway_url: String) -> Self {
        Self {
            gateway_url,
            snapshot: None,
            last_refresh: None,
            payments_scroll: 0,
            details_open: false,
            error: None,
        }
    }

    fn scroll_up(&mut self) {
        self.payments_scroll = self.payments_scroll.saturating_sub(1);
    }

    fn scroll_down(&mut self) {
        let max = self
            .snapshot
            .as_ref()
            .map(|s| s.recent_payments.len().saturating_sub(1))
            .unwrap_or(0);
        self.payments_scroll = self.payments_scroll.saturating_add(1).min(max);
    }
}

#[derive(Debug)]
enum AppEvent {
    Refresh(Box<Snapshot>),
    RefreshError(String),
    Tick,
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    gateway_url: String,
    admin_token: Option<String>,
    client: reqwest::Client,
    interval: Duration,
) -> Result<()> {
    let mut app = App::new(gateway_url.clone());
    let (tx, mut rx) = mpsc::channel::<AppEvent>(32);

    // Spawn the polling task. It owns the http client + admin token.
    let poll_tx = tx.clone();
    let poll_url = gateway_url.clone();
    let poll_token = admin_token.clone();
    let poll_client = client.clone();
    let poll_interval = interval;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(poll_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            match fetch_snapshot(&poll_client, &poll_url, poll_token.as_deref()).await {
                Ok(snap) => {
                    if poll_tx
                        .send(AppEvent::Refresh(Box::new(snap)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    if poll_tx
                        .send(AppEvent::RefreshError(format!("{e:#}")))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });

    // Spawn the input task so we can multiplex with poll events.
    let input_tx = tx.clone();
    tokio::spawn(async move {
        let mut input_ticker = tokio::time::interval(Duration::from_millis(100));
        loop {
            input_ticker.tick().await;
            if input_tx.send(AppEvent::Tick).await.is_err() {
                break;
            }
        }
    });

    // Initial draw before any data arrives.
    terminal.draw(|f| draw(f, &app))?;

    while let Some(evt) = rx.recv().await {
        match evt {
            AppEvent::Refresh(snap) => {
                app.snapshot = Some(*snap);
                app.last_refresh = Some(Instant::now());
                app.error = None;
                let max = app
                    .snapshot
                    .as_ref()
                    .map(|s| s.recent_payments.len().saturating_sub(1))
                    .unwrap_or(0);
                if app.payments_scroll > max {
                    app.payments_scroll = max;
                }
            }
            AppEvent::RefreshError(msg) => {
                app.error = Some(msg);
            }
            AppEvent::Tick => {
                // Poll for keyboard events without blocking.
                if event::poll(Duration::from_millis(0))? {
                    if let Event::Key(key) = event::read()? {
                        if let KeyAction::Quit = handle_key(key, &mut app) {
                            return Ok(());
                        }
                        // Force-redraw immediately on input.
                        terminal.draw(|f| draw(f, &app))?;
                        continue;
                    }
                }
            }
        }
        terminal.draw(|f| draw(f, &app))?;
    }

    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum KeyAction {
    Quit,
    Continue,
}

fn handle_key(key: KeyEvent, app: &mut App) -> KeyAction {
    if key.kind != KeyEventKind::Press {
        return KeyAction::Continue;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) => KeyAction::Quit,
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => KeyAction::Quit,
        (KeyCode::Char('d'), _) => {
            app.details_open = !app.details_open;
            KeyAction::Continue
        }
        (KeyCode::Char('r'), _) => {
            // Mark stale; the poll task will catch up on next tick.
            app.error = None;
            KeyAction::Continue
        }
        (KeyCode::Up, _) => {
            app.scroll_up();
            KeyAction::Continue
        }
        (KeyCode::Down, _) => {
            app.scroll_down();
            KeyAction::Continue
        }
        _ => KeyAction::Continue,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Snapshot model
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Snapshot {
    spend_today_usd: f64,
    spend_7d_usd: f64,
    spend_total_usd: f64,
    top_models: Vec<TopModel>,
    recent_payments: Vec<PaymentRow>,
    health: HealthSnapshot,
}

#[derive(Debug, Clone)]
struct TopModel {
    model: String,
    requests: i64,
    cost_usd: f64,
    mean_cost_usd: f64,
}

#[derive(Debug, Clone)]
struct PaymentRow {
    timestamp: String,
    model: String,
    cost_usd: f64,
    tx_signature: String,
}

#[derive(Debug, Clone)]
struct HealthSnapshot {
    overall: String,
    database: String,
    redis: String,
    providers: Vec<String>,
    solana_rpc: String,
    authenticated: bool,
}

// Wire types matching the gateway (kept private to this module).
#[derive(Debug, Deserialize)]
struct WireAdminStats {
    summary: WireSummary,
    by_model: Vec<WireModelStats>,
    by_day: Vec<WireDayStats>,
}

#[derive(Debug, Deserialize)]
struct WireSummary {
    #[allow(dead_code)] // Kept for wire-shape stability; not surfaced in TUI yet.
    #[serde(default)]
    total_requests: i64,
    #[serde(default)]
    total_cost_usdc: String,
}

#[derive(Debug, Deserialize)]
struct WireModelStats {
    model: String,
    requests: i64,
    #[serde(default)]
    cost_usdc: String,
}

#[derive(Debug, Deserialize)]
struct WireDayStats {
    date: String,
    #[allow(dead_code)] // Kept for wire-shape stability; not surfaced in TUI yet.
    #[serde(default)]
    requests: i64,
    #[serde(default)]
    spend: f64,
    #[serde(default)]
    cost_usdc: String,
}

#[derive(Debug, Deserialize)]
struct WireHealth {
    #[serde(default)]
    status: String,
    #[serde(default)]
    checks: Option<WireHealthChecks>,
}

#[derive(Debug, Deserialize)]
struct WireHealthChecks {
    #[serde(default)]
    database: String,
    #[serde(default)]
    redis: String,
    #[serde(default)]
    providers: Vec<String>,
    #[serde(default)]
    solana_rpc: String,
}

async fn fetch_snapshot(
    client: &reqwest::Client,
    base_url: &str,
    admin_token: Option<&str>,
) -> Result<Snapshot> {
    let stats_url = format!("{}/v1/admin/stats?days=7", base_url.trim_end_matches('/'));
    let health_url = format!("{}/health", base_url.trim_end_matches('/'));

    let stats_request = client.get(&stats_url);
    let stats_request = match admin_token {
        Some(t) => stats_request.bearer_auth(t),
        None => stats_request,
    };
    let health_request = client.get(&health_url);
    let health_request = match admin_token {
        Some(t) => health_request.bearer_auth(t),
        None => health_request,
    };

    let (stats_res, health_res) = tokio::join!(stats_request.send(), health_request.send());

    let stats_opt: Option<WireAdminStats> = match stats_res {
        Ok(resp) if resp.status().is_success() => resp.json().await.ok(),
        _ => None,
    };
    let health: WireHealth = match health_res {
        Ok(resp) => resp
            .json()
            .await
            .unwrap_or_else(|_| WireHealth::default_unknown()),
        Err(_) => WireHealth::default_unknown(),
    };

    let snapshot = build_snapshot(stats_opt, health, admin_token.is_some());
    Ok(snapshot)
}

impl WireHealth {
    fn default_unknown() -> Self {
        Self {
            status: "unreachable".to_string(),
            checks: None,
        }
    }
}

fn build_snapshot(
    stats: Option<WireAdminStats>,
    health: WireHealth,
    authenticated: bool,
) -> Snapshot {
    let (spend_today, spend_7d, spend_total, top_models) = match stats {
        Some(s) => derive_spend_and_models(&s),
        None => (0.0, 0.0, 0.0, Vec::new()),
    };

    let checks = health.checks.unwrap_or(WireHealthChecks {
        database: "unknown".to_string(),
        redis: "unknown".to_string(),
        providers: Vec::new(),
        solana_rpc: "unknown".to_string(),
    });

    Snapshot {
        spend_today_usd: spend_today,
        spend_7d_usd: spend_7d,
        spend_total_usd: spend_total,
        top_models,
        recent_payments: Vec::new(), // No endpoint yet; render empty-state.
        health: HealthSnapshot {
            overall: health.status,
            database: checks.database,
            redis: checks.redis,
            providers: checks.providers,
            solana_rpc: checks.solana_rpc,
            authenticated,
        },
    }
}

fn derive_spend_and_models(s: &WireAdminStats) -> (f64, f64, f64, Vec<TopModel>) {
    // Today's spend: pick the most-recent date entry.
    let spend_today = s
        .by_day
        .iter()
        .max_by(|a, b| a.date.cmp(&b.date))
        .map(|d| {
            if d.spend > 0.0 {
                d.spend
            } else {
                parse_usd(&d.cost_usdc)
            }
        })
        .unwrap_or(0.0);

    let spend_7d: f64 = s
        .by_day
        .iter()
        .map(|d| {
            if d.spend > 0.0 {
                d.spend
            } else {
                parse_usd(&d.cost_usdc)
            }
        })
        .sum();

    let spend_total = parse_usd(&s.summary.total_cost_usdc);

    let mut top_models: Vec<TopModel> = s
        .by_model
        .iter()
        .map(|m| {
            let cost_usd = parse_usd(&m.cost_usdc);
            let mean = if m.requests > 0 {
                cost_usd / m.requests as f64
            } else {
                0.0
            };
            TopModel {
                model: m.model.clone(),
                requests: m.requests,
                cost_usd,
                mean_cost_usd: mean,
            }
        })
        .collect();
    top_models.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    top_models.truncate(5);

    (spend_today, spend_7d, spend_total, top_models)
}

fn parse_usd(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(0.0)
}

// ───────────────────────────────────────────────────────────────────────────
// Formatters
// ───────────────────────────────────────────────────────────────────────────

/// Format a USD value as `$X.YYYYY` with adaptive precision.
fn format_usd(usd: f64) -> String {
    if usd >= 100.0 {
        format!("${usd:.2}")
    } else if usd >= 1.0 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.6}")
    }
}

/// Format a tx signature as `prefix..suffix` (8 chars + 4 chars).
fn truncate_signature(sig: &str) -> String {
    let len = sig.len();
    if len <= 16 {
        sig.to_string()
    } else {
        let prefix = &sig[..8];
        let suffix = &sig[len.saturating_sub(4)..];
        format!("{prefix}..{suffix}")
    }
}

/// Format an absolute timestamp string for the payments panel.
///
/// Best-effort: if the input is parseable as RFC3339, render `HH:MM` for today
/// or `MMM DD HH:MM` for older. Otherwise return as-is.
fn format_relative_timestamp(s: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let parsed = chrono::DateTime::parse_from_rfc3339(s);
    let dt = match parsed {
        Ok(d) => d.with_timezone(&chrono::Utc),
        Err(_) => return s.to_string(),
    };
    let same_day = dt.date_naive() == now.date_naive();
    if same_day {
        dt.format("%H:%M").to_string()
    } else {
        dt.format("%b %d %H:%M").to_string()
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Rendering
// ───────────────────────────────────────────────────────────────────────────

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Title
            Constraint::Min(10),   // Body
            Constraint::Length(1), // Footer
        ])
        .split(area);

    draw_title_bar(f, outer[0], app);
    draw_body(f, outer[1], app);
    draw_footer(f, outer[2]);

    if app.details_open {
        draw_details_modal(f, area, app);
    }
}

fn draw_title_bar(f: &mut Frame, area: Rect, app: &App) {
    let label = match (&app.last_refresh, &app.error) {
        (_, Some(err)) => format!(" gateway error: {err} "),
        (Some(t), None) => format!(" last refresh: {}s ago ", t.elapsed().as_secs()),
        (None, None) => " connecting... ".to_string(),
    };
    let title = Line::from(vec![
        Span::styled(" Solvela live ", Style::default().bold().fg(Color::Cyan)),
        Span::raw(format!(" — {} ", app.gateway_url)),
        Span::styled(label, Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(title), area);
}

fn draw_footer(f: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(" q", Style::default().bold()),
        Span::raw(" quit  "),
        Span::styled("r", Style::default().bold()),
        Span::raw(" refresh  "),
        Span::styled("↑↓", Style::default().bold()),
        Span::raw(" scroll  "),
        Span::styled("d", Style::default().bold()),
        Span::raw(" details "),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_body(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(rows[0]);
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);

    draw_spend(f, top[0], app);
    draw_top_models(f, top[1], app);
    draw_recent_payments(f, bottom[0], app);
    draw_health(f, bottom[1], app);
}

fn draw_spend(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" Spend ");
    let lines = match &app.snapshot {
        None => vec![Line::from(Span::styled(
            "  (no data yet)",
            Style::default().fg(Color::DarkGray),
        ))],
        Some(s) => vec![
            Line::from(format!("  today: {}", format_usd(s.spend_today_usd))),
            Line::from(format!("    7d:  {}", format_usd(s.spend_7d_usd))),
            Line::from(format!(" total:  {}", format_usd(s.spend_total_usd))),
        ],
    };
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_top_models(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" Top models ");
    match &app.snapshot {
        None => {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "  (no data yet)",
                    Style::default().fg(Color::DarkGray),
                ))
                .block(block),
                area,
            );
        }
        Some(s) if s.top_models.is_empty() => {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "  no requests in the last 7 days",
                    Style::default().fg(Color::DarkGray),
                ))
                .block(block),
                area,
            );
        }
        Some(s) => {
            let rows = s.top_models.iter().map(|m| {
                Row::new(vec![
                    m.model.clone(),
                    format!("{} calls", m.requests),
                    format_usd(m.cost_usd),
                    format_usd(m.mean_cost_usd),
                ])
            });
            let widths = [
                Constraint::Percentage(45),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Length(12),
            ];
            let table = Table::new(rows, widths).block(block);
            f.render_widget(table, area);
        }
    }
}

fn draw_recent_payments(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Recent payments ");

    let snapshot = match &app.snapshot {
        Some(s) => s,
        None => {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "  (no data yet)",
                    Style::default().fg(Color::DarkGray),
                ))
                .block(block),
                area,
            );
            return;
        }
    };

    if snapshot.recent_payments.is_empty() {
        let body = Paragraph::new(vec![
            Line::from(Span::styled(
                "  no recent-payments stream yet",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  (server-side TODO: expose a live tx feed)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )),
        ])
        .block(block)
        .wrap(Wrap { trim: true });
        f.render_widget(body, area);
        return;
    }

    let now = chrono::Utc::now();
    let mut lines = Vec::with_capacity(snapshot.recent_payments.len() * 2);
    for (idx, p) in snapshot
        .recent_payments
        .iter()
        .enumerate()
        .skip(app.payments_scroll)
    {
        let highlight = if idx == app.payments_scroll {
            Style::default().fg(Color::Cyan).bold()
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format_relative_timestamp(&p.timestamp, now), highlight),
            Span::raw("  "),
            Span::raw(p.model.clone()),
            Span::raw("  "),
            Span::raw(format_usd(p.cost_usd)),
        ]));
        lines.push(Line::from(Span::styled(
            format!("        {}", truncate_signature(&p.tx_signature)),
            Style::default().fg(Color::DarkGray),
        )));
    }
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_health(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" Health ");
    let snapshot = match &app.snapshot {
        Some(s) => s,
        None => {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "  (no data yet)",
                    Style::default().fg(Color::DarkGray),
                ))
                .block(block),
                area,
            );
            return;
        }
    };

    let overall_color = match snapshot.health.overall.as_str() {
        "ok" => Color::Green,
        "degraded" => Color::Yellow,
        "error" | "unreachable" => Color::Red,
        _ => Color::Gray,
    };

    let mut lines = vec![Line::from(vec![
        Span::raw("  status:    "),
        Span::styled(
            snapshot.health.overall.clone(),
            Style::default().fg(overall_color).bold(),
        ),
    ])];

    if snapshot.health.authenticated {
        lines.push(Line::from(format!(
            "  database:  {}",
            snapshot.health.database
        )));
        lines.push(Line::from(format!(
            "  redis:     {}",
            snapshot.health.redis
        )));
        lines.push(Line::from(format!(
            "  solana_rpc: {}",
            snapshot.health.solana_rpc
        )));
        if snapshot.health.providers.is_empty() {
            lines.push(Line::from(Span::styled(
                "  providers: (none configured — demo mode)",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            lines.push(Line::from(format!(
                "  providers: {}",
                snapshot.health.providers.join(", ")
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  (set SOLVELA_ADMIN_TOKEN for full checks)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_details_modal(f: &mut Frame, area: Rect, app: &App) {
    // 60% width, 50% height, centered.
    let w = area.width.saturating_mul(60) / 100;
    let h = area.height.saturating_mul(50) / 100;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let modal = Rect::new(x, y, w, h);

    f.render_widget(Clear, modal);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Payment details (d to close) ")
        .border_style(Style::default().fg(Color::Cyan));

    let snapshot = match &app.snapshot {
        Some(s) => s,
        None => {
            f.render_widget(
                Paragraph::new("  no snapshot loaded yet").block(block),
                modal,
            );
            return;
        }
    };

    let payment = snapshot.recent_payments.get(app.payments_scroll);
    let lines = match payment {
        None => vec![Line::from(Span::styled(
            "  no payment selected — recent payments stream not available yet",
            Style::default().fg(Color::DarkGray),
        ))],
        Some(p) => vec![
            Line::from(format!("  timestamp:  {}", p.timestamp)),
            Line::from(format!("  model:      {}", p.model)),
            Line::from(format!("  cost:       {}", format_usd(p.cost_usd))),
            Line::from(format!("  tx:         {}", p.tx_signature)),
            Line::from(""),
            Line::from(Span::styled(
                format!("  https://solscan.io/tx/{}", p.tx_signature),
                Style::default().fg(Color::Blue),
            )),
        ],
    };
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        modal,
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Tests — pure logic only; TUI rendering is not tested.
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_health_checks() -> WireHealthChecks {
        WireHealthChecks {
            database: "not_configured".to_string(),
            redis: "not_configured".to_string(),
            providers: vec!["openai".to_string()],
            solana_rpc: "configured".to_string(),
        }
    }

    #[test]
    fn parse_usd_handles_well_formed_string() {
        assert!((parse_usd("3.847291") - 3.847291).abs() < 1e-9);
        assert_eq!(parse_usd("0.000000"), 0.0);
    }

    #[test]
    fn parse_usd_returns_zero_on_garbage() {
        assert_eq!(parse_usd(""), 0.0);
        assert_eq!(parse_usd("not a number"), 0.0);
    }

    #[test]
    fn format_usd_uses_adaptive_precision() {
        assert_eq!(format_usd(0.0), "$0.000000");
        assert_eq!(format_usd(0.123456789), "$0.123457");
        assert_eq!(format_usd(5.5), "$5.5000");
        assert_eq!(format_usd(1234.5), "$1234.50");
    }

    #[test]
    fn truncate_signature_keeps_short_signatures() {
        assert_eq!(truncate_signature(""), "");
        assert_eq!(truncate_signature("short"), "short");
        assert_eq!(truncate_signature("0123456789abcdef"), "0123456789abcdef");
    }

    #[test]
    fn truncate_signature_shortens_long_signatures() {
        let sig = "58Aabc1234567890longsignatured2f1";
        let out = truncate_signature(sig);
        assert!(out.starts_with("58Aabc12"));
        assert!(out.ends_with("d2f1"));
        assert!(out.contains(".."));
    }

    #[test]
    fn format_relative_timestamp_returns_input_for_unparseable() {
        let now = chrono::Utc::now();
        assert_eq!(format_relative_timestamp("garbage", now), "garbage");
        assert_eq!(format_relative_timestamp("", now), "");
    }

    #[test]
    fn format_relative_timestamp_renders_hhmm_for_same_day() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-04-30T15:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let out = format_relative_timestamp("2026-04-30T14:32:11Z", now);
        assert_eq!(out, "14:32");
    }

    #[test]
    fn format_relative_timestamp_renders_full_for_other_days() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-04-30T15:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let out = format_relative_timestamp("2026-04-28T08:15:00Z", now);
        assert_eq!(out, "Apr 28 08:15");
    }

    #[test]
    fn build_snapshot_handles_no_stats() {
        let health = WireHealth {
            status: "ok".to_string(),
            checks: Some(empty_health_checks()),
        };
        let s = build_snapshot(None, health, true);
        assert_eq!(s.spend_today_usd, 0.0);
        assert_eq!(s.spend_7d_usd, 0.0);
        assert_eq!(s.spend_total_usd, 0.0);
        assert!(s.top_models.is_empty());
        assert_eq!(s.health.overall, "ok");
        assert!(s.health.authenticated);
    }

    #[test]
    fn build_snapshot_extracts_models_and_spend() {
        let stats = WireAdminStats {
            summary: WireSummary {
                total_requests: 1247,
                total_cost_usdc: "3.847291".to_string(),
            },
            by_model: vec![
                WireModelStats {
                    model: "claude-sonnet-4-6".to_string(),
                    requests: 412,
                    cost_usdc: "1.923000".to_string(),
                },
                WireModelStats {
                    model: "gpt-oss-120b".to_string(),
                    requests: 200,
                    cost_usdc: "0.000000".to_string(),
                },
            ],
            by_day: vec![
                WireDayStats {
                    date: "2026-04-29".to_string(),
                    requests: 30,
                    spend: 0.5,
                    cost_usdc: "0.500000".to_string(),
                },
                WireDayStats {
                    date: "2026-04-30".to_string(),
                    requests: 47,
                    spend: 0.142,
                    cost_usdc: "0.142300".to_string(),
                },
            ],
        };
        let s = build_snapshot(
            Some(stats),
            WireHealth {
                status: "ok".to_string(),
                checks: Some(empty_health_checks()),
            },
            false,
        );
        assert!((s.spend_today_usd - 0.142).abs() < 1e-6);
        assert!((s.spend_7d_usd - 0.642).abs() < 1e-6);
        assert!((s.spend_total_usd - 3.847291).abs() < 1e-6);
        assert_eq!(s.top_models.len(), 2);
        assert_eq!(s.top_models[0].model, "claude-sonnet-4-6");
        assert!(!s.health.authenticated);
    }

    #[test]
    fn snapshot_top_models_sorted_descending_by_cost() {
        let stats = WireAdminStats {
            summary: WireSummary {
                total_requests: 0,
                total_cost_usdc: "0.0".to_string(),
            },
            by_model: vec![
                WireModelStats {
                    model: "cheap".to_string(),
                    requests: 100,
                    cost_usdc: "0.001".to_string(),
                },
                WireModelStats {
                    model: "premium".to_string(),
                    requests: 5,
                    cost_usdc: "5.0".to_string(),
                },
                WireModelStats {
                    model: "free".to_string(),
                    requests: 1000,
                    cost_usdc: "0.0".to_string(),
                },
            ],
            by_day: vec![],
        };
        let s = build_snapshot(
            Some(stats),
            WireHealth {
                status: "ok".to_string(),
                checks: Some(empty_health_checks()),
            },
            true,
        );
        assert_eq!(s.top_models[0].model, "premium");
        assert_eq!(s.top_models[1].model, "cheap");
        assert_eq!(s.top_models[2].model, "free");
    }

    #[test]
    fn app_scroll_bounds_saturating() {
        let mut app = App::new("http://localhost:8402".to_string());
        // No snapshot — scroll up/down should be no-ops, never panic.
        app.scroll_up();
        assert_eq!(app.payments_scroll, 0);
        app.scroll_down();
        assert_eq!(app.payments_scroll, 0);

        // With three payments, scroll_down stops at index 2 (len-1).
        app.snapshot = Some(Snapshot {
            spend_today_usd: 0.0,
            spend_7d_usd: 0.0,
            spend_total_usd: 0.0,
            top_models: vec![],
            recent_payments: vec![
                PaymentRow {
                    timestamp: "t".to_string(),
                    model: "m".to_string(),
                    cost_usd: 0.0,
                    tx_signature: "s".to_string(),
                };
                3
            ],
            health: HealthSnapshot {
                overall: "ok".to_string(),
                database: "x".to_string(),
                redis: "x".to_string(),
                providers: vec![],
                solana_rpc: "x".to_string(),
                authenticated: false,
            },
        });
        for _ in 0..10 {
            app.scroll_down();
        }
        assert_eq!(app.payments_scroll, 2);
        for _ in 0..10 {
            app.scroll_up();
        }
        assert_eq!(app.payments_scroll, 0);
    }

    #[test]
    fn handle_key_q_quits() {
        let mut app = App::new("http://x".to_string());
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(handle_key(key, &mut app), KeyAction::Quit);
    }

    #[test]
    fn handle_key_ctrl_c_quits() {
        let mut app = App::new("http://x".to_string());
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(key, &mut app), KeyAction::Quit);
    }

    #[test]
    fn handle_key_d_toggles_details() {
        let mut app = App::new("http://x".to_string());
        assert!(!app.details_open);
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        handle_key(key, &mut app);
        assert!(app.details_open);
        handle_key(key, &mut app);
        assert!(!app.details_open);
    }
}
