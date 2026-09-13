mod metrics;
mod services;
mod util;

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_secs(5);
const HISTORY_LIMIT: usize = 721;

#[derive(Default)]
struct State {
    status: String,
    history: VecDeque<String>,
}

fn main() -> std::io::Result<()> {
    let address = std::env::var("LIVE_ADDRESS").unwrap_or_else(|_| "0.0.0.0:9999".into());
    let listener = TcpListener::bind(&address)?;
    let state = Arc::new(RwLock::new(State::default()));
    let sampler_state = Arc::clone(&state);
    thread::spawn(move || sample(sampler_state));

    // A bounded queue and read deadlines keep slow clients from blocking the page.
    let (sender, receiver) = mpsc::sync_channel::<TcpStream>(32);
    let receiver = Arc::new(Mutex::new(receiver));
    for _ in 0..4 {
        let receiver = Arc::clone(&receiver);
        let state = Arc::clone(&state);
        thread::spawn(move || {
            loop {
                let stream = receiver.lock().unwrap().recv();
                let Ok(stream) = stream else { break };
                if let Err(error) = handle_connection(stream, &state) {
                    eprintln!("Request failed: {error}");
                }
            }
        });
    }
    println!("Serving Pi Status at http://{address}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let _ = sender.try_send(stream);
            }
            Err(error) => eprintln!("Connection failed: {error}"),
        }
    }
    Ok(())
}

fn sample(state: Arc<RwLock<State>>) {
    let mut collector = metrics::Collector::new();
    let mut service_data = "[]".to_owned();
    let mut services_updated_at = 0;
    let mut cycle = 0u64;
    loop {
        let started = Instant::now();
        if cycle.is_multiple_of(6) {
            service_data = services::collect();
            services_updated_at = util::now();
        }
        let snapshot = collector.collect(cycle.is_multiple_of(6));
        let status = util::object(&[
            ("metrics", snapshot.json()),
            ("system", collector.system.clone()),
            ("services", service_data.clone()),
            ("services_updated_at", services_updated_at.to_string()),
        ]);
        {
            let mut state = state.write().unwrap();
            state.status = status;
            state.history.push_back(snapshot.point_json());
            if state.history.len() > HISTORY_LIMIT {
                state.history.pop_front();
            }
        }
        cycle = cycle.wrapping_add(1);
        thread::sleep(INTERVAL.saturating_sub(started.elapsed()));
    }
}

fn route(path: &str, state: &RwLock<State>) -> (u16, &'static str, String) {
    match path.split('?').next().unwrap_or(path) {
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
        "/api/history" => (
            200,
            "application/json",
            format!(
                "[{}]",
                state
                    .read()
                    .unwrap()
                    .history
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ),
        "/favicon.ico" => (204, "image/x-icon", String::new()),
        _ => (404, "text/plain; charset=utf-8", "Not found\n".into()),
    }
}

fn handle_connection(mut stream: TcpStream, state: &RwLock<State>) -> std::io::Result<()> {
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
        route(path, state)
    } else {
        (405, "text/plain", "Method not allowed\n".into())
    };
    let reason = match status {
        200 => "OK",
        204 => "No Content",
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
        assert_eq!(route("/api/status", &state).0, 503);
        assert_eq!(route("/missing.js", &state).0, 404);
        assert_eq!(route("/api/history?test=1", &state).2, "[]");
        assert!(route("/", &state).2.contains("Raspberry Pi"));
    }
}
