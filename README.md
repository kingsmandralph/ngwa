# Ngwa

**A fast, native Matrix client.** Ngwa is a lightweight desktop client for
[Matrix](https://matrix.org), written in Rust. No browser engine, no Electron:
it should start in well under a second and use a fraction of the memory of
web-based clients.

*Ngwa* is Igbo for "quick" — as in *ngwa ngwa*, "hurry up".

> **Status: early development.** Ngwa is at Milestone 1, a terminal preview
> that signs in and lists your rooms. The desktop window comes next. It is not
> ready for daily use yet.

## How it works

- **[matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk)** speaks
  the Matrix protocol: sign-in, sync, rooms, and (later) end-to-end encryption.
- **Sliding sync** loads only what is on screen, so your room list appears
  almost immediately instead of after a long first sync. Your homeserver must
  support it; matrix.org and current Synapse releases do.
- **[egui](https://github.com/emilk/egui)** will draw the interface, natively
  on Linux, macOS, and Windows.
- **Your session stays in the system credential store** (Keychain on macOS,
  Credential Manager on Windows, Secret Service on Linux). Local data is kept
  in an encrypted database.

## Roadmap

**v0.1 — a daily driver for unencrypted rooms**

- [x] Milestone 1: sign in with a password, resume the session, list rooms
- [ ] Milestone 2: the room list in a desktop window
- [ ] Milestone 3: open a room, read the timeline, scroll back, send and reply

**v0.2 — end-to-end encryption:** device verification, key backup and
recovery, so encrypted DMs work.

**Later:** media, reactions, edits, threads, notifications, packaging, and
automatic updates.

## Try it

You need [Rust](https://rustup.rs) 1.96 or newer.

```sh
git clone https://github.com/kingsmandralph/ngwa
cd ngwa
cargo run --release -- login    # homeserver, username, password
cargo run --release -- rooms    # list your rooms
cargo run --release -- logout   # sign out and forget this device
```

On Linux, a Secret Service provider (GNOME Keyring or KWallet) must be running.
The first build takes a few minutes; later builds are fast.

Encrypted rooms appear in the list, but reading their messages arrives in v0.2.

## Contributing

Issues and pull requests are welcome. Run `cargo fmt` and `cargo clippy`
before opening a pull request.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

Ngwa is an independent project and is not affiliated with or endorsed by The
Matrix.org Foundation. Matrix is a trademark of The Matrix.org Foundation.
