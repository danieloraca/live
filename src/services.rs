use crate::util::{command, object, quote};
use std::collections::{BTreeMap, BTreeSet};

// Ports are fallbacks when systemd's main process is a wrapper (for example Docker).
const SERVICES: &[(&str, &str, u16)] = &[
    ("iploc.service", "IP Location", 3000),
    ("id-generator.service", "ID Generator", 3012),
    ("tetris.service", "Tetris", 3020),
    ("solitaire.service", "Solitaire", 3021),
    ("trader-dashboard.service", "Trader Dashboard", 3040),
    ("sym_notes.service", "Sym Notes", 3444),
    ("jirpi.service", "JiraPi", 5644),
    ("live.service", "Live Status", 9999),
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

pub fn collect() -> String {
    let mut args = vec![
        "show",
        "--no-pager",
        "--property=Id,ActiveState,MainPID,LoadState",
    ];
    args.extend(SERVICES.iter().map(|service| service.0));
    let units = properties(&command("systemctl", &args).unwrap_or_default());
    // TCP listeners only: a UDP socket is not necessarily a web endpoint.
    let sockets = command("ss", &["-H", "-ltnp"]).unwrap_or_default();
    let rows = SERVICES
        .iter()
        .map(|(unit, name, fallback)| {
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
            object(&[
                ("unit", quote(unit)),
                ("name", quote(name)),
                ("state", quote(state)),
                ("port", port.to_string()),
            ])
        })
        .collect::<Vec<_>>();
    format!("[{}]", rows.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn units_stay_associated_when_property_order_changes() {
        let data = properties(
            "ActiveState=active\nId=live.service\nMainPID=42\n\nId=jirpi.service\nActiveState=failed\nMainPID=0",
        );
        assert_eq!(data["live.service"]["ActiveState"], "active");
        assert_eq!(data["jirpi.service"]["ActiveState"], "failed");
    }
}
