use crate::util::{command, now, number, object, quote};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum Check {
    Http(&'static str),
    // Availability only: authenticated MCP/database health remains unverified.
    Tcp,
}

// Ports are fallbacks when systemd's main process is a wrapper (for example Docker).
const SERVICES: &[(&str, &str, u16, Check)] = &[
    ("iploc.service", "IP Location", 3000, Check::Http("/")),
    (
        "id-generator.service",
        "ID Generator",
        3012,
        Check::Http("/"),
    ),
    ("tetris.service", "Tetris", 3020, Check::Http("/")),
    ("solitaire.service", "Solitaire", 3021, Check::Http("/")),
    ("dario.service", "Dario", 3041, Check::Http("/")),
    (
        "trader-dashboard.service",
        "Trader Dashboard",
        3040,
        Check::Http("/"),
    ),
    ("elite.service", "Elite", 3141, Check::Http("/")),
    ("sym_notes.service", "Sym Notes", 3444, Check::Http("/")),
    ("dreamer.service", "Dreamer", 3445, Check::Tcp),
    ("jirpi.service", "JiraPi", 5644, Check::Http("/healthz")),
    ("token-grove.service", "Token Grove", 9988, Check::Http("/")),
    ("live.service", "Live Status", 9999, Check::Http("/")),
];

fn properties(output: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    output
        .split("\n\n")
        .filter_map(|block| {
            let values: BTreeMap<String, String> = block
                .lines()
                .filter_map(|line| {
                    let (key, value) = line.split_once('=')?;
                    Some((key.into(), value.into()))
                })
                .collect();
            Some((values.get("Id")?.clone(), values))
        })
        .collect()
}

fn ports(output: &str, pid: u32) -> BTreeSet<u16> {
    if pid == 0 {
        return BTreeSet::new();
    }
    let pattern = format!("pid={pid},");
    output
        .lines()
        .filter(|line| line.contains(&pattern))
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            // ss omits the Netid column when only one transport is requested.
            let address_index = if fields.first() == Some(&"tcp") { 4 } else { 3 };
            fields.get(address_index)?.rsplit(':').next()?.parse().ok()
        })
        .collect()
}

fn probe_address(sockets: &str, port: u16, pid: u32) -> SocketAddr {
    let pid_pattern = format!("pid={pid},");
    let has_process_sockets = pid != 0 && sockets.lines().any(|line| line.contains(&pid_pattern));
    for line in sockets
        .lines()
        .filter(|line| !has_process_sockets || line.contains(&pid_pattern))
    {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let address_index = if fields.first() == Some(&"tcp") { 4 } else { 3 };
        let Some((host, listener_port)) =
            fields.get(address_index).and_then(|s| s.rsplit_once(':'))
        else {
            continue;
        };
        if listener_port.parse::<u16>() != Ok(port) {
            continue;
        }
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = host.parse::<IpAddr>()
            && !ip.is_unspecified()
        {
            return SocketAddr::new(ip, port);
        }
    }
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[derive(Default)]
pub struct Collector {
    last_response: BTreeMap<&'static str, u64>,
}

#[derive(Default)]
struct Probe {
    status: Option<u16>,
    tcp_connected: Option<bool>,
    latency_ms: Option<f64>,
}

fn probe(address: SocketAddr, check: Check) -> Probe {
    let timeout = Duration::from_secs(2);
    let start = Instant::now();
    let Ok(mut stream) = TcpStream::connect_timeout(&address, timeout) else {
        return Probe {
            tcp_connected: matches!(check, Check::Tcp).then_some(false),
            ..Probe::default()
        };
    };
    let Check::Http(path) = check else {
        return Probe {
            tcp_connected: Some(true),
            latency_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
            ..Probe::default()
        };
    };
    let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
        || stream.write_all(request.as_bytes()).is_err()
    {
        return Probe::default();
    }
    let mut response = Vec::with_capacity(128);
    let mut buffer = [0; 128];
    while response.len() < 512 {
        let Ok(read) = stream.read(&mut buffer) else {
            return Probe::default();
        };
        if read == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..read]);
        if response.contains(&b'\n') {
            break;
        }
    }
    let status = parse_status(&response);
    Probe {
        status,
        tcp_connected: None,
        latency_ms: status.map(|_| start.elapsed().as_secs_f64() * 1000.0),
    }
}

fn parse_status(response: &[u8]) -> Option<u16> {
    let first_line = String::from_utf8_lossy(response);
    let mut fields = first_line
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    if !fields
        .next()
        .is_some_and(|version| version.starts_with("HTTP/"))
    {
        return None;
    }
    fields
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| (100..=599).contains(value))
}

