use crate::util::{number, object, quote};
use rusqlite::{Connection, params};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
const FLUSH_INTERVAL: Duration = Duration::from_secs(30);
// Keep live sampling bounded if the disk stays unavailable for a long time.
const PENDING_LIMIT: usize = 720;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub timestamp: u64,
    pub cpu: Option<f64>,
    pub rx: Option<f64>,
    pub tx: Option<f64>,
}

impl Point {
    pub fn json(&self) -> String {
        object(&[
            ("timestamp", self.timestamp.to_string()),
            ("cpu", number(self.cpu)),
            ("rx", number(self.rx)),
            ("tx", number(self.tx)),
        ])
    }
}

#[derive(Clone, Copy)]
pub struct Range {
    pub minutes: u64,
    pub resolution: u64,
}

impl Range {
    pub fn parse(query: &str) -> Option<Self> {
        let minutes = if query.is_empty() {
            60
        } else {
            query.strip_prefix("minutes=")?.parse().ok()?
        };
        let resolution = match minutes {
            15 | 60 => 5,
            1440 => 60,
            10080 => 600,
            43200 => 1800,
            _ => return None,
        };
        Some(Self {
            minutes,
            resolution,
        })
    }
}

pub fn database_path() -> PathBuf {
    if let Some(path) = std::env::var_os("LIVE_HISTORY_DB") {
        PathBuf::from(path)
    } else if let Some(directory) = std::env::var_os("STATE_DIRECTORY") {
        PathBuf::from(directory).join("history.sqlite3")
    } else {
        PathBuf::from("data/history.sqlite3")
    }
}

pub struct Store {
    connection: Connection,
    pending: VecDeque<Point>,
    last_flush: Instant,
    persisted_through: Option<u64>,
    write_failed: bool,
    dropped_samples: u64,
}

#[derive(Default, Debug, PartialEq)]
struct Summary {
    sum: f64,
    count: u64,
    peak: Option<f64>,
}

impl Summary {
    fn record(&mut self, value: Option<f64>) {
        if let Some(value) = value {
            self.sum += value;
            self.count += 1;
            self.peak = Some(self.peak.map_or(value, |p| p.max(value)));
        }
    }

    fn json(&self) -> String {
        object(&[
            (
                "average",
                number((self.count > 0).then(|| self.sum / self.count as f64)),
            ),
            ("peak", number(self.peak)),
            ("samples", self.count.to_string()),
        ])
    }
}

#[derive(Debug, PartialEq)]
struct History {
    points: Vec<Point>,
    summary: [Summary; 3],
}

struct Bucket {
    timestamp: u64,
    sums: [f64; 3],
    counts: [u64; 3],
}

impl Bucket {
    fn push(buckets: &mut Vec<Self>, point: Point, resolution: u64) {
        if buckets
            .last()
            .is_none_or(|b| b.timestamp / resolution != point.timestamp / resolution)
        {
            buckets.push(Self {
                timestamp: point.timestamp,
                sums: [0.0; 3],
                counts: [0; 3],
            });
        }
        let bucket = buckets.last_mut().unwrap();
        bucket.timestamp = point.timestamp;
        for (i, value) in [point.cpu, point.rx, point.tx].into_iter().enumerate() {
            if let Some(value) = value {
                bucket.sums[i] += value;
                bucket.counts[i] += 1;
            }
        }
    }

