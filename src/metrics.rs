use crate::util::{command, now, number, object, quote, read};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
struct Cpu {
    total: u64,
    idle: u64,
}

impl Cpu {
    fn parse(input: &str) -> Option<Self> {
        let fields = input
            .lines()
            .find(|line| line.starts_with("cpu "))?
            .split_whitespace()
            .skip(1)
            .take(8)
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        if fields.len() < 4 {
            return None;
        }
        // Guest time is already included in user/nice; do not count it twice.
        Some(Self {
            total: fields.iter().sum(),
            idle: fields[3] + fields.get(4).copied().unwrap_or(0),
        })
    }

    fn usage(self, before: Self) -> Option<f64> {
        let total = self.total.checked_sub(before.total)?;
        if total == 0 {
            return None;
        }
        let idle = self.idle.saturating_sub(before.idle).min(total);
        Some((total - idle) as f64 / total as f64 * 100.0)
    }
}

#[derive(Clone, Copy, Debug)]
struct Memory {
    total: u64,
    available: u64,
    swap_total: u64,
    swap_free: u64,
}

impl Memory {
    fn parse(input: &str) -> Option<Self> {
        let values: BTreeMap<&str, u64> = input
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                Some((
                    key,
                    value
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()?
                        .checked_mul(1024)?,
                ))
            })
            .collect();
        let total = *values.get("MemTotal")?;
        if total == 0 {
            return None;
        }
        // MemAvailable includes reclaimable cache; MemFree alone overstates usage.
        Some(Self {
            total,
            available: (*values.get("MemAvailable")?).min(total),
            swap_total: *values.get("SwapTotal").unwrap_or(&0),
            swap_free: *values.get("SwapFree").unwrap_or(&0),
        })
    }

    fn json(self) -> String {
        object(&[
            ("total", self.total.to_string()),
            ("available", self.available.to_string()),
            ("used", (self.total - self.available).to_string()),
            (
                "percent",
                ((self.total - self.available) as f64 / self.total as f64 * 100.0).to_string(),
            ),
            ("swap_total", self.swap_total.to_string()),
            (
                "swap_used",
                self.swap_total.saturating_sub(self.swap_free).to_string(),
            ),
        ])
    }
}

#[derive(Clone, Copy)]
struct Disk {
    total: u64,
    used: u64,
    available: u64,
}

impl Disk {
    fn parse(output: &str) -> Option<Self> {
        let fields = output
            .lines()
            .last()?
            .split_whitespace()
            .collect::<Vec<_>>();
        if fields.len() < 6 {
            return None;
        }
        let total = fields[1].parse::<u64>().ok()?.checked_mul(1024)?;
        if total == 0 {
            return None;
        }
        Some(Self {
            total,
            used: fields[2].parse::<u64>().ok()?.checked_mul(1024)?,
            available: fields[3].parse::<u64>().ok()?.checked_mul(1024)?,
        })
    }
    fn json(self) -> String {
        object(&[
            ("total", self.total.to_string()),
            ("used", self.used.to_string()),
            ("available", self.available.to_string()),
            (
                "percent",
                (self.used as f64 / self.total as f64 * 100.0).to_string(),
            ),
        ])
    }
}

type Counters = BTreeMap<String, (u64, u64)>;

fn network(input: &str, physical: impl Fn(&str) -> bool) -> Counters {
    input
        .lines()
        .filter_map(|line| {
            let (name, fields) = line.split_once(':')?;
            let name = name.trim();
            if !physical(name) {
                return None;
            }
            let fields = fields.split_whitespace().collect::<Vec<_>>();
            Some((
                name.to_owned(),
                (fields.first()?.parse().ok()?, fields.get(8)?.parse().ok()?),
            ))
        })
        .collect()
}

fn network_rate(current: &Counters, before: &Counters, seconds: f64) -> Option<(f64, f64)> {
    if seconds <= 0.0 {
        return None;
    }
    let deltas = current
        .iter()
        .filter_map(|(name, &(rx, tx))| {
            let &(old_rx, old_tx) = before.get(name)?;
            Some((rx.checked_sub(old_rx)?, tx.checked_sub(old_tx)?))
        })
        .collect::<Vec<_>>();
    if deltas.is_empty() {
        return None;
    }
    Some((
        deltas.iter().map(|v| v.0 as f64).sum::<f64>() / seconds,
        deltas.iter().map(|v| v.1 as f64).sum::<f64>() / seconds,
    ))
}

