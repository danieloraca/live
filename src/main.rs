use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;

const ADDRESS: &str = "0.0.0.0:9999";

struct Service {
    unit: &'static str,
    name: &'static str,
    port_hint: Option<&'static str>,
}

const SERVICES: &[Service] = &[
    Service {
        unit: "iploc.service",
        name: "IP Location",
        port_hint: Some("3000"),
    },
    Service {
        unit: "id-generator.service",
        name: "ID Generator",
        port_hint: Some("3012"),
    },
    Service {
        unit: "sym_notes.service",
        name: "Sym Notes",
        port_hint: Some("3444"),
    },
    Service {
        unit: "live.service",
        name: "Live Status",
        port_hint: Some("9999"),
    },
];

fn page(host: &str) -> String {
    let services = SERVICES
        .iter()
        .map(|service| service_row(service, host))
        .collect::<Vec<_>>()
        .join("\n");

    r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Pi Status</title>
  <style>
    :root {
      color-scheme: light dark;
      font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: #101418;
      color: #eef3f8;
    }

    body {
      margin: 0;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 24px;
      box-sizing: border-box;
    }

    main {
      width: min(100%, 820px);
      border: 1px solid #33404c;
      border-radius: 8px;
      padding: 28px;
      background: #172029;
      box-shadow: 0 16px 40px rgb(0 0 0 / 30%);
    }

    h1 {
      margin: 0 0 10px;
      font-size: 32px;
      line-height: 1.15;
    }

    p {
      margin: 0;
      color: #b8c7d6;
      font-size: 16px;
      line-height: 1.5;
    }

    .status {
      display: inline-flex;
      align-items: center;
      gap: 8px;
      margin-bottom: 18px;
      color: #7ddc9a;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.08em;
      font-size: 13px;
    }

    .dot {
      width: 10px;
      height: 10px;
      border-radius: 50%;
      background: currentColor;
      box-shadow: 0 0 18px currentColor;
    }

    .services {
      display: grid;
      gap: 10px;
      margin-top: 28px;
    }

    .service {
      display: grid;
      grid-template-columns: minmax(0, 1fr) minmax(100px, auto) auto;
      gap: 14px;
      align-items: center;
      padding: 14px 16px;
      border: 1px solid #33404c;
      border-radius: 8px;
      background: #111820;
    }

    .service-name {
      display: block;
      color: #eef3f8;
      font-weight: 700;
      text-decoration: none;
    }

    .service-name:hover {
      text-decoration: underline;
    }

    .service-unit {
      display: block;
      margin-top: 3px;
      color: #8fa2b5;
      font-family: ui-monospace, "SFMono-Regular", Consolas, monospace;
      font-size: 13px;
      overflow-wrap: anywhere;
    }

    .service-ports {
      color: #d8e3ee;
      font-family: ui-monospace, "SFMono-Regular", Consolas, monospace;
      font-size: 13px;
      text-align: right;
      white-space: nowrap;
    }

    .pill {
      border-radius: 999px;
      padding: 5px 10px;
      font-size: 12px;
      font-weight: 800;
      text-transform: uppercase;
      letter-spacing: 0.05em;
      white-space: nowrap;
    }

    .pill-active {
      background: #12351f;
      color: #7ddc9a;
    }

    .pill-inactive {
      background: #3a2f16;
      color: #ffd36a;
    }

    .pill-failed {
      background: #3a1717;
      color: #ff8a8a;
    }

    .pill-unknown {
      background: #26313d;
      color: #b8c7d6;
    }

    @media (max-width: 560px) {
      main {
        padding: 22px;
      }

      h1 {
        font-size: 28px;
      }

      .service {
        grid-template-columns: 1fr;
      }

      .service-ports {
        text-align: left;
      }

      .pill {
        width: max-content;
      }
    }
  </style>
</head>
<body>
  <main>
    <div class="status"><span class="dot"></span> Online</div>
    <h1>Raspberry Pi Status</h1>
    <p>This page is being served by a small Rust process on port 9999.</p>
    <section class="services" aria-label="Services">
      {services}
    </section>
  </main>
</body>
</html>
"#
    .replace("{services}", &services)
}

