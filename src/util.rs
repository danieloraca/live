use std::fmt::Display;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim_matches(['\0', '\n', '\r', ' ']).to_owned())
}

pub fn command(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout.take(1_048_576).read_to_string(&mut output).ok()?;
        Some(output)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let finished = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let output = reader.join().ok()??;
    // systemctl can return a failure code for one missing unit while still
    // returning valid properties for the other units. Each caller parses data.
    (finished && !output.trim().is_empty()).then(|| output.trim().to_owned())
}

pub fn quote(value: &str) -> String {
    let mut result = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if c < '\u{20}' => result.push_str(&format!("\\u{:04x}", c as u32)),
            _ => result.push(c),
        }
    }
    result.push('"');
    result
}

pub fn number<T: Display>(value: Option<T>) -> String {
    value
        .map(|n| n.to_string())
        .unwrap_or_else(|| "null".into())
}

pub fn object(fields: &[(&str, String)]) -> String {
    format!(
        "{{{}}}",
        fields
            .iter()
            .map(|(key, value)| format!("{}:{value}", quote(key)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_escapes_control_characters_and_quotes() {
        assert_eq!(quote("a\"\\\n\0é"), "\"a\\\"\\\\\\n\\u0000é\"");
        assert_eq!(number::<u64>(None), "null");
    }

    #[test]
    fn preserves_partial_command_output_when_one_unit_is_missing() {
        assert_eq!(
            command(
                "sh",
                &[
                    "-c",
                    "printf 'Id=live.service\\nActiveState=active\\n'; exit 1"
                ]
            ),
            Some("Id=live.service\nActiveState=active".into())
        );
    }
}
