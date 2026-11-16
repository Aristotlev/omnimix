# OmniMix

**Every app and every speaker, its own volume and EQ.** Equalizer, sound mixer and audio dashboard for Windows and Mac. See every app playing sound and every speaker, headset and cable you have plugged in, then set volume, mute and EQ for each one.

<p>
  <a href="https://github.com/Aristotlev/omnimix/releases/latest/download/OmniMix-Windows.exe"><b>⬇ Download for Windows</b></a> &nbsp;·&nbsp;
  <a href="https://github.com/Aristotlev/omnimix/releases/latest/download/OmniMix-Mac.dmg"><b>⬇ Download for Mac</b></a>
</p>

Windows 10 (version 2004 or later) / 11, 64-bit, and macOS 14.4 Sonoma or later (Apple silicon and Intel). Free. Once installed, OmniMix updates itself.

## What you get

- **Every playback device**, labelled by how it is connected: **USB-C**, USB, Bluetooth, AUX 3.5 mm (front or rear, with the jack color), HDMI or DisplayPort, S/PDIF, built-in, virtual.
- **Every app playing audio**, with its real name and icon.
- **Volume and mute** per **app**, per **device** and per **channel** (left, right, center, subwoofer, surround).
- **10-band EQ** per app and per device: **Sub, Bass, Lows, Body, Mud, Mids, Claps, Presence, Hats, Air**. Plus a full parametric mode with up to 16 bands, 12 presets, your own presets, clip guard and stereo width.
- **Playback only.** Nothing is recorded. Nothing is saved to disk except your settings.

## First launch

OmniMix is new and not yet signed with a paid Apple or Microsoft certificate, so your computer asks once before opening it:

- **Windows:** run `OmniMix-Windows.exe`. It installs for your user only, no admin needed. If you see "Windows protected your PC", click **More info**, then **Run anyway**.
- **Mac:** open `OmniMix-Mac.dmg` and drag **OmniMix** to **Applications**. The first time you open it, macOS may say it can't verify the developer. Go to **System Settings → Privacy & Security**, scroll down and click **Open Anyway**. When macOS asks to let OmniMix record system audio, click **Allow**. Otherwise apps you add EQ to go silent.

## Good to know

**Windows**

- Per-app and per-device EQ need a second active output to park the original sound on: HDMI audio from a monitor, unused onboard speakers, or the free VB-CABLE. OmniMix picks one by itself (you can change it in Settings). Volume, mute and channel control work without it.
- Needs the WebView2 runtime. It is already on Windows 11 and current Windows 10, and the installer fetches it if missing.

**Mac**

- You can change the audio permission later in **System Settings → Privacy & Security → Screen & System Audio Recording**.
- macOS has no per-app mixer of its own, so app volume, app mute and left/right mute run through OmniMix. Back at 100% and unmuted, the app plays untouched again.
- Level meters show for anything OmniMix processes. macOS shows no meters for untouched apps or outputs.
- macOS does not tell USB-C apart from USB-A, so both show as USB.
- If OmniMix quits or crashes, every app plays normally again.

## Privacy

Everything stays on your computer. OmniMix has no accounts, no servers and no analytics. It only plays audio and never records it.

[Report a problem](https://github.com/Aristotlev/omnimix/issues)

## FAQ

**How do updates work?**
OmniMix checks for a new version when it opens and every 6 hours, shows "OmniMix x.y.z is available", and installs and restarts in one click. Every update is signed and checked before it installs. **Settings → Updates → Check now** checks right away.

**Where are my settings kept?**
Windows: `%APPDATA%\app.omnimix.mixer\state.json`. App EQ follows the app by its program name, device EQ follows the device.

---

<details>
<summary><b>For developers</b></summary>

Built on an OS-agnostic Rust core, a Tauri 2 shell and a React + TypeScript dashboard. See [ARCHITECTURE.md](ARCHITECTURE.md) for the API map.

### Diagnostics (Windows, from source builds)

```powershell
.\omnimix-probe.exe list       # every output, connection type, every app
.\omnimix-probe.exe selftest   # plays a quiet tone and proves the per-app EQ path end to end
```

The self-test ends with `Verdict: per-app EQ path WORKS on this PC` or lists what failed.

### How per-app EQ works without a driver

1. OmniMix taps the app's audio with Windows process loopback (Windows 10 2004+).
2. It moves the app to a spare output (the **parking output**) and mutes that output, so you only hear the processed copy.
3. The engine applies app EQ, then device EQ, then renders to your real output.

Added latency on processed apps only: about 30 / 55 / 90 ms (Low / Balanced / Safe in Settings).

If OmniMix is killed mid-session, the next launch restores every app and the parking output from its journal.

### Releases and automatic updates

One time, on your Mac, from the repo folder:

```bash
bash scripts/publish-github.sh        # creates the public repo, signing key, secrets, first release
```

Every update after that:

```bash
bash scripts/release.sh 0.1.3 "What changed"
```

GitHub Actions (`.github/workflows/release.yml`) builds the Windows installer and a universal macOS app, signs both, publishes them with `latest.json` for the updater, and attaches the fixed-name downloads `OmniMix-Windows.exe` and `OmniMix-Mac.dmg` that the download buttons point at. Installed copies (0.1.1 and newer) update themselves.

Keep `~/.tauri/omnimix.key` and `~/.tauri/omnimix.key.password` backed up. Without them no further update can reach installed copies.

### Build

Windows (native, MSVC):

```powershell
winget install Rustlang.Rustup OpenJS.NodeJS.LTS
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
cd app
npm install
npx tauri build                      # -> target\release\bundle\nsis\
cargo build --release -p omnimix-probe
```

macOS from source:

```bash
bash build-mac.sh          # installs Rust if missing, builds OmniMix.app + .dmg into dist-mac/
```

Linux cross-build to Windows:

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

### Layout

```
crates/omnimix-core      OS-agnostic: model, DSP, real-time engine, mixer, state, presets, mock rig
crates/omnimix-windows   Windows backend (WASAPI, Core Audio, CfgMgr32, USB IOCTLs, routing)
crates/omnimix-macos     macOS backend (Core Audio HAL, process taps, private aggregates)
crates/omnimix-ffi       C ABI + include/omnimix.h (embed the EQ engine in any app, any OS)
crates/omnimix-probe     diagnostics CLI
app/                     Tauri v2 shell + React dashboard
docs/                    website: https://aristotlev.github.io/omnimix/
```

Porting to Linux = one crate implementing `omnimix_core::backend::AudioBackend`.

</details>