fn service_row(service: &Service, host: &str) -> String {
    let state = service_state(service.unit);
    let detected_ports = service_ports(service.unit);
    let ports = if detected_ports.is_empty() {
        service.port_hint.unwrap_or("none").to_owned()
    } else {
        detected_ports.join(", ")
    };
    let class = match state.as_str() {
        "active" => "pill-active",
        "inactive" => "pill-inactive",
        "failed" => "pill-failed",
        _ => "pill-unknown",
    };
    let name = match link_port(&detected_ports, service.port_hint) {
        Some(port) => format!(
            r#"<a class="service-name" target="_blank" href="{}">{}</a>"#,
            escape_html(&service_url(host, &port)),
            escape_html(service.name)
        ),
        None => format!(
            r#"<span class="service-name">{}</span>"#,
            escape_html(service.name)
        ),
    };

    format!(
        r#"<article class="service">
        <div>
          {}
          <span class="service-unit">{}</span>
        </div>
        <span class="service-ports">{}</span>
        <span class="pill {class}">{}</span>
      </article>"#,
        name,
        escape_html(service.unit),
        escape_html(&ports),
        escape_html(&state)
    )
}

fn service_state(unit: &str) -> String {
    match Command::new("systemctl").args(["is-active", unit]).output() {
        Ok(output) => {
            let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();

            if state.is_empty() {
                "unknown".to_owned()
            } else {
                state
            }
        }
        Err(_) => "unknown".to_owned(),
    }
}

fn service_ports(unit: &str) -> Vec<String> {
    let Some(pid) = service_main_pid(unit) else {
        return Vec::new();
    };

    let Ok(output) = Command::new("ss").args(["-H", "-ltnup"]).output() else {
        return Vec::new();
    };

    let sockets = String::from_utf8_lossy(&output.stdout);
    let pid_pattern = format!("pid={pid},");
    let mut ports = BTreeSet::new();

    for line in sockets.lines().filter(|line| line.contains(&pid_pattern)) {
        let parts = line.split_whitespace().collect::<Vec<_>>();

        if parts.len() < 5 {
            continue;
        }

        if let Some(port) = local_port(parts[4]) {
            ports.insert(port);
        }
    }

    ports.into_iter().collect()
}

fn service_main_pid(unit: &str) -> Option<u32> {
    let output = Command::new("systemctl")
        .args(["show", unit, "--property=MainPID", "--value"])
        .output()
        .ok()?;

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
}

fn local_port(address: &str) -> Option<String> {
    let port = address.rsplit(':').next()?.trim_matches(']');

    if port.is_empty() || port == "*" {
        None
    } else {
        Some(port.to_owned())
    }
}

fn link_port(detected_ports: &[String], port_hint: Option<&str>) -> Option<String> {
    detected_ports
        .first()
        .cloned()
        .or_else(|| port_hint.map(ToOwned::to_owned))
}

fn service_url(host: &str, port: &str) -> String {
    format!("http://{}:{}/", host_without_port(host), port)
}

fn request_host(request: &str) -> &str {
    request
        .lines()
        .find_map(|line| line.strip_prefix("Host: "))
        .unwrap_or("127.0.0.1")
        .trim()
}

fn host_without_port(host: &str) -> &str {
    if host.starts_with('[') {
        return host.find(']').map(|index| &host[..=index]).unwrap_or(host);
    }

    host.split(':').next().unwrap_or(host)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind(ADDRESS)?;
    println!("Serving status page at http://{ADDRESS}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => handle_connection(stream)?,
            Err(error) => eprintln!("Connection failed: {error}"),
        }
    }

    Ok(())
}

fn handle_connection(mut stream: TcpStream) -> std::io::Result<()> {
    let mut buffer = [0; 1024];
    let bytes_read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let first_line = request.lines().next().unwrap_or_default();
    let host = request_host(&request);

    let (status, body, content_type) =
        if first_line.starts_with("GET / ") || first_line.starts_with("GET /index.html ") {
            ("HTTP/1.1 200 OK", page(host), "text/html; charset=utf-8")
        } else {
            (
                "HTTP/1.1 404 Not Found",
                "Not found\n".to_owned(),
                "text/plain; charset=utf-8",
            )
        };

    let response = format!(
        "{status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    stream.write_all(response.as_bytes())?;
    stream.flush()
}
