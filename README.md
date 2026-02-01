# Buechsentelefon

A simple, secure WebRTC audio chat server. Minimal, sufficient, and flexible enough for small groups.

## Philosophy

- **Minimal** — No accounts, no persistence of chat; just rooms and voice.
- **Sufficient** — TLS, optional room and server passwords, connection indicators, basic audio settings.
- **Flexible** — Optional branding (favicon, login logo, header/room banners, background image) and custom CSS to adapt look and feel.

## Requirements

- [Rust](https://rustup.rs/) (install via rustup; then `cargo` is available).

## Install

```bash
git clone https://github.com/thibran/buechsentelefon.git
cd buechsentelefon
cargo install --path .
```

Then run `buechsentelefon`; the binary is in your Cargo `bin` directory (e.g. `~/.cargo/bin/buechsentelefon`).

## Setup

1. **First run** — If no config exists, one is created and the server exits. Set a password:
   ```bash
   buechsentelefon set-password YOUR_PASSWORD
   ```
2. **Start the server** — Run `buechsentelefon` (no subcommand). Open the shown URL in a browser (e.g. `https://localhost:4433`), enter the password, set your display name and audio devices in Setup, then join a room.

### Config location

Config file: `config.toml`. By default it is created in the OS config directory:

| OS      | Path                                                        |
| ------- | ----------------------------------------------------------- |
| Linux   | `~/.config/buechsentelefon/config.toml`                     |
| macOS   | `~/Library/Application Support/buechsentelefon/config.toml` |
| Windows | `%APPDATA%\buechsentelefon\config.toml`                     |

Override with `buechsentelefon --config /path/to/config.toml`.

Optional: favicon, login logo, header/room banners, background image, and a path to custom CSS can be set in the `[branding]` section of `config.toml`.
