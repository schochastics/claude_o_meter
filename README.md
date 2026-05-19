# claude_o_meter

A macOS menu bar app that shows Claude Code session + weekly quota usage at a glance. Rust rebuild of [JackBhanded/claude-meter](https://github.com/JackBhanded/claude-meter) (which targets Windows / Python).

The menu bar shows `● 47%` — a colored dot (blue → green → orange → red as utilization climbs) and the higher of session/weekly utilization. Click for the full breakdown: session bar with reset countdown, weekly bar, per-model split, refresh, launch-at-login, quit.

## Build

```sh
cargo bundle --release
codesign --force --deep --sign - target/release/bundle/osx/claude_o_meter.app
open target/release/bundle/osx/claude_o_meter.app
```

Or, with the helper:

```sh
./scripts/build_app.sh --open
```

Requires `cargo-bundle`:

```sh
cargo install cargo-bundle --locked
```

Running via `cargo run` works for development but **notifications will silently fail** and **launch-at-login cannot register** — both require the binary to live inside an ad-hoc-signed `.app` bundle.

## First run

1. Make sure you've logged into Claude Code at least once (`claude login`) so the OAuth token is in your Keychain. Verify with:
   ```sh
   security find-generic-password -s "Claude Code-credentials"
   ```
2. Launch the app. macOS will prompt:
   *"claude_o_meter wants to access your confidential information stored in 'Claude Code-credentials' in your keychain."*
   Click **Always Allow** so the app can poll unattended.
3. The dot turns from `…` (loading) to a color reflecting your usage.

## Token expiry

OAuth tokens issued by `claude login` expire (currently ~8h). When that happens the menu bar shows `?` and a notification fires: *"Claude Code token expired — run `claude login`"*. The app re-reads the Keychain on every poll, so logging in again refreshes the data without restarting the app.

The app does **not** attempt to use the refresh token — the OAuth refresh endpoint is undocumented and Claude Code itself doesn't refresh in the background.

## Data source

`GET https://api.anthropic.com/api/oauth/usage` with `Authorization: Bearer <oauth-token>` and `anthropic-beta: oauth-2025-04-20`. This is the same endpoint that backs `claude.ai/settings/usage`.

Default poll interval is 7 minutes; on rate-limit (HTTP 429) the app backs off exponentially, capped at 30 min and floored at 60 s regardless of what the server says in `Retry-After` (the endpoint has been known to return `Retry-After: 0`).

## Settings

Persisted to `~/Library/Application Support/com.cynkra.claude-o-meter/settings.json`:

```json
{
  "refresh_secs": 420,
  "idle_refresh_secs": 1200,
  "notify_session": true,
  "notify_weekly": true,
  "thresholds": [0.75, 0.9, 0.95]
}
```

Edit and restart the app to apply.

## Verification

```sh
cargo test        # 42 tests: unit + wiremock integration
cargo run --example dump_token   # prints token preview + expiry from Keychain
```

End-to-end: bundle, sign, open, watch one real poll:

```sh
./scripts/build_app.sh --open
log stream --predicate 'process == "claude_o_meter"'
```

## Tech

- [`tray-icon`](https://docs.rs/tray-icon) + [`tao`](https://docs.rs/tao) — NSStatusItem and event loop
- [`reqwest`](https://docs.rs/reqwest) (rustls) + [`tokio`](https://docs.rs/tokio) — polling
- [`security-framework`](https://docs.rs/security-framework) — Keychain access
- [`mac-notification-sys`](https://docs.rs/mac-notification-sys) — native NSUserNotifications
- [`smappservice-rs`](https://docs.rs/smappservice-rs) — SMAppService launch-at-login
- [`cargo-bundle`](https://github.com/burtonageo/cargo-bundle) — `.app` packaging
