mod history;
mod metrics;
mod services;
mod util;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct State {
    status: String,
}

fn main() -> Result<(), history::Error> {
    let address = std::env::var("LIVE_ADDRESS").unwrap_or_else(|_| "0.0.0.0:9999".into());
    let listener = TcpListener::bind(&address)?;
    listener.set_nonblocking(true)?;
    let database = history::database_path();
    let history = Arc::new(Mutex::new(history::Store::open(&database)?));
    let stopping = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stopping))?;
    }
    let state = Arc::new(RwLock::new(State::default()));
    let sampler_state = Arc::clone(&state);
    let sampler_history = Arc::clone(&history);
    let sampler_stopping = Arc::clone(&stopping);
    let sampler = thread::spawn(move || sample(sampler_state, sampler_history, sampler_stopping));

    // A bounded queue and read deadlines keep slow clients from blocking the page.
    let (sender, receiver) = mpsc::sync_channel::<TcpStream>(32);
    let receiver = Arc::new(Mutex::new(receiver));
    for _ in 0..4 {
        let receiver = Arc::clone(&receiver);
        let state = Arc::clone(&state);
        let history = Arc::clone(&history);
        thread::spawn(move || {
            loop {
                let stream = receiver.lock().unwrap().recv();
                let Ok(stream) = stream else { break };
                if let Err(error) = handle_connection(stream, &state, &history) {
                    eprintln!("Request failed: {error}");
                }
            }
        });
    }
    println!("Serving Pi Status at http://{address}");
    println!(
        "Saving history to {} (no automatic expiry)",
        database.display()
    );
    while !stopping.load(Ordering::Relaxed) && !sampler.is_finished() {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = sender.try_send(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                eprintln!("Connection failed: {error}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    stopping.store(true, Ordering::Relaxed);
    sampler.thread().unpark();
    sampler
        .join()
        .map_err(|_| std::io::Error::other("Sampler failed"))??;
    Ok(())
}

fn sample(
    state: Arc<RwLock<State>>,
    history: Arc<Mutex<history::Store>>,
    stopping: Arc<AtomicBool>,
) -> rusqlite::Result<()> {
    let mut collector = metrics::Collector::new();
    let mut service_data = "[]".to_owned();
    let mut services_updated_at = 0;
    let mut cycle = 0u64;
    while !stopping.load(Ordering::Relaxed) {
        let started = Instant::now();
        if cycle.is_multiple_of(6) {
            service_data = services::collect();
            services_updated_at = util::now();
        }
        let snapshot = collector.collect(cycle.is_multiple_of(6));
        let history_status = {
            let mut history = history.lock().unwrap();
            history.record(snapshot.point());
            history.status_json()
        };
        let status = util::object(&[
            ("metrics", snapshot.json()),
            ("system", collector.system.clone()),
            ("services", service_data.clone()),
            ("services_updated_at", services_updated_at.to_string()),
            ("history", history_status),
        ]);
        state.write().unwrap().status = status;
        cycle = cycle.wrapping_add(1);
        thread::park_timeout(INTERVAL.saturating_sub(started.elapsed()));
    }
    history.lock().unwrap().flush()
}

fn route(
    path: &str,
    state: &RwLock<State>,
    history: &Mutex<history::Store>,
) -> (u16, &'static str, String) {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    match path {
        "/" | "/index.html" => (
            200,
            "text/html; charset=utf-8",
            include_str!("../static/index.html").into(),
        ),
        "/style.css" => (
            200,
            "text/css; charset=utf-8",
            include_str!("../static/style.css").into(),
        ),
        "/app.js" => (
            200,
            "text/javascript; charset=utf-8",
            include_str!("../static/app.js").into(),
        ),
        "/charts.mjs" => (
            200,
            "text/javascript; charset=utf-8",
            include_str!("../static/charts.mjs").into(),
        ),
        "/api/status" => {
            let state = state.read().unwrap();
            if state.status.is_empty() {
                (
                    503,
                    "application/json",
                    "{\"error\":\"First sample is being collected\"}".into(),
                )
            } else {
                (200, "application/json", state.status.clone())
            }
        }
        "/api/history" => {
            let Some(range) = history::Range::parse(query) else {
                return (
                    400,
                    "application/json",
                    "{\"error\":\"Supported minutes: 15, 60, 1440, 10080, 43200\"}".into(),
                );
            };
            match history.lock().unwrap().query_json(range, util::now()) {
                Ok(json) => (200, "application/json", json),
                Err(error) => {
                    eprintln!("History query failed: {error}");
                    (
                        503,
                        "application/json",
                        "{\"error\":\"History is temporarily unavailable\"}".into(),
                    )
                }
            }
        }
        "/favicon.ico" => (204, "image/x-icon", String::new()),
        _ => (404, "text/plain; charset=utf-8", "Not found\n".into()),
    }
}

fn handle_connection(
    mut stream: TcpStream,
    state: &RwLock<State>,
    history: &Mutex<history::Store>,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    let deadline = Instant::now() + Duration::from_secs(2);
    while request.len() < 8192 && Instant::now() < deadline {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|part| part == b"\r\n\r\n") {
            break;
        }
    }
    if !request.windows(4).any(|part| part == b"\r\n\r\n") {
        return Ok(());
    }
    let request = String::from_utf8_lossy(&request);
    let mut first_line = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = first_line.next().unwrap_or_default();
    let path = first_line.next().unwrap_or_default();
    let (status, content_type, body) = if method == "GET" || method == "HEAD" {
        route(path, state, history)
    } else {
        (405, "text/plain", "Method not allowed\n".into())
    };
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Service Unavailable",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if method != "HEAD" {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_report_warmup_and_do_not_return_html_for_missing_assets() {
        let state = RwLock::new(State::default());
        let dir = tempfile::tempdir().unwrap();
        let history =
            Mutex::new(history::Store::open(&dir.path().join("history.sqlite3")).unwrap());
        assert_eq!(route("/api/status", &state, &history).0, 503);
        assert_eq!(route("/missing.js", &state, &history).0, 404);
        assert_eq!(route("/api/history?minutes=0", &state, &history).0, 400);
        assert!(
            route("/api/history", &state, &history)
                .2
                .starts_with("{\"points\":[],\"resolution_seconds\":5,")
        );
        assert_eq!(route("/charts.mjs", &state, &history).0, 200);
        assert!(route("/", &state, &history).2.contains("My Pi"));
    }
}
