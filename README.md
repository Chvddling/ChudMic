# ChudMic

Use your Android phone as a microphone for your Windows PC over Wi-Fi.

- **Phone app:** Connect / Disconnect, Mute, mic volume slider, level meter. It keeps working with the screen off and has Mute and Disconnect buttons in the notification.
- **PC app:** Connect / Disconnect, Mute, volume slider, **voice pitch slider (±6 semitones)**, level meter, and a choice of output device.
- **Pairing:** the phone and PC find each other automatically on the same Wi-Fi. If that's blocked, scan the QR code the PC shows, or type its IP.
- **Delay:** about 30–80 ms on a normal home network. Audio is 48 kHz mono, uncompressed, and never leaves your local network.

## Setup (one time)

1. **Install VB-Cable** (free) from https://vb-audio.com/Cable/. Run `VBCABLE_Setup_x64.exe` as administrator, then reboot.
   This adds the device pair "CABLE Input" (where ChudMic plays the audio) and "CABLE Output" (which apps use as a mic).
2. **Install the phone app:** copy `ChudMic.apk` to your phone and open it. Android will ask you to allow installs from that source.
3. **Run `chudmic.exe` on the PC.** The first time, Windows Firewall asks about network access: tick **Private networks** and allow it.
   (If you clicked Cancel by mistake: Windows Security → Firewall → Allow an app through firewall → ChudMic → Private.)

## Use

1. Open ChudMic on both devices, connected to the same Wi-Fi.
2. Tap your PC in the phone's list, or click **Connect** next to the phone in the PC app.
3. In Discord / OBS / your game, pick **CABLE Output** as the microphone.

The PC app picks "CABLE Input" automatically when it finds it. You can change the device under "Send audio to:". If you choose your speakers instead, you'll hear yourself, which is handy for testing.

**Pitch:** move the Pitch slider in the PC app to make your voice higher or lower (half-semitone steps, ±6). At 0 the effect is off and adds no delay; any other setting adds about 50 ms. Your setting is remembered.

**Mute and volume are separate on each side.** Phone mute stops sending, and the PC app shows "The phone has muted its mic". PC mute silences the output, and the phone shows "The PC has muted this mic".

## Getting the APK and EXE (built by GitHub for free)

1. Create a new GitHub repository and push this whole folder to it.
2. Open the repo's **Actions** tab. The "Build" workflow runs on every push and takes about 5 minutes.
3. Open the finished run and download **ChudMic-android** (contains `ChudMic.apk`) and **ChudMic-windows** (contains `chudmic.exe`).
4. Optional: push a tag such as `git tag v1.0.0 && git push --tags` to get both files as a GitHub Release.

### Building locally instead
- PC: install Rust from https://rustup.rs, then `cd pc && cargo build --release`. The result is `pc\target\release\chudmic.exe`.
- Android: open the `android` folder in Android Studio and choose Build → Build APK(s).

## Troubleshooting

| Problem | Fix |
|---|---|
| Phone doesn't see the PC | Allow the firewall prompt (Private network). Make sure the Wi-Fi is set as a *Private* network in Windows. Guest networks and "AP/client isolation" block devices from seeing each other. As a fallback, use **Scan QR** or **Enter IP**. |
| "No answer from PC" | Same as above. Also check that only one copy of the PC app is running. |
| Connected but no sound in Discord | Pick **CABLE Output** as the mic in Discord, and **CABLE Input** under "Send audio to:" in ChudMic. |
| Crackles | Move closer to the router, or use 5 GHz Wi-Fi. Phone hotspots and busy 2.4 GHz networks add jitter. |
| Audio stops when the screen is off | Settings → Apps → ChudMic → Battery → *Unrestricted* (needed on Xiaomi, Samsung and similar phones). |

## Testing the PC side without a phone

`python tools/fake_phone.py` sends a 440 Hz tone to the PC app on the same machine. Add an IP to target another PC.

## How it works

```
Phone mic --48 kHz PCM / UDP--> PC app --(jitter buffer, resample)--> CABLE Input ==> CABLE Output --> Discord/OBS
```

- Discovery: UDP broadcast on port **47800**. Audio session: UDP port **47801** on the PC.
- The protocol is described at the top of `pc/src/proto.rs`.
- Pitch shifting (`pc/src/pitch.rs`) uses WSOLA time-stretching followed by resampling: each 40 ms grain is placed where it best matches the previous one, which keeps the voice clean. It runs in the network thread, not the audio callback, and uses well under 1 % CPU.
- The PC keeps about 60 ms of audio buffered and nudges the playback speed by up to ±0.4 % so that phone and PC clock drift never builds up delay.

## Project layout

```
android/   Kotlin app (no AndroidX UI libraries; QR scanning uses zxing-android-embedded, Apache-2.0)
pc/        Rust app: egui (window), cpal (audio), qrcode
tools/     fake_phone.py test sender
.github/   CI that builds the APK and EXE
```

License: MIT.
