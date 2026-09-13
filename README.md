# Pi Status

A lightweight Raspberry Pi dashboard served by a single Rust binary, with
bundled SQLite and no JavaScript framework, external fonts, or third-party requests.

The dashboard includes:

- Uptime, CPU usage and core count, available/used RAM, and SoC temperature.
- Persistent CPU and physical-network history, with 15-minute, one-hour,
  24-hour, 7-day, and 30-day views.
- Root filesystem usage, load averages, CPU clock, swap, and process count.
- Download/upload rates and interface transfer totals.
- Raspberry Pi throttling and undervoltage warnings when vcgencmd is available.
- All eight existing app links, with their systemd states and TCP ports.

## Run

Requires Rust 1.87 or later and a C compiler (for bundled SQLite).
No separate database server or SQLite package is required. On the Pi:

~~~sh
cargo build --release
./target/release/live
~~~

The default address is http://0.0.0.0:9999. Visit http://192.168.0.25:9999
from the local network. To preview on a different local port:

~~~sh
LIVE_ADDRESS=127.0.0.1:9998 cargo run
~~~

On macOS, Linux-only readings are explicitly unavailable. They are never
replaced with sample data. Disk usage can still be read locally.

## Data and refresh behavior

Hardware samples are collected every five seconds in one background sampler.
Service states, disk space, and Pi firmware flags are checked every 30 seconds.
The browser fetches cached readings; extra visitors do not trigger extra probes.

CPU percentage and physical-network download/upload rates are saved to SQLite
at their original five-second resolution, including timestamps and missing
readings. **History is kept indefinitely, with no automatic expiry or downsampling
on disk.** It accumulates with no page open and survives process and Pi restarts.
Readings collected before this feature was installed cannot be recovered.

The database defaults to `data/history.sqlite3` relative to the working directory.
With the existing systemd unit, this is
`/home/danutz/Development/live/data/history.sqlite3`. Rebuilding the binary leaves
it untouched, and `data/` is excluded from Git. Set `LIVE_HISTORY_DB` to override
the path; when systemd supplies `STATE_DIRECTORY`, the default is
`$STATE_DIRECTORY/history.sqlite3` instead. The service user must be able to
write to the database's directory, including its SQLite sidecar files.

~~~sh
LIVE_HISTORY_DB=/path/to/history.sqlite3 ./target/release/live
~~~

Writes are batched every 30 seconds in SQLite transactions with WAL and FULL
synchronization. The first sample is saved immediately; normal SIGTERM/SIGINT
shutdown flushes the remaining batch. A crash or power loss can lose the last
uncommitted batch (up to about 30 seconds). Pending samples are included in chart
queries. If saving fails, the dashboard warns and retries; a bounded buffer holds
the latest 720 unsaved samples (about an hour), and the page reports if any are
lost. A database that cannot be opened causes a clear startup failure rather
than silently starting an empty history.

The 15-minute and one-hour charts show original samples. Longer chart views use
1-minute, 10-minute, and 30-minute averages respectively to keep responses small;
the original rows remain in SQLite. Missing periods are left empty, although
averaged views cannot show gaps shorter than their bucket size. Hidden tabs
pause polling and reload saved history when reopened. Longer views reload every
30 seconds while current readings still refresh every five seconds.

Disk use grows as history accumulates. For a consistent backup, stop the service,
copy the database and any remaining `-wal`/`-shm` sidecars together, then restart
it, or use SQLite's online backup API. Do not copy just the main database file
while the service is running.

Read-only endpoints:

- GET /api/status: current metrics, machine details, services, and history save
  status (`state`, `persisted_through`, `pending_samples`, `dropped_samples`).
- GET /api/history?minutes=60: an object with `points` and `resolution_seconds`.
  Supported minute values: `15`, `60` (default), `1440`, `10080`, `43200`.
  Each point contains `timestamp`, `cpu`, `rx`, and `tx`. CPU is a percentage;
  network rates are bytes/second. Unsupported ranges return HTTP 400.
  This replaces the previous bare-array history response.

Linux data comes from /proc and /sys. RAM usage uses MemAvailable so reclaimable
cache is not mistaken for unavailable memory. CPU uses differences between
aggregate counters, excluding duplicate guest time. Network rates include
physical interfaces only, excluding loopback and Docker bridges/veth devices.
Transfers count bytes since each interface started; a reset starts a new rate
baseline. Root disk availability excludes reserved space.

Service state reflects systemd, not an HTTP health check. Ports are detected
from the main process's TCP listeners when permissions allow. Configured ports
remain fallbacks for wrapper/container services. Configure entries in
src/services.rs. Commands are bounded to two seconds; unavailable metrics
remain null. Request workers are bounded, and disconnected clients do not stop
the server.

No visitor tracking is enabled.

Metric definitions: [Linux proc documentation](https://docs.kernel.org/filesystems/proc.html)
and [Raspberry Pi firmware status](https://www.raspberrypi.com/documentation/computers/os.html#get_throttled).

## Verification

~~~sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
node --check static/app.js
python3 tests/history_restart.py
~~~

## Existing systemd deployment

The unit in deploy/live.service runs
/home/danutz/Development/live/target/release/live as danutz.

After copying the reviewed source changes to that checkout:

~~~sh
cd /home/danutz/Development/live
cargo test
cargo build --release
sudo systemctl restart live.service
systemctl is-active live.service
curl --fail http://127.0.0.1:9999/api/status
~~~

The static HTML, CSS, and JavaScript are embedded at compile time, so rebuild
the binary whenever they change. Optional vcgencmd permissions only affect
firmware warnings; no elevated privileges are required by the web server.