fn scalar(path: &str) -> Option<f64> {
    read(path)?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0)
}

pub struct Snapshot {
    timestamp: u64,
    uptime: Option<f64>,
    cpu: Option<f64>,
    cores: usize,
    memory: Option<Memory>,
    temperature: Option<f64>,
    frequency: Option<f64>,
    disk: Option<Disk>,
    load: Option<Vec<f64>>,
    processes: Option<usize>,
    interfaces: Vec<String>,
    received: Option<u64>,
    sent: Option<u64>,
    rx: Option<f64>,
    tx: Option<f64>,
    throttled: Option<u32>,
}

impl Snapshot {
    pub fn point_json(&self) -> String {
        object(&[
            ("timestamp", self.timestamp.to_string()),
            ("cpu", number(self.cpu)),
            ("rx", number(self.rx)),
            ("tx", number(self.tx)),
        ])
    }

    pub fn json(&self) -> String {
        object(&[
            ("timestamp", self.timestamp.to_string()),
            ("uptime", number(self.uptime)),
            ("cpu", number(self.cpu)),
            ("cores", self.cores.to_string()),
            (
                "memory",
                self.memory
                    .map(Memory::json)
                    .unwrap_or_else(|| "null".into()),
            ),
            ("temperature", number(self.temperature)),
            ("frequency", number(self.frequency)),
            (
                "disk",
                self.disk.map(Disk::json).unwrap_or_else(|| "null".into()),
            ),
            (
                "load",
                self.load
                    .as_ref()
                    .map(|l| {
                        format!(
                            "[{}]",
                            l.iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    })
                    .unwrap_or_else(|| "null".into()),
            ),
            ("processes", number(self.processes)),
            (
                "interfaces",
                format!(
                    "[{}]",
                    self.interfaces
                        .iter()
                        .map(|s| quote(s))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            ),
            ("received", number(self.received)),
            ("sent", number(self.sent)),
            ("rx", number(self.rx)),
            ("tx", number(self.tx)),
            ("throttled", number(self.throttled)),
        ])
    }
}

pub struct Collector {
    pub system: String,
    previous_cpu: Option<Cpu>,
    previous_network: Counters,
    previous_at: Instant,
    disk: Option<Disk>,
    throttled: Option<u32>,
}

impl Collector {
    pub fn new() -> Self {
        let os = read("/etc/os-release").and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_owned())
            })
        });
        let model =
            read("/proc/device-tree/model").or_else(|| read("/sys/firmware/devicetree/base/model"));
        Self {
            system: object(&[
                (
                    "model",
                    model.as_deref().map(quote).unwrap_or_else(|| "null".into()),
                ),
                (
                    "os",
                    os.as_deref()
                        .map(quote)
                        .unwrap_or_else(|| quote(std::env::consts::OS)),
                ),
                (
                    "kernel",
                    read("/proc/sys/kernel/osrelease")
                        .as_deref()
                        .map(quote)
                        .unwrap_or_else(|| "null".into()),
                ),
                ("architecture", quote(std::env::consts::ARCH)),
            ]),
            previous_cpu: None,
            previous_network: BTreeMap::new(),
            previous_at: Instant::now(),
            disk: None,
            throttled: None,
        }
    }

    pub fn collect(&mut self, slow: bool) -> Snapshot {
        if slow {
            // POSIX 1 KiB output works on Linux and on the local development machine.
            self.disk = command("df", &["-Pk", "/"]).and_then(|s| Disk::parse(&s));
            self.throttled = command("vcgencmd", &["get_throttled"])
                .and_then(|s| u32::from_str_radix(s.strip_prefix("throttled=0x")?, 16).ok());
        }
        let at = Instant::now();
        let stat = read("/proc/stat");
        let cpu = stat.as_deref().and_then(Cpu::parse);
        let counters = read("/proc/net/dev")
            .map(|s| {
                network(&s, |name| {
                    Path::new(&format!("/sys/class/net/{name}/device")).exists()
                })
            })
            .unwrap_or_default();
        let rate = network_rate(
            &counters,
            &self.previous_network,
            at.duration_since(self.previous_at).as_secs_f64(),
        );
        let load = read("/proc/loadavg").and_then(|s| {
            let v = s
                .split_whitespace()
                .take(3)
                .map(str::parse::<f64>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            (v.len() == 3 && v.iter().all(|n| n.is_finite() && *n >= 0.0)).then_some(v)
        });
        let snapshot = Snapshot {
            timestamp: now(),
            uptime: scalar("/proc/uptime"),
            cpu: cpu
                .zip(self.previous_cpu)
                .and_then(|(new, old)| new.usage(old)),
            cores: stat
                .as_ref()
                .map(|s| {
                    s.lines()
                        .filter(|line| {
                            line.strip_prefix("cpu")
                                .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()))
                        })
                        .count()
                })
                .unwrap_or(0),
            memory: read("/proc/meminfo").and_then(|s| Memory::parse(&s)),
            temperature: scalar("/sys/class/thermal/thermal_zone0/temp").map(|n| n / 1000.0),
            frequency: scalar("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")
                .map(|n| n / 1000.0),
            disk: self.disk,
            load,
            processes: std::fs::read_dir("/proc").ok().map(|dirs| {
                dirs.flatten()
                    .filter(|d| {
                        d.file_name()
                            .to_string_lossy()
                            .chars()
                            .all(|c| c.is_ascii_digit())
                    })
                    .count()
            }),
            interfaces: counters.keys().cloned().collect(),
            received: (!counters.is_empty()).then(|| counters.values().map(|v| v.0).sum()),
            sent: (!counters.is_empty()).then(|| counters.values().map(|v| v.1).sum()),
            rx: rate.map(|v| v.0),
            tx: rate.map(|v| v.1),
            throttled: self.throttled,
        };
        self.previous_cpu = cpu;
        self.previous_network = counters;
        self.previous_at = at;
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_excludes_guest_and_handles_counter_resets() {
        let old = Cpu::parse("cpu 100 10 20 800 20 10 10 30 90 5").unwrap();
        assert_eq!(old.total, 1000);
        let new = Cpu::parse("cpu 120 10 30 860 20 10 10 40 90 5").unwrap();
        assert_eq!(new.usage(old), Some(40.0));
        assert_eq!(old.usage(new), None);
        assert_eq!(old.usage(old), None);
        assert!(Cpu::parse("cpu bad").is_none());
    }

    #[test]
    fn memory_uses_available_and_handles_absent_swap() {
        let m = Memory::parse("MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 600 kB").unwrap();
        assert_eq!(m.total - m.available, 400 * 1024);
        assert_eq!(m.swap_total, 0);
        assert!(Memory::parse("MemTotal: 0 kB\nMemAvailable: 0 kB").is_none());
        assert!(Memory::parse("MemTotal: 1000 kB").is_none());
    }

    #[test]
    fn network_ignores_virtual_interfaces_and_counter_resets() {
        let counters = network(
            "eth0: 100 0 0 0 0 0 0 0 200 0\nlo: 900 0 0 0 0 0 0 0 900 0\nveth0: 500 0 0 0 0 0 0 0 500 0",
            |n| n == "eth0",
        );
        assert_eq!(counters.len(), 1);
        let current = BTreeMap::from([("eth0".into(), (200, 400))]);
        assert_eq!(network_rate(&current, &counters, 5.0), Some((20.0, 40.0)));
        assert_eq!(network_rate(&counters, &current, 5.0), None);
        assert_eq!(network_rate(&current, &BTreeMap::new(), 5.0), None);
    }

    #[test]
    fn disk_preserves_available_space_excluding_reserved_blocks() {
        let disk = Disk::parse("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/root 1000 300 650 32% /").unwrap();
        assert_eq!(disk.total, 1024000);
        assert_eq!(disk.available, 665600);
        assert!(Disk::parse("unavailable").is_none());
    }
}
