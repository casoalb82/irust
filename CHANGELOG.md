# Changelog

## Unreleased

Added
- Multi-network support: several servers at once, with the window list grouped per network.
- `/network add` and `/network modify` can set bouncer options too.
- `/channel add|modify|remove|list` for per-channel settings that persist.
- Split windows with any number of panes (`/split -h` or `-v`), and `/window item` to group several channels or queries in one window slot.
- `/eval` with irssi-style expandos.
- `/format` to edit a theme's format templates live, with `-delete` and `-reset`.
- `/recode` for per-target and per-network character sets.
- `/rmreconns`, `/squery`, `/servlist`, `/sethost`, `/setname`, `/netsplit`, `/help` (alias of `/cmds`).
- `/ignore -time` for ignores that expire on their own, plus `-except` and `-replies`.
- `/hilight -actcolor`, `-priority` and `-color`.
- `/dcc resume`, DCC auto-accept settings, DCC port range and own-IP settings.
- `/log start|stop` to switch logging on or off for one window.
- `/set` options: `nick`, `cmdchars`, `scrollback_lines`, `flood_interval_ms`, `paste_grace_ms`, `show_away_once`, `show_quit_once`, `window_history`, `autolog_ignore_targets`, `recode_out_default_charset`.
- IRCv3 `batch` negotiation (basic).
- `STARTTLS` per network (`starttls = true` in `config.toml`).
- More scripting hooks: `on_join`, `on_part`, `on_quit`, `on_nick`, `on_topic`, `on_mode`, `on_connect`, `on_disconnect`.

Changed
- Connections set `TCP_NODELAY`, so small chat lines are no longer held back.
- `/update` reads its manifest URL from the `IRUST_UPDATE_URL` environment variable.

Fixed
- Replayed private messages from a bouncer no longer show up again on every start or reconnect.
- Bouncer backlog older than 60 seconds no longer rings the bell on a highlight.
- `/connect` and the automatic reconnect show connection progress, like irssi does.
- Away nicks and a few theme colors stay readable on terminals with unusual color schemes.
- Fixes for drawing lines packed with emoji.
- A non-UTF-8 byte from the network no longer drops the whole connection.
- Old log content is no longer replayed into the window on connect, which duplicated the MOTD.
- Passwords typed with `/msg`, `/oper` and similar commands are no longer saved in the input history.

Removed
- `/quitmsg` (add, remove and list).

## v1.4.3 (2026-09-04)

- `DCC SEND` and `DCC GET` for real file transfers.
- Daily log rotation using irssi's strftime convention.
- SASL SCRAM-SHA-256.
- `/hilight` flags `-regex` and others, `/ignore` gets six more levels, `/lastlog` gets `-regex` and `-case`.
- Mouse support in the TUI.
- TLS certificate pinning (`tls_fingerprint` in `config.toml`).
- Six more IRCv3 capabilities, including `account-notify` and `account-tag`.
- Custom themes.
- `/split` for a second pane.

## v1.4.2 (2026-09-03)

- IRCv3 `MONITOR` for `/notify`.
- A configurable status bar (`/statusbar`).
- `/layout save` stores the joined channels, keys included.
- `/ignore -regex`.
- `TAGMSG` support.
- Fixes for WHO away status, service notices with embedded colors, and a dull mIRC color 8.

## v1.4.1 (2026-09-02)

- `/cmds` lists commands, with usage and an example for each.
- `/set show_joinpart on|off`.
- Join, part and quit lines match irssi's format.
- Netsplit detection that collapses the quit and rejoin noise.
- `config.toml` is written with mode 600.
- The `--debug` log redacts passwords (`PASS`, SASL, `OPER`, `IDENTIFY`).
- A broken `config.toml` shows a warning in the startup banner instead of silently using defaults.

## v1.4.0 (2026-09-01)

- `/set ident` and `/set realname`.
- Raw-paste detection.
- Level-scoped `/ignore`.
- `/set theme classic|vivid|mono`.
- `/update` checks a manifest and replaces the binary.
- `/pluginload`, `/pluginunload` and `/pluginreload`.

## v1.3.0 (2026-08-31)

- Channel keys survive an automatic rejoin.
- `/reconnect` keeps manually joined channels.
- Joining a very large channel no longer drops the connection.
- Redraws are capped at about 20 fps during heavy traffic.
- `tls = false` network overrides are honored.
- `/connect` handles bracketed IPv6 addresses.
- CTCP `FINGER`, `USERINFO` and `SOURCE`.
- `/network list|add|remove`.
- SASL EXTERNAL (CertFP).
- `/set`, `/exec`, `/hilight` and `/dehilight`.

## v1.2.0 (2026-08-30)

- `echo-message` capability, host shown on join and quit, a separate kind for wallops.

## v1.1.0 (2026-08-29)

- Cursor-based editing of the input line.
- Tab completion for channel names.
- `/wc` for window sizes, and a window list that wraps.
- Fixes for races between `/ip` and `WHOIS`.

## v1.0.0 (2026-08-25)

- Multiple servers: `/connect`, `/disconnect`, `/server`.
- Window model with scrollback, `/window` and most of irssi's command set.
- `/ip` for geolocation lookups.

## v0.1 (2026-08-24)

First working version: a Rust workspace of seven crates, a ratatui interface with buffers and a nicklist, TLS, CAP and SASL, the basic IRC commands, and Lua plugins.