    fn point(self) -> Point {
        let average = |i| {
            if self.counts[i] == 0 {
                None
            } else {
                Some(self.sums[i] / self.counts[i] as f64)
            }
        };
        Point {
            timestamp: self.timestamp,
            cpu: average(0),
            rx: average(1),
            tx: average(2),
        }
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if path.as_os_str().is_empty() || path == Path::new(":memory:") {
            return Err("LIVE_HISTORY_DB must name an on-disk database".into());
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(1))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 1 {
            return Err(format!("Unsupported history database version {version}").into());
        }
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA cache_size=-2048;
             BEGIN;
             CREATE TABLE IF NOT EXISTS samples (
                 timestamp INTEGER PRIMARY KEY CHECK(timestamp >= 0),
                 cpu REAL,
                 rx REAL,
                 tx REAL
             );
             PRAGMA user_version=1;
             COMMIT;",
        )?;
        let persisted_through =
            connection.query_row("SELECT MAX(timestamp) FROM samples", [], |r| {
                Ok(r.get::<_, Option<i64>>(0)?.map(|t| t as u64))
            })?;
        Ok(Self {
            connection,
            pending: VecDeque::new(),
            last_flush: Instant::now(),
            persisted_through,
            write_failed: false,
            dropped_samples: 0,
        })
    }

    pub fn record(&mut self, point: Point) {
        self.pending.retain(|p| p.timestamp != point.timestamp);
        if self.pending.len() == PENDING_LIMIT {
            self.pending.pop_front();
            self.dropped_samples += 1;
        }
        self.pending.push_back(point);
        if self.persisted_through.is_none() || self.last_flush.elapsed() >= FLUSH_INTERVAL {
            match self.flush() {
                Ok(()) => {}
                Err(error) => {
                    if !self.write_failed {
                        eprintln!(
                            "History could not be saved; retaining recent samples in memory: {error}"
                        );
                    }
                    self.write_failed = true;
                }
            }
        }
    }

    pub fn flush(&mut self) -> rusqlite::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let transaction = self.connection.transaction()?;
        {
            let mut insert = transaction.prepare(
                "INSERT INTO samples (timestamp, cpu, rx, tx) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(timestamp) DO UPDATE SET cpu=excluded.cpu, rx=excluded.rx, tx=excluded.tx",
            )?;
            for point in &self.pending {
                insert.execute(params![
                    point.timestamp as i64,
                    point.cpu,
                    point.rx,
                    point.tx
                ])?;
            }
        }
        transaction.commit()?;
        self.persisted_through = self
            .pending
            .iter()
            .map(|p| p.timestamp)
            .chain(self.persisted_through)
            .max();
        self.pending.clear();
        self.last_flush = Instant::now();
        if self.write_failed {
            eprintln!("History storage recovered");
        }
        self.write_failed = false;
        Ok(())
    }

    pub fn status_json(&self) -> String {
        object(&[
            (
                "state",
                quote(if self.write_failed { "error" } else { "ok" }),
            ),
            ("persisted_through", number(self.persisted_through)),
            ("pending_samples", self.pending.len().to_string()),
            ("dropped_samples", self.dropped_samples.to_string()),
        ])
    }

    fn query(&self, range: Range, end: u64) -> rusqlite::Result<History> {
        let start = end.saturating_sub(range.minutes * 60);
        let resolution = if range.resolution == 5 {
            1
        } else {
            range.resolution
        };
        let mut pending = self
            .pending
            .iter()
            .copied()
            .filter(|p| p.timestamp >= start && p.timestamp <= end)
            .collect::<Vec<_>>();
        pending.sort_unstable_by_key(|p| p.timestamp);
        let mut pending = pending.into_iter().peekable();
        // Stream the primary-key range and aggregate into small in-memory buckets.
        // SQL GROUP BY on timestamp / resolution would sort all raw rows and can
        // spill to a temporary disk file when requesting a month of readings.
        let mut statement = self.connection.prepare(
            "SELECT timestamp, cpu, rx, tx FROM samples WHERE timestamp BETWEEN ?1 AND ?2 ORDER BY timestamp",
        )?;
        let rows = statement.query_map(params![start as i64, end as i64], |row| {
            Ok(Point {
                timestamp: row.get::<_, i64>(0)? as u64,
                cpu: row.get(1)?,
                rx: row.get(2)?,
                tx: row.get(3)?,
            })
        })?;
        let mut buckets = Vec::new();
        let mut summary: [Summary; 3] = Default::default();
        let mut record = |point: Point| {
            for (stat, value) in summary.iter_mut().zip([point.cpu, point.rx, point.tx]) {
                stat.record(value);
            }
            Bucket::push(&mut buckets, point, resolution);
        };
        for row in rows {
            let point = row?;
            let mut replaced = false;
            while pending
                .peek()
                .is_some_and(|p| p.timestamp <= point.timestamp)
            {
                let newer = pending.next().unwrap();
                replaced = newer.timestamp == point.timestamp;
                record(newer);
            }
            if !replaced {
                record(point);
            }
        }
        for point in pending {
            record(point);
        }
        Ok(History {
            points: buckets.into_iter().map(Bucket::point).collect(),
            summary,
        })
    }

    pub fn query_json(&self, range: Range, end: u64) -> rusqlite::Result<String> {
        let history = self.query(range, end)?;
        Ok(object(&[
            (
                "points",
                format!(
                    "[{}]",
                    history
                        .points
                        .iter()
                        .map(Point::json)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            ),
            ("resolution_seconds", range.resolution.to_string()),
            (
                "summary",
                object(&[
                    ("cpu", history.summary[0].json()),
                    ("rx", history.summary[1].json()),
                    ("tx", history.summary[2].json()),
                ]),
            ),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(timestamp: u64, cpu: Option<f64>) -> Point {
        Point {
            timestamp,
            cpu,
            rx: Some(100.0),
            tx: None,
        }
    }

    #[test]
    fn disk_history_survives_reopen_and_does_not_expire() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        let old = point(100, Some(20.0));
        let recent = point(10_000_000, Some(40.0));
        {
            let mut store = Store::open(&path).unwrap();
            store.record(old);
            store.record(recent);
            store.flush().unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.query(Range::parse("").unwrap(), 100).unwrap().points,
            vec![old]
        );
        assert_eq!(
            store
                .query(Range::parse("").unwrap(), recent.timestamp)
                .unwrap()
                .points,
            vec![recent]
        );
        let integrity: String = store
            .connection
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    #[test]
    fn pending_samples_are_visible_and_averaged_once_before_and_after_commit() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("history.sqlite3")).unwrap();
        store.record(point(120, Some(10.0)));
        store.record(point(120, Some(30.0)));
        store.record(point(125, Some(50.0)));
        store.record(point(130, None));
        let range = Range::parse("minutes=1440").unwrap();
        let before = store.query(range, 130).unwrap();
        assert_eq!(before.points, vec![point(130, Some(40.0))]);
        assert_eq!(before.summary[0].peak, Some(50.0));
        assert_eq!(before.summary[0].count, 2);
        let saved: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(saved, 1);
        store.flush().unwrap();
        assert_eq!(store.query(range, 130).unwrap(), before);
    }

    #[test]
    fn failed_batch_rolls_back_and_remains_available_for_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("history.sqlite3")).unwrap();
        store.record(point(100, Some(10.0)));
        store.record(point(105, Some(20.0)));
        store.record(point(110, Some(30.0)));
        store.connection.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON samples WHEN NEW.timestamp = 110 BEGIN SELECT RAISE(FAIL, 'test full disk'); END;").unwrap();
        assert!(store.flush().is_err());
        assert_eq!(store.pending.len(), 2);
        let saved: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(saved, 1);
        store
            .connection
            .execute_batch("DROP TRIGGER fail_insert")
            .unwrap();
        store.flush().unwrap();
        assert_eq!(
            store
                .query(Range::parse("").unwrap(), 110)
                .unwrap()
                .points
                .len(),
            3
        );
    }

    #[test]
    fn queries_bound_the_window_and_keep_missing_intervals_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("history.sqlite3")).unwrap();
        for timestamp in (0..=9000).step_by(5) {
            store.record(point(timestamp, Some(25.0)));
            if timestamp % 300 == 0 {
                store.flush().unwrap();
            }
        }
        store.flush().unwrap();
        let hour = store.query(Range::parse("").unwrap(), 9000).unwrap();
        assert_eq!(hour.points.len(), 721);
        assert_eq!(hour.points[0].timestamp, 5400);
        store.record(point(10000, None));
        let day = store
            .query(Range::parse("minutes=1440").unwrap(), 10000)
            .unwrap();
        assert!(day.points.len() <= 1441);
        assert_eq!(day.points.last().unwrap().cpu, None);
        assert_eq!(day.points[day.points.len() - 2].timestamp, 9000);
        assert!(Range::parse("minutes=99999999").is_none());
        assert!(Range::parse("minutes=60&minutes=15").is_none());
    }

    #[test]
    fn month_view_keeps_all_original_rows_and_returns_small_averaged_results() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("history.sqlite3")).unwrap();
        store
            .connection
            .execute_batch(
                "WITH RECURSIVE ticks(t) AS (
                 SELECT 0 UNION ALL SELECT t + 5 FROM ticks WHERE t < 2592000
             ) INSERT INTO samples SELECT t, t % 100, 100, NULL FROM ticks;",
            )
            .unwrap();
        let month = store
            .query(Range::parse("minutes=43200").unwrap(), 2_592_000)
            .unwrap();
        assert_eq!(month.points.len(), 1441);
        assert_eq!(month.points[0], point(1795, Some(47.5)));
        let rows: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 518_401);
    }

    #[test]
    fn prolonged_write_failure_is_bounded_reported_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("history.sqlite3")).unwrap();
        store.record(point(100, Some(10.0)));
        store.last_flush = Instant::now() - FLUSH_INTERVAL;
        store
            .connection
            .execute_batch("PRAGMA query_only=ON")
            .unwrap();
        for timestamp in 101..=824 {
            store.record(point(timestamp, Some(20.0)));
        }
        assert_eq!(store.pending.len(), PENDING_LIMIT);
        assert_eq!(store.dropped_samples, 4);
        assert!(store.status_json().contains("\"state\":\"error\""));
        store
            .connection
            .execute_batch("PRAGMA query_only=OFF")
            .unwrap();
        store.flush().unwrap();
        assert!(store.pending.is_empty());
        assert!(store.status_json().contains("\"state\":\"ok\""));
        let rows: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 721);
    }

    #[test]
    fn period_summary_uses_raw_samples_not_averages_of_unequal_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("history.sqlite3")).unwrap();
        store.record(point(5, Some(100.0)));
        store.record(point(60, Some(0.0)));
        store.record(point(65, Some(0.0)));
        store.record(point(70, Some(0.0)));
        store.record(point(75, None));
        let result = store
            .query(Range::parse("minutes=1440").unwrap(), 75)
            .unwrap();
        assert_eq!(result.points.len(), 2);
        assert_eq!(result.summary[0].count, 4);
        assert_eq!(result.summary[0].sum / result.summary[0].count as f64, 25.0);
        assert_eq!(result.summary[0].peak, Some(100.0));
        assert_eq!(result.summary[2].count, 0);
        assert_eq!(result.summary[2].peak, None);
    }
}
