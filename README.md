# tmuxmux-mobile

A mobile companion to [tmuxmux](https://github.com/qume/tmuxmux): a touch/keyboard
tmux **session selector** and **terminal** for Android (and, by design, iOS later).

Unlike the desktop app — which shells out to the `ssh` and `tmux` binaries — a
phone has no such binaries and a sandbox that forbids spawning them, so this app
speaks **SSH in-process** (via [russh](https://crates.io/crates/russh)) and drives
tmux over that connection. The UI is [egui](https://github.com/emilk/egui), so the
whole thing is one Rust codebase that also cross-compiles to iOS.

## Design

- **Selector view** — your hosts; tap one to connect and list its tmux sessions;
  tap a session to attach, or create a new one.
- **Terminal view** — the attached session, rendered from a VT100 parser. A
  physical keyboard is the intended input. Tap **≡ Sessions** to go back; the
  tmux session keeps running on the server.
- The two views **toggle**; they never share the screen (deliberate, for small
  displays).

## Install

Grab the latest `tmuxmux-mobile.apk` from
[Releases](https://github.com/qume/tmuxmux-mobile/releases) and sideload it
(enable "install unknown apps" for your browser/file manager). arm64 only —
i.e. essentially every real phone.

On first launch, tap **➕ Host** and enter host / port / username and either a
password or an OpenSSH private key. Config is stored in the app's private
storage as `config.json`.

## Importing hosts

Instead of typing hosts in, drop a file into the app's external files dir and
it's imported on next launch (then deleted so it won't clobber later edits):

```sh
# native format (recommended) — a JSON Config, see src/config.rs
adb push import.json  /sdcard/Android/data/xyz.geocam.tmuxmux/files/import.json
# or a desktop tmuxmux hosts.toml (best-effort conversion)
adb push hosts.toml   /sdcard/Android/data/xyz.geocam.tmuxmux/files/hosts.toml
```

**`hosts.toml` caveats.** The desktop app shells out to `ssh`, so its config
leans on things this app can't do in-process:
- **`ssh`-config aliases** (a bare `name` with no real hostname) — the app has
  no hostname/key for them, so they're imported as-is and will only connect if
  the name is a resolvable, directly-reachable host.
- **`command = "… cloudflared access ssh … ProxyCommand …"`** hosts need the
  `ssh` + `cloudflared` binaries to open a Cloudflare Access tunnel. There's no
  way to do that in-process, so these are **skipped** on import.

So a desktop `hosts.toml` that's mostly cloudflared/alias hosts will import
few or no usable entries. Direct-SSH hosts (host + password or key) work.

## Build

Desktop (for quick UI/logic testing without a device):

```sh
cargo run --example desktop
```

Android APK (needs the Android SDK + NDK; see `.github/workflows/build.yml`
for the exact setup CI uses):

```sh
rustup target add aarch64-linux-android
cargo install cargo-apk
export ANDROID_HOME=~/Android/Sdk
export ANDROID_NDK_ROOT=$ANDROID_HOME/ndk/27.2.12479018
cargo apk build --lib --release
# -> target/release/apk/tmuxmux-mobile.apk
```

## Signing

`release.keystore` is committed intentionally: this is a personal tool (not on
the Play Store), and a stable signing key means you can update the app in place
without uninstalling first. If this ever becomes a real published app, rotate
the key and move it to a CI secret.

## Status

Early MVP, **verified working on a physical Android 11 device**: launch,
import config, SSH connect (password or key), list sessions, attach/create,
live terminal (colors + box-drawing + tmux status line), keyboard input,
resize, detach.

Not yet: touch text selection, on-screen modifier keys, host-key fingerprint
pinning (currently trust-on-first-use), ProxyCommand/tunnel support.
