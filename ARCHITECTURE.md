# OmniMix architecture

## Layers

```
 React dashboard (app/src)            Tauri shell (app/src-tauri)
   cards, EQ modal, meters  <-- IPC -->  commands in, events out
                                            |
 ===================== omnimix-core (OS-agnostic, pure Rust) =====================
   mixer::Mixer      reconcile loop, groups sessions per app, sink choice, state, presets
   engine            Strip (per app): ring -> drift ASRC -> EQ -> gain -> meters
                     BusProcessor (per device): sum -> device EQ -> safety limiter
   dsp               RBJ biquads (f64 state), 10 musical bands, crossfaded updates,
                     Kaiser-sinc ASRC, limiter, presets
   model             Device, Source, Channel, ConnectionKind (USB-C, USB, BT, AUX, ...)
   backend           AudioBackend trait: control plane + DSP plane
   mock              full simulated rig (runs the real engine)
 =================================================================================
                                            |
 omnimix-windows     WASAPI, Core Audio, CfgMgr32, USB hub IOCTLs, per-app routing
 omnimix-macos       Core Audio HAL, process objects, process taps, private aggregates
 omnimix-ffi         C ABI (omnimix.h) so any app on any OS can embed the EQ engine
 omnimix-probe       CLI inventory + live self-test of the per-app EQ path
```

Nothing above the backend line touches an OS API. A new OS is one crate that implements `AudioBackend`.

## Control plane (Windows)

| Feature | API |
|---|---|
| Outputs, state, default | IMMDeviceEnumerator, IMMNotificationClient |
| Device volume, mute, per-channel level | IAudioEndpointVolume |
| Apps per device | IAudioSessionManager2, IAudioSessionControl2 |
| App volume, mute | ISimpleAudioVolume |
| App per-channel mute | IChannelAudioVolume |
| Meters | IAudioMeterInformation (endpoint and session) |
| Set default output | IPolicyConfig::SetDefaultEndpoint |
| App name, icon | QueryFullProcessImageName, version resource FileDescription, SHDefExtractIcon |
| Accent color | UISettings AccentLight2 (dark UI variant) |

Channel mute stores the previous level in state and restores it on unmute.

## Connection detection

1. Endpoint -> IDeviceTopology -> connected KS filter -> device interface path -> PnP instance id.
2. Bus = instance id prefix. `USB`, `BTHENUM` (A2DP), `BTHHFENUM` (hands-free), `BTHLE*` (LE Audio), `HDAUDIO`, `INTELAUDIO`, `ROOT`/`SWD` (virtual).
3. USB: walk the devnode tree to the device sitting on a hub port, open the hub, `IOCTL_USB_GET_PORT_CONNECTOR_PROPERTIES` bit 3 = `PortConnectorIsTypeC`, `IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX` = speed, VID, PID.
4. AUX / built-in / optical: IKsJackDescription (connector type, front/rear, color, integrated vs jack).
5. HDMI vs DisplayPort: PKEY_AudioEndpoint_JackSubType against KSNODETYPE_HDMI_INTERFACE / DISPLAYPORT_INTERFACE.

A USB-C dongle behind a dock reports the dock's port, which is the connector the device is physically in.

## DSP plane (per-app and per-device EQ, no driver)

```
 App ──► parking output (endpoint muted)        silent
   └──► process-loopback tap (pre-endpoint) ──► ring ──► Strip(app EQ) ─┐
 App2 ─► ...                                                             ├─► Bus(device EQ, limiter) ──► WASAPI render ──► real output
                                                                         ┘
```

* Tap: `ActivateAudioInterfaceAsync(VAD\Process_Loopback, INCLUDE_TARGET_PROCESS_TREE)`, float32 stereo at the device rate. Works on Windows 10 2004 (build 19041) and newer.
* Silence the original: the app is routed to a spare output via the per-app default endpoint (the same mechanism as Settings > Volume mixer > App output) and that output's endpoint is muted. The loopback tap sits before endpoint mute, so capture stays full level.
* App volume and app mute still use the Windows session controls, which sit before the tap, so the Windows mixer and OmniMix always agree.
* Clock drift between parking output and real output is absorbed by a PI-controlled ASRC (holds the ring at the latency target, under 0.4% ratio change, tested at 300 ppm).
* Added latency on processed apps: about 30 ms (Low), 55 ms (Balanced), 90 ms (Safe). Unprocessed apps are untouched.
* Device EQ = every app on that output goes through the bus. System sounds stay direct.
* An app that hard-selects its own output device cannot be rerouted. OmniMix detects that after 4 s, restores it, and shows the reason on its card.

### Crash safety

Every route and every sink mute is written to `journal.json` before it is applied. On next start the sink mute is restored and each journaled app is put back on its previous output the moment it is seen. Clean exit restores everything.

## Real-time rules

* Render thread: no locks except `try_lock` mailboxes, no allocation after warm-up, MMCSS "Pro Audio".
* Parameter changes crossfade over one block, so dragging EQ never clicks.
* Auto clip guard lowers output by the curve's largest boost, the limiter guarantees no sample above -0.3 dBFS.

## Porting map

| Contract | Windows (done) | macOS (done) | Linux |
|---|---|---|---|
| Outputs + hotplug | MMDevice | AudioObject kAudioHardwarePropertyDevices | PipeWire nodes (media.class Audio/Sink) |
| Connection kind | topology + USB IOCTL + jack | kAudioDevicePropertyTransportType | node props device.bus / api.bluez5 |
| Apps | session manager | process objects (14.4+) | stream nodes (Stream/Output/Audio) |
| Divert an app | loopback tap + parking output | CATapDescription mutedWhenTapped + private aggregate | relink stream into a filter node |
| App volume / mute | Windows session controls | engine strip gain (`soft_app_mixer`) | stream node volume |
| Render a bus | WASAPI shared event | AudioDeviceIOProc | pw_stream playback |

## State

`%APPDATA%\app.omnimix.mixer\state.json`: device EQ by endpoint id, app EQ by executable name (so it follows the app), channel memories, user presets, prefs. Written atomically, corrupt files are set aside and defaults used.
