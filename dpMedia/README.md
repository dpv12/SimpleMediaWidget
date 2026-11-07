# dpMedia

A tiny always-on-top "now playing" overlay for Windows and Linux. It shows the
cover art, title and artist of the current track, has Previous / Play-Pause /
Next buttons, and a timeline you can click to jump to any point of the song or
video. It talks to the OS media layer directly (Windows SMTC, Linux MPRIS), so
it works with Spotify and any other player that registers there. No Web API,
no login.

## Download

Windows users do not need to compile anything: grab `dpmedia.exe` from the
[latest release](https://github.com/dpv12/SimpleMediaWidget/releases/latest)
and run it. A `dpmedia.exe.sha256` checksum is attached to every release.

## Use

| Action | How |
| --- | --- |
| Move the widget | Hold **Ctrl** and drag with the **left mouse button** |
| Skip / pause | Click the buttons |
| Jump to a time | Click the thin timeline along the lower edge of the widget at that point (hover shows the time), or drag the handle to scrub |
| About / Settings | Hover the widget, click the small **i** (or use the tray menu) |
| Quit | Hover the widget, click the small **x** (or use the tray menu) |
| Forget saved position | Start with `dpmedia --reset-position` |

The timeline is always visible along the lower edge and fills as the track
plays. Hovering it shows the time a click would jump to and the total length.
Jumping works when the player allows seeking; live streams and players that
report no length show an empty bar.

### Settings: which media sources are shown

Click the **i** and use the **Settings** section:

- **Ignore browser media** hides everything that comes from a browser (YouTube,
  Netflix, ...), so a video running in a tab no longer replaces your music.
- **Detected players** lists every player the OS currently reports. Untick one
  to ignore it. Players you switched off stay in the list (even when closed),
  so you can switch them back on.

If everything is ignored, the widget shows "Nothing playing". Settings apply
immediately and are saved.

### Tray icon (Windows)

The widget has no taskbar button; it lives in the notification area instead.

| Tray action | Effect |
| --- | --- |
| Left click | Show / hide the overlay |
| Right click -> **Toggle Overlay** | Show / hide the overlay |
| Right click -> **Reset Position** | Center the widget on the primary display |
| Right click -> **About / Settings** | Open the About & Settings window |
| Right click -> **Quit dpMedia** | Save the position and exit |

"Hidden" means the window stays alive but paints nothing and lets every click
through to the game. It costs no frames. On Linux there is no tray icon (it
would need GTK / libappindicator); the widget stays visible and the hover **i**
and **x** buttons cover Settings and Quit.

### Config

Position and source settings are stored as JSON, written shortly after a
change:

- Linux: `~/.config/dpmedia/config.json`
- Windows: `%APPDATA%\dpmedia\config.json`

Files from older versions still load (the removed `pins` entry is ignored and
disappears on the next save).

## Build and run

Requires Rust 1.80 or newer (`rustup`).

### Windows

1. Install the Visual Studio Build Tools ("Desktop development with C++").
2. ```powershell
   cargo run --release
   ```
   The binary is `target\release\dpmedia.exe`. The C runtime is linked
   statically (`.cargo/config.toml`), so the exe runs on a clean Windows
   install.

### Linux

1. Install build dependencies.
   - Debian / Ubuntu:
     ```bash
     sudo apt install build-essential pkg-config libdbus-1-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev libegl1-mesa-dev
     ```
   - Fedora: `sudo dnf install gcc pkgconf-pkg-config dbus-devel libxkbcommon-devel wayland-devel mesa-libGL-devel mesa-libEGL-devel`
   - Arch: `sudo pacman -S base-devel pkgconf dbus libxkbcommon wayland mesa`
2. Run it. A compositing window manager is needed for transparency.
   ```bash
   WINIT_UNIX_BACKEND=x11 cargo run --release
   ```
   The binary is `target/release/dpmedia`.

`WINIT_UNIX_BACKEND=x11` runs it through X11/XWayland, which is what you want
on a Wayland desktop (see limitations).

## Publishing a release

`.github/workflows/release.yml` (in the repository root) builds on
`windows-latest` and attaches `dpmedia.exe` to a GitHub Release:

```bash
git tag v0.4.1
git push origin v0.4.1
```

It can also be started by hand from the **Actions** tab (enter the tag to
publish). The tag should match the `version` in `Cargo.toml`.

## How it stays cheap

- **Reactive rendering.** eframe only runs a frame on input events or when
  something calls `request_repaint`. There is no animation loop, so an idle
  overlay uses no GPU/CPU.
- **State changes come from a worker thread.** It sleeps on a channel, polls the
  OS once per second, compares with the last state, and wakes the UI only when
  something differs (track, artist, play/pause, cover, player appeared/vanished).
  Button clicks and seeks go to that thread and run immediately. Tray clicks
  wake the UI the same way, once per click.
- **The timeline costs one tiny repaint per second, only while playing.** The
  worker publishes the position once per poll; paused, without a player or
  hidden via the tray, the overlay is back at zero frames.
- **Cover art is decoded once per track change**, on the worker thread, center-
  cropped and downscaled to 192x192 before it becomes a texture. Nothing is
  written to disk.
- **The window is exactly the pill.** It has a fixed size, so the game receives
  clicks everywhere around the widget.
- **Text layout is cached.** Title/artist ellipsizing runs once per track
  change, not per frame.
- **Position saves are debounced** (one small file write ~0.5 s after you stop
  dragging).
- Release profile uses fat LTO, one codegen unit, `panic = "abort"`, stripped.

## Known limitations

- **Exclusive fullscreen games cannot be overlaid** by any normal window. Use
  borderless-windowed mode.
- **Wayland (native):** the protocol has no "always on top" for normal windows
  and clients cannot read or set their own position, so use the X11/XWayland
  command above for the full feature set.
- **Focus:** clicking the widget gives it keyboard focus, which takes focus away
  from the game. Some games pause when that happens.
- **Fonts:** egui's built-in font covers Latin, Cyrillic and Greek but not
  CJK. Such titles will show placeholder boxes until a fallback font is added.
- **Which player:** among the allowed sources, a playing one wins, then the one
  Windows calls "current" (Linux: a paused one). Neither is pinned to Spotify;
  use the Settings to ignore the others.
- **Browsers:** a browser reports one session for all its tabs, so you can
  ignore browsers as a whole, not single tabs or sites.
- **Timeline accuracy:** some players only report their position when it
  changes (play, pause, seek). The bar is then extrapolated from the last
  report and re-synced at the next one.
- **Cover art** depends on the player publishing it (SMTC thumbnail / MPRIS
  `mpris:artUrl`); without one the widget shows a placeholder.
- **Multiple monitors with different scaling:** the window keeps its size when
  it crosses to a monitor with another scale, and the position is saved in
  physical pixels so it comes back to the right spot (and is pulled back onto
  the desktop if a monitor is gone). "Reset Position" centers on the primary
  display.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any additional
terms or conditions.

## Haftungsausschluss / Disclaimer

**Deutsch.** dpMedia wird unentgeltlich als freie Software zur Verfügung
gestellt (Schenkung, §§ 516 ff. BGB). Nach § 521 BGB hat der Anbieter nur
Vorsatz und grobe Fahrlässigkeit zu vertreten; für leichte Fahrlässigkeit
wird nicht gehaftet. Für Sach- und Rechtsmängel haftet der Anbieter nur,
wenn er einen Mangel arglistig verschwiegen hat (§§ 523, 524 BGB). Die Nutzung
erfolgt auf eigenes Risiko. Zwingende gesetzliche Haftung, die sich nicht
ausschließen lässt, bleibt unberührt.

**English.** dpMedia is provided free of charge as open-source software, which
under German law is a gift (§§ 516 ff. BGB). Under § 521 BGB the provider is
liable only for intent and gross negligence, not for slight negligence. For
defects of quality or title the provider is liable only if a defect was
fraudulently concealed (§§ 523, 524 BGB). Use is at your own risk. Mandatory
statutory liability that cannot be excluded remains unaffected.

## Third-party software

dpMedia is built on the following open-source components. Their licenses
apply to their respective code; the full texts are in each crate's repository.

| Component | License |
| --- | --- |
| [egui / eframe](https://github.com/emilk/egui) | MIT OR Apache-2.0 |
| [image](https://github.com/image-rs/image) | MIT OR Apache-2.0 |
| [serde](https://github.com/serde-rs/serde), [serde_json](https://github.com/serde-rs/json) | MIT OR Apache-2.0 |
| [dirs](https://github.com/dirs-dev/dirs-rs) | MIT OR Apache-2.0 |
| [open](https://github.com/Byron/open-rs) | MIT |
| [tray-icon](https://github.com/tauri-apps/tray-icon) (Windows) | MIT OR Apache-2.0 |
| [windows](https://github.com/microsoft/windows-rs) (Windows) | MIT OR Apache-2.0 |
| [mpris](https://github.com/Mange/mpris-rs) (Linux) | Apache-2.0 |
| [ureq](https://github.com/algesten/ureq) (Linux) | MIT OR Apache-2.0 |

Fonts bundled with egui: Ubuntu-Light (Ubuntu Font Licence 1.0), Hack (MIT /
Bitstream Vera), Noto Emoji (SIL OFL 1.1) and emoji-icon-font. Transitive
dependencies carry their own licenses; `cargo tree` lists them, and a tool such
as `cargo-about` can generate the complete notice file.

## Trademarks

dpMedia is an independent project and is not affiliated with, endorsed by or
sponsored by Spotify AB. "Spotify" is a trademark of Spotify AB. Windows is a
trademark of Microsoft Corporation.
