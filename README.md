# Ngwa

**A fast, native Matrix client.** Ngwa is a lightweight desktop client for
[Matrix](https://matrix.org), written in Rust. No browser engine, no Electron:
it should start in well under a second and use a fraction of the memory of
web-based clients.

*Ngwa* is Igbo for "quick" — as in *ngwa ngwa*, "hurry up".

> **Status: early development.** Ngwa can sign in, list your rooms, open
> them, and send and reply to messages in unencrypted rooms. Encrypted rooms,
> images and notifications are still to come.

<p align="center"><img src="docs/screenshot.png" width="720" alt="Ngwa with the room list on the left and a conversation open on the right, including a reply"></p>

## How it works

- **[matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk)** speaks
  the Matrix protocol: sign-in, sync, rooms, and (later) end-to-end encryption.
- **Sliding sync** loads only what is on screen, so your room list appears
  almost immediately instead of after a long first sync. Your homeserver must
  support it; matrix.org and current Synapse releases do.
- **[egui](https://github.com/emilk/egui)** draws the interface with OpenGL,
  natively on Linux, macOS, and Windows. There is no browser engine.
- **The window never waits on the network.** Syncing runs on background
  threads; the window only redraws when something changes.
- **Your session stays in the system credential store** (Keychain on macOS,
  Credential Manager on Windows, Secret Service on Linux), or in an owner-only
  file where there is none. Local data is kept in an encrypted database.

## Roadmap

**v0.1 — a daily driver for unencrypted rooms**

- [x] Milestone 1: sign in with a password, resume the session, list rooms
- [x] Milestone 2: the room list in a desktop window, live, with search
- [x] Milestone 3: open a room, read the timeline, scroll back, send and reply
- [x] Release: builds for Windows, macOS and Linux, clickable links

**v0.2 — end-to-end encryption:** device verification, key backup and
recovery, so encrypted DMs work.

**Later:** media, reactions, edits, threads, notifications, formatted
messages, and automatic updates.

## Download

Get the latest release from [ngwa.chat](https://ngwa.chat) or the
[releases page](https://github.com/kingsmandralph/ngwa/releases/latest):

| System | File |
| --- | --- |
| Windows 10 and 11 | `ngwa-windows-x86_64.zip`: unzip and run `ngwa.exe` |
| macOS 11 or later | `ngwa-macos-universal.zip`: move Ngwa to Applications |
| Linux x86_64 | `ngwa-linux-x86_64.tar.gz`: unpack and run `./install-linux.sh` |

The builds aren't signed by Apple or Microsoft yet. On macOS, right-click
Ngwa and choose **Open** the first time. On Windows, if SmartScreen appears,
choose **More info**, then **Run anyway**.

When signing in, give your full Matrix ID (like `@you:matrix.org`). With
just a username, the homeserver defaults to matrix.org. Your homeserver
needs sliding sync, which matrix.org and current Synapse releases have.

## Build from source

You need [Rust](https://rustup.rs) 1.96 or newer.

```sh
cargo install --git https://github.com/kingsmandralph/ngwa
ngwa
```

The first build takes a few minutes. The terminal commands work too:
`ngwa login`, `ngwa rooms`, `ngwa logout` and `ngwa --version`. To look
around without an account, run `ngwa --demo`.

Where Linux has no credential store (WSL, servers, minimal desktops), Ngwa
saves the session in a file only you can read instead. On WSL, Ngwa uses
X11 for a normal Windows title bar; set `NGWA_WAYLAND=1` to use Wayland
instead.

Encrypted rooms appear in the list, but reading their messages arrives in v0.2.

Ngwa uses the [Inter](https://rsms.me/inter/) typeface, under the SIL Open
Font License ([assets/fonts/OFL.txt](assets/fonts/OFL.txt)).

## Releasing

Push a tag like `v0.1.0` and the release workflow builds every platform and
publishes a GitHub Release with notes from `docs/release-notes/<tag>.md`.
Pushing to the `release-check` branch runs the same builds without
publishing. The website in `site/` is static and deploys to Cloudflare
Pages.

## Contributing

Issues and pull requests are welcome. Run `cargo fmt` and `cargo clippy`
before opening a pull request. `cargo run -- --demo` opens the window with
made-up rooms, which is the quickest way to work on the interface.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

Ngwa is an independent project and is not affiliated with or endorsed by The
Matrix.org Foundation. Matrix is a trademark of The Matrix.org Foundation.
