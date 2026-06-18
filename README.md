# live

Tiny Rust status page intended to run on a Raspberry Pi.

The page shows the current `systemd` state and listening ports for:

- `iploc.service`
- `live.service`
- `sym_notes.service`

## Run

```sh
cargo run
```

The server listens on `0.0.0.0:9999`, so from another device on the same network open:

```text
http://<raspberry-pi-ip>:9999
```
