# dpMedia

A tiny always-on-top "now playing" overlay for Windows and Linux. It shows the
current title and artist and has Previous / Play-Pause / Next buttons. It talks
to the OS media layer directly (Windows SMTC, Linux MPRIS), so it works with
Spotify and any other player that registers there. No Web API, no login.

## Use

| Action | How |
| --- | --- |
| Move the widget | Hold **Ctrl** and drag with the **left mouse button** |
| Skip / pause | Click the buttons |
| Quit | Hover the widget, click the small **x** in the top-right corner |
| Forget saved position | Start with `dpmedia --reset-position` |

The position is saved shortly after you stop dragging:

- Linux: `~/.config/dpmedia/config.json`
- Windows: `%APPDATA%\dpmedia\config.json`

## Build and run

Requires Rust 1.76 or newer (`rustup`).

### Windows

1. Install the Visual Studio Build Tools ("Desktop development with C++").
2. ```powershell
   cargo run --release
   ```
   The binary is `target\release\dpmedia.exe`.

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

## How it stays cheap

- **Reactive rendering.** eframe only runs a frame on input events or when
  something calls `request_repaint`. There is no animation loop, so an idle
  overlay uses no GPU/CPU.
- **State changes come from a worker thread.** It sleeps on a channel, polls the
  OS once per second, compares with the last state, and wakes the UI only when
  something differs (track, artist, play/pause, player appeared/vanished).
  Button clicks go to that thread and run immediately.
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
- **Which player:** Windows decides which SMTC session is "current"; Linux
  prefers a player that is currently playing. Neither is pinned to Spotify.
