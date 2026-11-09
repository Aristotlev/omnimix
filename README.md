# OmniMix

Equalizer, sound mixer and audio dashboard for Windows 10/11 and macOS 14.4+, built on an OS-agnostic Rust core.

* Every playback device, labelled by how it is connected: **USB-C**, USB, Bluetooth (A2DP / hands-free / LE), AUX 3.5 mm (front or rear, jack color), HDMI or DisplayPort, S/PDIF, built-in, virtual
* Every app playing audio, with its real name and icon
* Mute and volume per **app**, per **device**, per **channel** (L, R, C, LFE, SL, SR ...)
* 10-band musical EQ per app and per device: **Sub, Bass, Lows, Body, Mud, Mids, Claps, Presence, Hats, Air**, plus a full parametric mode (bell, shelves, cuts, notch, up to 16 bands), 12 presets, your own presets, clip guard, stereo width
* Playback only. Nothing is recorded or written to disk except your settings

## Install

| File | Use |
|---|---|
| `OmniMix_0.1.0_x64-setup.exe` | Installer (per-user, no admin) |
| `OmniMix.exe` | Portable, run from any folder |
| `omnimix-probe.exe` | Diagnostics and self-test |

The builds are not code-signed yet, so SmartScreen shows "Windows protected your PC". Click **More info > Run anyway**.

Needs the WebView2 runtime, already present on Windows 11 and current Windows 10. The installer fetches it if missing.

## macOS (14.4 Sonoma or newer)

```bash
bash build-mac.sh          # installs Rust if missing, builds OmniMix.app + .dmg into dist-mac/
```

* First time an app is processed, macOS asks to allow OmniMix to record system audio. Allow it, or processed apps go silent. Toggle later in System Settings > Privacy & Security > Screen & System Audio Recording.
* macOS has no per-app mixer, so app volume, app mute and L/R mute run through the OmniMix engine: changing them taps that app (silent original, processed copy plays). Back at 100% and unmuted, the app plays untouched again.
* Meters show for anything OmniMix processes. macOS exposes no meters for untouched apps or outputs.
* Connection types come from Core Audio: USB, Bluetooth (stereo / hands-free / LE), HDMI, DisplayPort, Thunderbolt, AirPlay, built-in speakers vs headphone jack, virtual. macOS does not report USB-C vs USB-A, so both show as USB.
* If OmniMix quits or crashes, macOS removes its taps and every app plays normally. Nothing to restore.

## Verify on your PC (2 minutes)

```powershell
.\omnimix-probe.exe list       # every output, connection type, every app
.\omnimix-probe.exe selftest   # plays a quiet tone and proves the per-app EQ path end to end
```

The self-test ends with `Verdict: per-app EQ path WORKS on this PC` or lists what failed.

## How per-app EQ works without a driver

1. OmniMix taps the app's audio with Windows process loopback (Windows 10 2004+).
2. It moves the app to a spare output (the **parking output**) and mutes that output, so you only hear the processed copy.
3. The engine applies app EQ, then device EQ, then renders to your real output.

Needs a second active output to park on: HDMI audio from your monitor, unused onboard speakers, or the free VB-CABLE virtual cable. Picked automatically, changeable in Settings. Volume, mute and channel control work with no parking output at all.

Added latency on processed apps only: about 30 / 55 / 90 ms (Low / Balanced / Safe in Settings).

If OmniMix is killed mid-session, the next launch restores every app and the parking output from its journal.

## Releases and automatic updates

Every installed copy (0.1.1 and newer) checks GitHub on launch and every 6 hours, shows "OmniMix x.y.z is available", and installs + restarts on one click. Updates are signed: the app only accepts packages signed with your private key.

One time, on your Mac, from the repo folder:

```bash
bash scripts/publish-github.sh        # creates the public repo, signing key, secrets, first release
```

Every update after that:

```bash
bash scripts/release.sh 0.1.2 "What changed"
```

GitHub Actions (`.github/workflows/release.yml`) then builds the Windows installer and a universal macOS app, signs both, and publishes them with `latest.json`. Users of 0.1.0 install 0.1.1 once by hand. Everything after updates itself.

Keep `~/.tauri/omnimix.key` and `~/.tauri/omnimix.key.password` backed up. Without them no further update can reach installed copies.

Downloads from GitHub are not code-signed yet: Windows shows SmartScreen (More info > Run anyway), macOS needs System Settings > Privacy & Security > Open Anyway the first time. Updates installed by the app itself skip both.

## Build

Windows (native, MSVC):

```powershell
winget install Rustlang.Rustup OpenJS.NodeJS.LTS
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
cd app
npm install
npx tauri build                      # -> src-tauri\target\release\bundle\nsis\
cargo build --release -p omnimix-probe
```

Linux cross-build to Windows (what produced these binaries):

```bash
sudo apt install mingw-w64 nsis rustc-1.97 cargo-1.97 rust-1.97-src
source scripts/env.sh                          # distro toolchain + build-std for Windows
export PATH=$PWD/scripts/shim:$PATH            # lets the Tauri CLI see the Windows target
cd app && npm install
npx tauri build --target x86_64-pc-windows-gnu -- -Zbuild-std=std,panic_unwind
```

Dev loop with simulated devices (any OS):

```bash
cd app && npm run dev          # browser preview, demo rig
cargo test -p omnimix-core     # DSP, engine, mixer tests
cargo run -p omnimix-probe     # demo rig inventory on non-Windows
```

`OmniMix.exe --demo` runs the desktop app on the simulated rig.

## Layout

```
crates/omnimix-core      OS-agnostic: model, DSP, real-time engine, mixer, state, presets, mock rig
crates/omnimix-windows   Windows backend (WASAPI, Core Audio, CfgMgr32, USB IOCTLs, routing)
crates/omnimix-macos     macOS backend (Core Audio HAL, process taps, private aggregates)
crates/omnimix-ffi       C ABI + include/omnimix.h (embed the EQ engine in any app, any OS)
crates/omnimix-probe     diagnostics CLI
app/                     Tauri v2 shell + React dashboard
```

Porting to Linux = one crate implementing `omnimix_core::backend::AudioBackend`. See `ARCHITECTURE.md` for the API map.

## Settings file

`%APPDATA%\app.omnimix.mixer\state.json`. App EQ follows the app by executable name, device EQ follows the device.
