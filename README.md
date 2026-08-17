# Ale, My Eyes! Android

This repository contains the Android arm64 LAN command client. It scans a QR code shown by a desktop, opens one encrypted Noise-over-WebSocket session, streams mono PCM16 audio, displays progress and explicit decisions, and sends the user's confirmation or rejection.

The app does not run ASR, language or vision models, memory, or desktop automation locally. Android system `TextToSpeech` plays the desktop-provided, separately redacted `speech_text`; recording and disconnect stop the speech queue. The app has no source or Cargo dependency on the independent desktop repository.

## Compatibility and security

- Android arm64 (`arm64-v8a`) only, minimum API 26.
- Remote protocol v3 only. Older desktops are rejected without fallback.
- The desktop and Android device must be reachable over the same LAN.
- Pairing is memory-only. Every app start or disconnect requires a new QR scan.
- The six-digit pairing code is never persisted.
- No automatic reconnect and no automatic command replay.

Version `0.3.0` must not be released for general use until the desktop and Android client pass the [remote protocol v3](docs/REMOTE-PROTOCOL-V3.md) acceptance matrix.

## Development

```bash
cargo fmt --all -- --check
cargo test -p ale-core
cargo check --workspace
cargo check -p ale-gui --target aarch64-linux-android --lib
```

The Android target check requires the Rust `aarch64-linux-android` target and an Android NDK. A host workspace check does not compile the Android platform code.

Build and verify the signed arm64 APK:

```bash
./scripts/package-android.sh
```

The output directory contains `ale-my-eyes-arm64.apk` and `SHA256SUMS`. Without release signing environment variables, the script creates a temporary test keystore; that artifact is not a production release.

Run local lifecycle smoke tests against the configured Pixel 7 Pro API 34 and Pixel Tablet API 35 arm64 AVDs:

```bash
./scripts/smoke-android-avds.sh ale-my-eyes-android/ale-my-eyes-arm64.apk
```

See [Android architecture](docs/ARCHITECTURE.md), [release checklist](docs/ANDROID-RELEASE-CHECKLIST.md), and [remediation status](docs/ANDROID-ISSUES.md).