impl Collector {
    pub fn collect(&mut self) -> String {
        let mut args = vec![
            "show",
            "--no-pager",
            "--property=Id,ActiveState,MainPID,LoadState",
        ];
        args.extend(SERVICES.iter().map(|service| service.0));
        let units = properties(&command("systemctl", &args).unwrap_or_default());
        // TCP listeners only: a UDP socket is not necessarily a web endpoint.
        let sockets = command("ss", &["-H", "-ltnp"]).unwrap_or_default();
        let entries = SERVICES
            .iter()
            .map(|(unit, name, fallback, check)| {
                let values = units.get(*unit);
                let get = |key: &str| values.and_then(|p| p.get(key)).map(String::as_str);
                let state = if get("LoadState") == Some("not-found") {
                    "unknown"
                } else {
                    get("ActiveState").unwrap_or("unknown")
                };
                let pid = get("MainPID").and_then(|s| s.parse().ok()).unwrap_or(0);
                let detected = ports(&sockets, pid);
                let port = if detected.contains(fallback) {
                    *fallback
                } else {
                    detected.first().copied().unwrap_or(*fallback)
                };
                (
                    *unit,
                    *name,
                    state,
                    probe_address(&sockets, port, pid),
                    *check,
                )
            })
            .collect::<Vec<_>>();
        // Local checks run together so one slow app cannot stall sampling.
        let probes = thread::scope(|scope| {
            entries
                .iter()
                .map(|(_, _, state, address, check)| {
                    scope.spawn(move || {
                        if *state == "active" {
                            probe(*address, *check)
                        } else {
                            Probe::default()
                        }
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|task| task.join().unwrap_or_default())
                .collect::<Vec<_>>()
        });
        let rows = entries
            .into_iter()
            .zip(probes)
            .map(|((unit, name, state, address, check), probe)| {
                let reachable = match check {
                    Check::Tcp => probe.tcp_connected == Some(true),
                    Check::Http(_) => probe
                        .status
                        .is_some_and(|status| (200..500).contains(&status)),
                };
                if reachable {
                    self.last_response.insert(unit, now());
                }
                object(&[
                    ("unit", quote(unit)),
                    ("name", quote(name)),
                    ("state", quote(state)),
                    ("port", address.port().to_string()),
                    (
                        "check",
                        quote(match check {
                            Check::Http(_) => "http",
                            Check::Tcp => "tcp",
                        }),
                    ),
                    (
                        "tcp_connected",
                        probe
                            .tcp_connected
                            .map_or("null".into(), |value| value.to_string()),
                    ),
                    ("http_status", number(probe.status)),
                    ("latency_ms", number(probe.latency_ms)),
                    (
                        "last_response",
                        number(self.last_response.get(unit).copied()),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        format!("[{}]", rows.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_availability_sends_no_credentials_or_http_and_reports_failure() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();
            assert!(request.is_empty());
            // Keep the listener owned until the caller has received the result.
            listener
        });
        let result = probe(address, Check::Tcp);
        assert_eq!(result.tcp_connected, Some(true));
        assert_eq!(result.status, None);
        assert!(result.latency_ms.is_some());
        drop(server.join().unwrap());
        // Port zero cannot identify a listening service; avoid races from reusing
        // a recently closed ephemeral port in another parallel test.
        let result = probe(SocketAddr::from(([127, 0, 0, 1], 0)), Check::Tcp);
        assert_eq!(result.tcp_connected, Some(false));
        assert_eq!(result.status, None);
        assert_eq!(result.latency_ms, None);
    }

    #[test]
    fn http_probes_still_request_the_configured_health_path() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"GET /healthz HTTP/1.1\r\n"));
            assert!(!String::from_utf8_lossy(&request).contains("Authorization"));
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        let result = probe(address, Check::Http("/healthz"));
        server.join().unwrap();
        assert_eq!(result.status, Some(204));
        assert_eq!(result.tcp_connected, None);
    }

    #[test]
    fn sockets_match_exact_pid_and_parse_ipv6() {
        let sockets = "LISTEN 0 128 [::]:3444 [::]:* users:((\"live\",pid=42,fd=4))\nLISTEN 0 128 0.0.0.0:9999 0.0.0.0:* users:((\"other\",pid=142,fd=4))";
        assert_eq!(ports(sockets, 42), BTreeSet::from([3444]));
        assert_eq!(
            ports(
                "tcp LISTEN 0 128 *:3000 *:* users:((\"app\",pid=42,fd=3))",
                42
            ),
            BTreeSet::from([3000])
        );
        assert!(ports(sockets, 0).is_empty());
    }

    #[test]
    fn probes_the_address_a_service_actually_binds() {
        let sockets =
            "LISTEN 0 128 0.0.0.0:9999 0.0.0.0:*\nLISTEN 0 128 192.168.0.25:5644 0.0.0.0:*";
        assert_eq!(
            probe_address(sockets, 5644, 0),
            "192.168.0.25:5644".parse().unwrap()
        );
        assert_eq!(
            probe_address(sockets, 9999, 0),
            "127.0.0.1:9999".parse().unwrap()
        );
        assert_eq!(
            probe_address("", 3444, 0),
            "127.0.0.1:3444".parse().unwrap()
        );
    }

    #[test]
    fn probes_its_own_listener_when_a_proxy_shares_the_port() {
        let sockets = "LISTEN 0 4096 100.106.171.95:9999 0.0.0.0:*\nLISTEN 0 128 127.0.0.1:9999 0.0.0.0:* users:((\"live\",pid=42,fd=3))";
        assert_eq!(
            probe_address(sockets, 9999, 42),
            "127.0.0.1:9999".parse().unwrap()
        );
        assert_eq!(
            probe_address(sockets, 9999, 142),
            "100.106.171.95:9999".parse().unwrap()
        );
    }

    #[test]
    fn units_stay_associated_when_property_order_changes() {
        let data = properties(
            "ActiveState=active\nId=live.service\nMainPID=42\n\nId=jirpi.service\nActiveState=failed\nMainPID=0",
        );
        assert_eq!(data["live.service"]["ActiveState"], "active");
        assert_eq!(data["jirpi.service"]["ActiveState"], "failed");
    }

    #[test]
    fn http_status_parser_accepts_success_and_errors() {
        assert_eq!(parse_status(b"HTTP/1.1 204 No Content\r\n"), Some(204));
        assert_eq!(
            parse_status(b"HTTP/1.0 503 Service Unavailable\r\n"),
            Some(503)
        );
        assert_eq!(parse_status(b"garbage\r\n"), None);
    }
}
