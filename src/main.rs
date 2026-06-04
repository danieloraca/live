use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

const ADDRESS: &str = "0.0.0.0:9999";

const PAGE: &str = r#"<!doctype html>
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
      display: grid;
      place-items: center;
      padding: 24px;
      box-sizing: border-box;
    }

    main {
      width: min(100%, 560px);
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
  </style>
</head>
<body>
  <main>
    <div class="status"><span class="dot"></span> Online</div>
    <h1>Raspberry Pi Status</h1>
    <p>This page is being served by a small Rust process on port 9999.</p>
  </main>
</body>
</html>
"#;

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

    let (status, body, content_type) = if first_line.starts_with("GET / ")
        || first_line.starts_with("GET /index.html ")
    {
        ("HTTP/1.1 200 OK", PAGE, "text/html; charset=utf-8")
    } else {
        ("HTTP/1.1 404 Not Found", "Not found\n", "text/plain; charset=utf-8")
    };

    let response = format!(
        "{status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    stream.write_all(response.as_bytes())?;
    stream.flush()
}
