# irust

A terminal IRC client written in Rust. I wrote it for [ChatNet](https://chatnet.gg),
but it works on any IRC network. The commands follow irssi closely, so if you
know irssi you already know most of it.

## What it does

- **Commands:** messaging, channels, moderation, whois/who/list, windows,
  aliases, key bindings, `/exec`, `/eval`, `/dcc`, ignore and highlight rules.
  Type `/cmds` for the full list and `/cmds <command>` for usage and an example.
- **Networks:** several servers at once, each with its own nick. Manage them with
  `/network add|modify|remove|list`, or edit `config.toml`. `/connect` takes a
  network name or a plain `host[:port]` (IPv6 works).
- **Security:** TLS by default, optional certificate pinning (`tls_fingerprint`),
  `STARTTLS`, and SASL with SCRAM-SHA-256, PLAIN or EXTERNAL (client certificate).
- **IRCv3:** `server-time`, `message-tags`, `echo-message`, `away-notify`,
  `account-notify`, `account-tag`, `extended-join`, `invite-notify`, `chghost`,
  `setname`, `multi-prefix`, `userhost-in-names`, `batch` (basic) and `MONITOR`
  for `/notify`.
- **Windows:** split panes (`/split -h` or `-v`), window items, scrollback with
  `/lastlog` and `/scrollback`, mouse support, and a configurable status bar.
- **Netsplits:** a burst of quits from one split is collapsed into one line.
- **DCC:** chat and file transfers, resumable, passive mode for NAT.
- **Themes:** `classic`, `vivid`, `mono`, or your own in `custom_themes.toml`,
  with editable line formats (`/format`).
- **Lua plugins:** react to messages, joins, parts, quits, nick changes, topics
  and modes, replace what is shown or sent, or add a new `/command`.
  Load, unload and reload scripts at runtime with `/pluginload` and friends.
  See `crates/irust-plugin/examples/`.
- **Flood protection:** outgoing lines are spaced so the server does not kick
  you for excess flood (`/set flood_interval_ms`).
- **Logging:** per-window logs, rotated daily, with `/log start|stop`.

## Building

You need a C compiler, `cmake` and Rust (via [rustup](https://rustup.rs)).

```bash
cargo build --release
cargo install --path .   # installs to ~/.cargo/bin/irust
```

Run the tests with:

```bash
cargo test --workspace
```

## Configuration

A missing `~/.config/irust/config.toml` is fine, it just means no network
settings yet. A minimal example:

```toml
[networks.mynetwork]
host = "irc.example.org"
port = 6697
nick = "yournick"
sasl_account = "yournick"
sasl_password = "hunter2"
autojoin = ["#somechannel"]
autoconnect = true
```

The same thing from inside the client:

```
/network add mynetwork irc.example.org -nick yournick -autojoin #somechannel
/connect mynetwork
```

## Updating

`/update` downloads a newer build and replaces the binary. It needs a release
manifest: set the `IRUST_UPDATE_URL` environment variable to its address. It never
restarts on its own.

## Layout

A Cargo workspace of seven crates:

| Crate | What it does |
|---|---|
| `irust-proto` | IRC message parsing and formatting |
| `irust-caps` | CAP and SASL negotiation |
| `irust-net` | TLS or plain connections, line framing |
| `irust-core` | State, buffers, nicklist, highlighting (no I/O) |
| `irust-config` | Built-in networks and the `config.toml` overlay |
| `irust-tui` | The terminal interface (ratatui) |
| `irust-plugin` | The Lua scripting host |

`src/main.rs` ties them together.

## License

MIT or Apache-2.0, at your option. See `LICENSE-MIT` and `LICENSE-APACHE`.

You can find me on ChatNet: `irc.chatnet.gg`, port 6697 (TLS), channel `#chatnet`.
