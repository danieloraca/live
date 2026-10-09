use crate::history::Error;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, TableState};
use serde::Deserialize;
use std::io::{IsTerminal, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_ADDRESS: &str = "127.0.0.1:9999";
const STATUS_INTERVAL: Duration = Duration::from_secs(5);
const HISTORY_INTERVAL: Duration = Duration::from_secs(30);

pub enum Command {
    Serve,
    Status(String),
    Watch(String),
    Help,
}

pub fn command(mut args: impl Iterator<Item = String>) -> Result<Command, Error> {
    let Some(action) = args.next() else {
        return Ok(Command::Serve);
    };
    if action == "--help" || action == "-h" || action == "help" {
        return Ok(Command::Help);
    }
    if action != "status" && action != "watch" {
        return Err(format!("Unknown command: {action}. Run `live --help` for usage.").into());
    }
    let mut address =
        std::env::var("LIVE_CLI_ADDRESS").unwrap_or_else(|_| DEFAULT_ADDRESS.to_owned());
    while let Some(arg) = args.next() {
        if arg == "--address" {
            address = args.next().ok_or("--address needs a host:port value")?;
        } else {
            return Err(format!("Unknown option: {arg}. Run `live --help` for usage.").into());
        }
    }
    if address.is_empty() || !address.contains(':') {
        return Err("CLI address must be host:port".into());
    }
    Ok(if action == "status" {
        Command::Status(address)
    } else {
        Command::Watch(address)
    })
}

pub fn run(command: Command) -> Result<(), Error> {
    match command {
        Command::Serve => unreachable!(),
        Command::Help => {
            println!(
                "Pi Status\n\nUsage:\n  live                         Start the web dashboard\n  live status [--address HOST:PORT]  Print current status\n  live watch  [--address HOST:PORT]  Open the terminal dashboard\n\nCLI address defaults to {DEFAULT_ADDRESS}; override it with LIVE_CLI_ADDRESS.\nThe web service must be running. Press q or Esc to leave watch."
            );
            Ok(())
        }
        Command::Status(address) => print_status(&fetch(&address, "/api/status")?),
        Command::Watch(address) => watch(&address),
    }
}

fn fetch<T: for<'de> Deserialize<'de>>(address: &str, path: &str) -> Result<T, Error> {
    let mut last_error = None;
    let mut connected = None;
    for target in address.to_socket_addrs()? {
        match TcpStream::connect_timeout(&target, Duration::from_secs(2)) {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let mut stream = connected.ok_or_else(|| {
        format!(
            "Cannot connect to Pi Status at {address}: {}",
            last_error.map_or_else(|| "address did not resolve".into(), |e| e.to_string())
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = Vec::new();
    stream.take(1_048_577).read_to_end(&mut response)?;
    if response.len() > 1_048_576 {
        return Err("Dashboard response is too large".into());
    }
    let header_end = response
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .ok_or("Invalid dashboard HTTP response")?;
    let headers = std::str::from_utf8(&response[..header_end])?;
    let code = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or("Invalid dashboard HTTP status")?;
    let body = &response[header_end + 4..];
    if code != 200 {
        let detail = serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("HTTP {code}"));
        return Err(format!("Dashboard at {address}: {detail}").into());
    }
    Ok(serde_json::from_slice(body)?)
}

#[derive(Deserialize)]
struct Status {
    metrics: Metrics,
    system: System,
    services: Vec<Service>,
    history: SaveState,
}

#[derive(Deserialize)]
struct Metrics {
    timestamp: u64,
    uptime: Option<f64>,
    cpu: Option<f64>,
    cores: usize,
    memory: Option<Storage>,
    temperature: Option<f64>,
    frequency: Option<f64>,
    disk: Option<Storage>,
    load: Option<Vec<f64>>,
    processes: Option<usize>,
    rx: Option<f64>,
    tx: Option<f64>,
    throttled: Option<u32>,
}

#[derive(Deserialize)]
struct Storage {
    total: u64,
    available: u64,
    percent: f64,
    #[serde(default)]
    swap_used: u64,
}

#[derive(Deserialize)]
struct System {
    model: Option<String>,
    os: String,
}

#[derive(Deserialize)]
struct Service {
    name: String,
    state: String,
    port: u16,
    http_status: Option<u16>,
    latency_ms: Option<f64>,
}

#[derive(Deserialize)]
struct SaveState {
    state: String,
    pending_samples: usize,
    dropped_samples: u64,
}

#[derive(Default, Deserialize)]
struct History {
    points: Vec<Point>,
}

#[derive(Deserialize)]
struct Point {
    timestamp: u64,
    cpu: Option<f64>,
    rx: Option<f64>,
    tx: Option<f64>,
    temperature: Option<f64>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn health(status: &Status) -> (&'static str, Color) {
    if now().saturating_sub(status.metrics.timestamp) > 20 {
        return ("Live data delayed", Color::Yellow);
    }
    if status.history.state != "ok" || status.history.dropped_samples > 0 {
        return ("History needs attention", Color::Red);
    }
    if status.services.is_empty() || status.services.iter().all(|s| s.state == "unknown") {
        return ("Service status unavailable", Color::Yellow);
    }
    if status.services.iter().any(|s| s.state != "active") {
        return ("A service needs attention", Color::Yellow);
    }
    if status
        .services
        .iter()
        .any(|s| s.http_status.is_none_or(|code| code >= 500))
    {
        return ("An app needs attention", Color::Yellow);
    }
    if status
        .metrics
        .throttled
        .is_some_and(|flags| flags & 15 != 0)
    {
        return ("Hardware needs attention", Color::Red);
    }
    ("All services running", Color::Green)
}

fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{value:.1}%"))
}

fn bytes(value: f64) -> String {
    let mut value = value;
    let mut unit = "B";
    for next in ["KiB", "MiB", "GiB", "TiB"] {
        if value.abs() < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    if unit == "B" {
        format!("{value:.0} {unit}")
    } else {
        format!("{value:.1} {unit}")
    }
}

fn maybe_bytes(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), bytes)
}

fn clock(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{value:.0} MHz"))
}

fn uptime(value: Option<f64>) -> String {
    let Some(seconds) = value else {
        return "—".into();
    };
    let minutes = seconds as u64 / 60;
    if minutes >= 1440 {
        format!("{}d {}h", minutes / 1440, minutes % 1440 / 60)
    } else if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

fn storage(value: Option<&Storage>) -> String {
    value.map_or_else(
        || "—".into(),
        |value| {
            format!(
                "{:.1}% used · {} free / {}",
                value.percent,
                bytes(value.available as f64),
                bytes(value.total as f64)
            )
        },
    )
}

fn alerts(flags: Option<u32>) -> &'static str {
    match flags {
        Some(flags) if flags & 1 != 0 => "Undervoltage now",
        Some(flags) if flags & 4 != 0 => "Throttled now",
        Some(flags) if flags & 8 != 0 => "Temperature limit now",
        Some(flags) if flags & 2 != 0 => "Frequency capped now",
        Some(flags) if flags & 0xf0000 != 0 => "Earlier hardware warning",
        Some(_) => "No hardware alerts",
        None => "Hardware alerts unavailable",
    }
}

fn print_status(status: &Status) -> Result<(), Error> {
    let m = &status.metrics;
    println!("Pi Status · {}", health(status).0);
    println!(
        "{} · {} · uptime {} · sample {}s ago",
        status.system.model.as_deref().unwrap_or("Machine"),
        status.system.os,
        uptime(m.uptime),
        now().saturating_sub(m.timestamp)
    );
    println!(
        "CPU {} ({} cores) · {} · load {}",
        percent(m.cpu),
        m.cores,
        clock(m.frequency),
        m.load
            .as_ref()
            .map(|load| load
                .iter()
                .map(|n| format!("{n:.2}"))
                .collect::<Vec<_>>()
                .join(" / "))
            .unwrap_or_else(|| "—".into())
    );
    println!(
        "Memory {} · disk {}",
        storage(m.memory.as_ref()),
        storage(m.disk.as_ref())
    );
    println!(
        "Temperature {} · {}",
        m.temperature
            .map_or_else(|| "—".into(), |v| format!("{v:.1} °C")),
        alerts(m.throttled)
    );
    println!(
        "Network ↓ {}/s · ↑ {}/s · processes {}",
        maybe_bytes(m.rx),
        maybe_bytes(m.tx),
        m.processes.map_or_else(|| "—".into(), |v| v.to_string())
    );
    println!("Services:");
    for service in &status.services {
        println!(
            "  {:<18} {:<10} :{:<5} {}",
            service.name,
            service.state,
            service.port,
            service_reply(service)
        );
    }
    if status.history.state != "ok" || status.history.dropped_samples > 0 {
        println!(
            "History: {} · {} pending · {} dropped",
            status.history.state, status.history.pending_samples, status.history.dropped_samples
        );
    }
    Ok(())
}

fn service_reply(service: &Service) -> String {
    service.http_status.map_or_else(
        || {
            if service.state == "active" {
                "No HTTP response"
            } else {
                "Not checked"
            }
            .into()
        },
        |code| {
            service.latency_ms.map_or_else(
                || format!("HTTP {code}"),
                |latency| format!("HTTP {code} · {latency:.0} ms"),
            )
        },
    )
}

fn watch(address: &str) -> Result<(), Error> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(
            "`live watch` needs an interactive terminal; use `live status` for plain output".into(),
        );
    }
    let mut status = None;
    let mut history = History::default();
    let mut error = String::new();
    let mut history_error = false;
    let mut last_status = Instant::now() - STATUS_INTERVAL;
    let mut last_history = Instant::now() - HISTORY_INTERVAL;
    let mut table_state = TableState::default();
    ratatui::run(|terminal| -> Result<(), Error> {
        loop {
            if last_status.elapsed() >= STATUS_INTERVAL {
                match fetch(address, "/api/status") {
                    Ok(value) => {
                        status = Some(value);
                        error.clear();
                    }
                    Err(failure) => error = failure.to_string(),
                }
                last_status = Instant::now();
            }
            if last_history.elapsed() >= HISTORY_INTERVAL {
                match fetch(address, "/api/history?minutes=15") {
                    Ok(value) => {
                        history = value;
                        history_error = false;
                    }
                    Err(_) => history_error = true,
                }
                last_history = Instant::now();
            }
            terminal.draw(|frame| {
                draw(
                    frame,
                    status.as_ref(),
                    &history,
                    &error,
                    history_error,
                    &mut table_state,
                )
            })?;
            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char('r') => {
                        last_status = Instant::now() - STATUS_INTERVAL;
                        last_history = Instant::now() - HISTORY_INTERVAL;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let len = status.as_ref().map_or(0, |s| s.services.len());
                        if len > 0 {
                            table_state.select(Some(
                                table_state.selected().map_or(0, |i| (i + 1).min(len - 1)),
                            ));
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        table_state
                            .select(Some(table_state.selected().unwrap_or(0).saturating_sub(1)));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })
}

fn draw(
    frame: &mut ratatui::Frame,
    status: Option<&Status>,
    history: &History,
    error: &str,
    history_error: bool,
    table_state: &mut TableState,
) {
    let area = frame.area();
    if area.width < 55 || area.height < 19 {
        frame.render_widget(
            Paragraph::new("Pi Status · enlarge terminal to at least 55 × 19 · q to quit"),
            area,
        );
        return;
    }
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);
    let (label, mut color) = status.map_or(("Connecting…", Color::Yellow), health);
    let title = if error.is_empty() {
        format!("Pi Status · {label}")
    } else {
        color = Color::Yellow;
        format!("Pi Status · connection delayed: {error}")
    };
    frame.render_widget(
        Paragraph::new(title)
            .style(Style::default().fg(color).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL)),
        sections[0],
    );
    if let Some(status) = status {
        draw_overview(frame, sections[1], status);
    } else {
        frame.render_widget(
            Paragraph::new("Waiting for the status API…")
                .block(Block::default().title("Overview").borders(Borders::ALL)),
            sections[1],
        );
    }
    draw_trends(frame, sections[2], history, history_error);
    draw_services(frame, sections[3], status, table_state);
    frame.render_widget(
        Paragraph::new("q/Esc quit  ·  r refresh  ·  ↑/↓ or j/k browse services"),
        sections[4],
    );
}

fn draw_overview(frame: &mut ratatui::Frame, area: Rect, status: &Status) {
    let m = &status.metrics;
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let left = vec![
        Line::from(format!(
            "CPU          {} · {} cores",
            percent(m.cpu),
            m.cores
        )),
        Line::from(format!("Memory       {}", storage(m.memory.as_ref()))),
        Line::from(format!(
            "Temperature  {}",
            m.temperature
                .map_or_else(|| "—".into(), |v| format!("{v:.1} °C"))
        )),
        Line::from(format!("Disk         {}", storage(m.disk.as_ref()))),
        Line::from(format!("Uptime       {}", uptime(m.uptime))),
    ];
    let right = vec![
        Line::from(format!("Download   {}/s", maybe_bytes(m.rx))),
        Line::from(format!("Upload     {}/s", maybe_bytes(m.tx))),
        Line::from(format!("ARM clock  {}", clock(m.frequency))),
        Line::from(format!(
            "Swap       {}",
            m.memory
                .as_ref()
                .map_or_else(|| "—".into(), |v| bytes(v.swap_used as f64))
        )),
        Line::from(alerts(m.throttled)),
    ];
    frame.render_widget(
        Paragraph::new(left).block(Block::default().title("Vitals").borders(Borders::ALL)),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(right).block(Block::default().title("Activity").borders(Borders::ALL)),
        columns[1],
    );
}

fn spark(
    values: impl Iterator<Item = (u64, Option<f64>)>,
    width: usize,
    ceiling: Option<f64>,
    end: u64,
) -> String {
    if width == 0 {
        return "—".into();
    }
    let start = end.saturating_sub(15 * 60);
    let mut sums = vec![0.0; width];
    let mut counts = vec![0u32; width];
    let mut saw_point = false;
    for (timestamp, value) in values {
        if timestamp < start || timestamp > end {
            continue;
        }
        saw_point = true;
        if let Some(value) = value {
            let index = (((timestamp - start) as u128 * width as u128) / (15 * 60))
                .min((width - 1) as u128) as usize;
            sums[index] += value;
            counts[index] += 1;
        }
    }
    if !saw_point {
        return "—".into();
    }
    let averages = sums
        .iter()
        .zip(&counts)
        .map(|(sum, count)| (*count > 0).then_some(*sum / *count as f64))
        .collect::<Vec<_>>();
    let peak = ceiling.unwrap_or_else(|| averages.iter().flatten().copied().fold(1.0, f64::max));
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    averages
        .iter()
        .map(|value| match value {
            Some(value) => bars[((value.max(0.0) / peak).clamp(0.0, 1.0) * 7.0).round() as usize],
            None => '·',
        })
        .collect()
}

fn draw_trends(frame: &mut ratatui::Frame, area: Rect, history: &History, failed: bool) {
    let width = area.width.saturating_sub(17) as usize;
    let end = now();
    let cpu = spark(
        history.points.iter().map(|p| (p.timestamp, p.cpu)),
        width,
        Some(100.0),
        end,
    );
    let temp = spark(
        history.points.iter().map(|p| (p.timestamp, p.temperature)),
        width,
        Some(100.0),
        end,
    );
    let network = spark(
        history
            .points
            .iter()
            .map(|p| (p.timestamp, p.rx.zip(p.tx).map(|(rx, tx)| rx + tx))),
        width,
        None,
        end,
    );
    let title = if failed {
        "15-minute trends · delayed"
    } else {
        "15-minute trends"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("CPU          {cpu}")),
            Line::from(format!("Temperature  {temp}")),
            Line::from(format!("Network      {network}")),
        ])
        .block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

fn draw_services(
    frame: &mut ratatui::Frame,
    area: Rect,
    status: Option<&Status>,
    table_state: &mut TableState,
) {
    let services = status.map_or(&[][..], |s| s.services.as_slice());
    let rows = services.iter().map(|service| {
        let color =
            if service.state != "active" || service.http_status.is_none_or(|code| code >= 500) {
                Color::Yellow
            } else {
                Color::Green
            };
        Row::new([
            service.name.clone(),
            format!(":{}", service.port),
            service.state.clone(),
            service_reply(service),
        ])
        .style(Style::default().fg(color))
    });
    let count = services.iter().filter(|s| s.state == "active").count();
    let title = format!("Services · {count}/{} active", services.len());
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Min(16),
        ],
    )
    .header(
        Row::new(["App", "Port", "State", "HTTP"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().title(title).borders(Borders::ALL))
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(table, area, table_state);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"metrics":{"timestamp":1,"uptime":3661,"cpu":42.5,"cores":4,"memory":{"total":8192,"available":4096,"percent":50,"swap_used":0},"temperature":53.2,"frequency":1500,"disk":null,"load":[0.1,0.2,0.3],"processes":98,"rx":1024,"tx":null,"throttled":0},"system":{"model":"Pi","os":"Linux"},"services":[{"name":"App","state":"active","port":3000,"http_status":200,"latency_ms":4}],"history":{"state":"ok","pending_samples":0,"dropped_samples":0}}"#;

    #[test]
    fn parses_real_status_shape_and_missing_readings() {
        let status: Status = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(status.metrics.cpu, Some(42.5));
        assert_eq!(status.metrics.tx, None);
        assert_eq!(status.services[0].port, 3000);
        assert_eq!(storage(status.metrics.disk.as_ref()), "—");
    }

    #[test]
    fn spark_keeps_missing_readings_distinct_from_zero() {
        assert_eq!(
            spark(
                [(0, Some(0.0)), (450, None), (900, Some(100.0))].into_iter(),
                3,
                Some(100.0),
                900,
            ),
            "▁·█"
        );
    }
}
