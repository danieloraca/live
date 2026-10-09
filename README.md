# Pi Status

A lightweight Raspberry Pi dashboard served by a single Rust binary, with
bundled SQLite and no JavaScript framework, external fonts, or third-party requests.

The dashboard includes:

- Uptime, CPU usage and core count, available/used RAM, and SoC temperature.
- Persistent CPU, physical-network, temperature, CPU-clock, and hardware-alert
  history, with 15-minute, one-hour, 24-hour, 7-day, and 30-day views.
- Root filesystem usage, load averages, CPU clock, swap, and process count.
- Download/upload rates and interface transfer totals.
- Raspberry Pi throttling and undervoltage warnings when vcgencmd is available.
- Configured app links, including Elite on port 3141, with their systemd states,
  HTTP response times, and last replies.

## Run

Requires Rust 1.88 or later and a C compiler (for bundled SQLite).
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

## Terminal dashboard

With the web service running, use the same binary to read its cached data:

~~~sh
./target/release/live status
./target/release/live watch
~~~

`status` prints a snapshot suited to SSH sessions and scripts. `watch` opens a
Ratatui dashboard with current vitals, 15-minute trends, service health, and
history warnings. Press `q` or Escape to quit, `r` to refresh, and the arrow keys
or `j`/`k` to browse services. The terminal view refreshes status every five
seconds and trends every 30 seconds. It needs an interactive terminal; `status`
does not.

Both commands connect to `127.0.0.1:9999` by default. If the service listens
elsewhere, pass `--address HOST:PORT` or set `LIVE_CLI_ADDRESS`. For example:

~~~sh
./target/release/live watch --address 127.0.0.1:9998
~~~

The CLI does not start a second sampler or write another history database. Run
`live --help` for command usage.

On macOS, Linux-only readings are explicitly unavailable. They are never
replaced with sample data. Disk usage can still be read locally.

## Data and refresh behavior

Hardware samples are collected every five seconds in one background sampler.
Service states and HTTP responses are checked once a minute; disk space is
checked every 30 seconds.
Pi firmware temperature, actual ARM clock, and throttle flags are sampled every
five seconds when `vcgencmd` is available; `/sys` provides temperature and
clock fallbacks.
The browser fetches cached readings; extra visitors do not trigger extra probes.

CPU percentage, physical-network download/upload rates, temperature, clock, and
throttle flags are saved to SQLite at their original five-second resolution,
including timestamps and missing readings. **History is kept indefinitely, with
no automatic expiry or downsampling on disk.** It accumulates with no page open
and survives process and Pi restarts. Existing databases migrate in place;
temperature, clock, and alert history starts when this version is installed.

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
pause polling and reload saved history when reopened. The 15-minute and
one-hour charts reload every 30 seconds, the 24-hour chart every minute, the
7-day chart every two minutes, and the 30-day chart every five minutes while
the tab is visible. Current readings still refresh every five seconds.

The charts show labeled scales, local timestamps, and period summaries.
CPU stays on a fixed 0–100% scale; the network scale adapts to traffic and labels
its units. The thermal chart uses aligned temperature and ARM clock tracks with
separate axes; red markers show samples with active throttle, temperature, or
undervoltage flags. The firmware's latched "occurred since boot" bits are retained
but do not create event markers with an invented timestamp. Period summaries use original
samples, excluding missing readings, so peaks remain accurate even when the
plotted line is averaged. Hover or tap to inspect a reading, or focus the chart
and use arrow keys, Home, and End. Escape clears the selection. Averaged readings
identify their time bucket. Unrecorded periods are marked as missing rather than
drawn as zero.

Disk use grows as history accumulates. For a consistent backup, stop the service,
copy the database and any remaining `-wal`/`-shm` sidecars together, then restart
it, or use SQLite's online backup API. Do not copy just the main database file
while the service is running.

Read-only endpoints:

- GET /api/status: current metrics, machine details, services, and history save
  status (`state`, `persisted_through`, `pending_samples`, `dropped_samples`).
- GET /api/history?minutes=60: an object with `points`, `resolution_seconds`, and
  `summary` (per-metric `average`, `peak`, and valid `samples` count).
  Supported minute values: `15`, `60` (default), `1440`, `10080`, `43200`.
  Each point contains `timestamp`, `cpu`, `rx`, `tx`, `temperature`, `frequency`,
  and `throttled`. CPU is a percentage; network rates are bytes/second;
  temperature is °C and ARM clock is MHz. Unsupported ranges return HTTP 400.
  This replaces the previous bare-array history response.

Linux data comes from /proc and /sys. RAM usage uses MemAvailable so reclaimable
cache is not mistaken for unavailable memory. CPU uses differences between
aggregate counters, excluding duplicate guest time. Network rates include
physical interfaces only, excluding loopback and Docker bridges/veth devices.
Transfers count bytes since each interface started; a reset starts a new rate
baseline. Root disk availability excludes reserved space.

Service state reflects systemd. For each active service, the dashboard also
requests its configured health path over HTTP on the address bound by its local
listener, with a bounded timeout. JiraPi uses `/healthz`; other apps use `/`.
A 2xx–4xx response updates its last reply; 5xx responses and timeouts warn.
The 404 at IP Location's API-only root is therefore visible without marking
the service unreachable. Last-reply times reset when this dashboard restarts.
Ports are detected from the main process's TCP listeners when
permissions allow. Configured ports remain fallbacks for wrapper/container
services. Configure entries in src/services.rs. System commands are bounded to
two seconds; unavailable metrics remain null. Request workers are bounded, and
disconnected clients do not stop the server.

No visitor tracking is enabled.

Metric definitions: [Linux proc documentation](https://docs.kernel.org/filesystems/proc.html)
and [Raspberry Pi firmware status](https://www.raspberrypi.com/documentation/computers/os.html#get_throttled).

## Verification

~~~sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
node --check static/app.js
node --test tests/charts.test.mjs
python3 tests/history_restart.py
~~~

## Existing systemd deployment

The unit in deploy/live.service runs
/home/danutz/Development/live/target/release/live as danutz.

Once the deployment script is available in the Pi checkout, run it as the
normal `danutz` user (not with `sudo`):

~~~sh
cd /home/danutz/Development/live
./deploy/update-pi.sh
~~~

For the first deployment of this script, run `git pull --ff-only` manually to
get it onto the Pi. After that, the script pulls with fast-forward only, runs
the Rust tests, rebuilds the release binary, restarts `live.service`, and waits
for the status API to respond. The static HTML, CSS, and JavaScript are embedded
at compile time, so rebuilding is required whenever they change. Optional
vcgencmd permissions only affect firmware warnings; no elevated privileges are
required by the web server.
