# Android Architecture

## Scope

The workspace is an Android-only arm64 client with two crates:

- `ale-core`: portable protocol types, pairing URI validation, Noise transport, the persistent remote-session actor, and a deterministic mock v2 desktop.
- `ale-gui`: Android Activity integration, asynchronous runtime permissions, QR scanning, Oboe recording, Slint UI, and session binding.

There is no iOS application and no local inference or automation engine in this repository.

## Runtime flow

1. The user grants camera access and scans an ephemeral desktop QR code.
2. `PairingInfo` validates the IP address, port, UUID session ID, six-digit code, and optional desktop name.
3. `RemoteSession` opens one WebSocket and completes a Noise `NNpsk0` handshake.
4. The server sends `ServerHello`. Any protocol version other than v2 closes the connection without fallback.
5. One background actor owns the WebSocket and Noise state for the session lifetime. UI tasks communicate with it over a bounded command queue.
6. Oboe requests 48 kHz mono float input. The successful stream's actual sample rate and channel count are validated and sent in `AudioStart`.
7. Every 100 ms the UI drains the recorder, converts samples to little-endian PCM16, and sends chunks of at most 24,576 bytes.
8. `AudioEnd` commits the chunk count, total frame count, and SHA-256. The desktop must validate all three before ASR.
9. The actor routes preview and execution events by request ID. The user may confirm, reject, or cancel through the same session.
10. Disconnect clears recording and pending confirmation state. The user must scan again.

## Bounds and failure behavior

- Recorder buffer: at most two seconds of negotiated audio. Overflow stops the stream and surfaces an error; old samples are never silently removed.
- Recording: at most 60 seconds. The UI displays elapsed time and automatically ends at the limit.
- Noise plaintext per frame: at most 48 KiB; AME2 reassembly permits complete JSON messages up to 1 MiB.
- PCM chunk: at most 24,576 bytes and aligned to complete frames.
- Actor command queue: 64 entries with a five-second enqueue/send deadline.
- Handshake: eight seconds.
- Preview: 90 seconds after `AudioEnd`.
- Confirmation result: 30 seconds. A timeout is an unknown outcome and is never retried automatically.
- Heartbeat: every 15 seconds; disconnect after 30 seconds without an inbound message.
- Unknown or duplicate request responses are logged and ignored. The third occurrence in one session closes it as a protocol violation; known responses that arrive after a local deadline are ignored without a strike.

No path reconnects, replays audio, or replays a command automatically.

## Android platform boundary

`ale-gui/src/lib.rs` is compiled only for Android. Platform operations are narrow:

- `android.rs`: retains the real NativeActivity, runs permission requests and settings intents on the Java UI thread, and reports granted, denied, or permanently denied states.
- `qr_scanner.rs`: Camera2 capture and QR decoding with cancellation-safe result publication.
- `audio.rs`: Oboe input and the bounded PCM buffer.

The Slint UI contains the active remote workflow only.

## Release boundary

The mobile mock server validates this repository's client behavior, but it is not a substitute for the real desktop. Production release remains blocked until a desktop implementation passes the contract in `REMOTE-PROTOCOL-V2.md` and the two-AVD manual matrix in `ANDROID-RELEASE-CHECKLIST.md` is complete.
