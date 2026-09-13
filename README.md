# Pi Status

A lightweight Raspberry Pi dashboard served by a single Rust binary, with no
crate dependencies, JavaScript framework, external fonts, or third-party requests.

The dashboard includes:

- Uptime, CPU usage and core count, available/used RAM, and SoC temperature.
- CPU and physical-network history, with 15-minute and one-hour views.
- Root filesystem usage, load averages, CPU clock, swap, and process count.
- Download/upload rates and interface transfer totals.
- Raspberry Pi throttling and undervoltage warnings when vcgencmd is available.
- All eight existing app links, with their systemd states and TCP ports.

## Run

Requires Rust 1.87 or later. On the Pi:

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

History is held in a bounded one-hour ring buffer and resets when the status
process restarts. It accumulates even with no page open. Short histories are
shown at their actual timestamps, and gaps are not filled with invented data.
Hidden tabs pause polling and load current history when reopened.

Read-only endpoints:

- GET /api/status: current metrics, machine details, and services.
- GET /api/history: up to one hour of CPU and network samples.

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

No visitor tracking or persistent metric storage is enabled.

Metric definitions: [Linux proc documentation](https://docs.kernel.org/filesystems/proc.html)
and [Raspberry Pi firmware status](https://www.raspberrypi.com/documentation/computers/os.html#get_throttled).

## Verification

~~~sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
node --check static/app.js
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
