# AlertWim

A tiny, good-looking local log window for [BoardDo](https://github.com/igornet0/BoardDo) scenarios.
Scenarios send alerts to it; it shows them as a live, filterable feed.

- One window: time · level · title · source · message, expandable JSON data.
- Filters by level, search (⌘K), clear (⌘L), pin on top, auto-scroll with a "new" badge.
- Dark and light themes follow the system.
- Starts once: launching it again only brings the running window to front.

## How BoardDo connects

On start AlertWim listens on `127.0.0.1:47821` (`ALERTWIM_PORT`, falls back to a free port) and writes
`~/.alertwim/endpoint.json`:

```json
{ "app": "alertwim", "url": "http://127.0.0.1:47821", "token": "…", "pid": 123, "version": "0.1.0", "exe": "…" }
```

BoardDo reads this file and creates its `AlertWim` connection automatically. The token is generated
once and kept in `~/.alertwim/config.json` (mode 600), so the connection survives restarts. The
BoardDo node **AlertWim message** (`alertwim.alert`) starts the app if it is closed and writes a line.

## HTTP API

Every route except `/health` needs `Authorization: Bearer <token>`.

| Method | Path | Body / result |
|--------|------|---------------|
| GET | `/health` | `{app, version, pid}` |
| POST | `/v1/alerts` | `{message, level?, title?, source?, data?, show? = true, focus? = false}` → `{ok, id, ts}` |
| GET | `/v1/alerts` | `{alerts: [...]}` (last 2000) |
| POST | `/v1/show` | bring the window to front |
| POST | `/v1/clear` | clear the feed |

`level`: `info` · `success` · `warn` · `error` · `debug`.

```bash
TOKEN=$(python3 -c 'import json,os;print(json.load(open(os.path.expanduser("~/.alertwim/endpoint.json")))["token"])')
curl -s -X POST http://127.0.0.1:47821/v1/alerts -H "Authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' -d '{"level":"warn","title":"Hello","message":"from curl"}'
```

## Build

Rust + Tauri 2, no Node toolchain: the UI is plain HTML/CSS/JS in `ui/` and is embedded at build time.

```bash
cargo run                 # dev
cargo build --release     # target/release/alertwim (what BoardDo installs)
cargo tauri build         # macOS .app / .dmg (needs tauri-cli)
```

## License

MIT
