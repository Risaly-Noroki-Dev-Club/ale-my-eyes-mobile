# Android Architecture

## Scope

The workspace is an Android-only arm64 client with two crates:

- `ale-core`: portable protocol types, pairing URI validation, Noise transport, the persistent remote-session actor, and a deterministic Misaka-Network v4 mock desktop.
- `ale-gui`: Android Activity integration, asynchronous runtime permissions, QR scanning, Oboe recording, Slint UI, and session binding.

There is no iOS application and no local inference or automation engine in this repository.

## Runtime flow

1. The user grants camera access and scans an ephemeral desktop QR code.
2. `PairingInfo` validates the IP address, port, UUID session ID, eight-character Crockford Base32 pairing code (case-insensitive, normalized to upper case), and optional desktop name.
3. `RemoteSession` opens one WebSocket and completes a Noise `NNpsk0` handshake.
4. The server sends `ServerHello`. Any protocol version other than Misaka-Network v4 closes the connection without fallback.
5. One background actor owns the WebSocket and Noise state for the session lifetime. UI tasks communicate with it over a bounded command queue.
6. Oboe requests 48 kHz mono float input. The successful stream's actual sample rate and channel count are validated and sent in `AudioStart`.
7. Every 100 ms the UI drains the recorder, converts samples to little-endian PCM16, and sends chunks of at most 24,576 bytes.
8. `AudioEnd` commits the chunk count, total frame count, and SHA-256. The desktop must validate all three before ASR.
9. The actor routes progress, decisions, assistant output, preview, and execution events by request ID. Client-initiated requests stay known for 180 seconds, so desktop follow-up speech and progress messages sent after the preview or terminal status still reach the UI. Decisions always retain visible yes/no buttons.
10. Android system TTS plays only the server-provided `speech_text`; confirmation and errors interrupt ordinary speech, while recording and disconnect stop the queue.
11. Disconnect clears recording, speech, decision, and pending confirmation state. The user must scan again.

## Bounds and failure behavior

- Recorder buffer: at most two seconds of negotiated audio. Only a genuine size overflow (or a poisoned lock) stops the stream; ordinary lock contention with the UI drain retries instead of aborting the recording, and old samples are never silently removed.
- Recording: hard protocol limit of 60 seconds. The UI displays elapsed time, starts the clock before the stream buffers any audio, automatically ends capture at 58 seconds, and truncates the final chunk so the upload never exceeds `sample_rate * channels * 2 * 60` bytes.
- Noise plaintext per frame: at most 48 KiB; MSKN reassembly permits complete JSON messages up to 1 MiB.
- PCM chunk: at most 24,576 bytes and aligned to complete frames.
- Actor command queue: 64 entries with a five-second enqueue/send deadline.
- Handshake: eight seconds for the whole exchange (connect, Noise handshake, ServerHello, ClientHello).
- Preview: 90 seconds after `AudioEnd`.
- Confirmation result: 30 seconds. A timeout is an unknown outcome (`outcome_unknown`) and is never retried automatically.
- Heartbeat: every 15 seconds; disconnect after 30 seconds without an inbound message.
- Unknown or duplicate request responses are logged and ignored. Any handled message resets the strike counter, and three *consecutive* unrouteable messages close the session as a protocol violation. Requests stay known for 180 seconds (at most 64 ids), so desktop follow-up messages for a known request are routed instead of striking; requests retired by a deadline, timeout, or cancel are ignored without a strike.
- `Error` messages with a `null` request_id are session-level errors: reported to the UI without closing the session and without a strike.

No path reconnects, replays audio, or replays a command automatically.

## Android platform boundary

`ale-gui/src/lib.rs` is compiled only for Android. Platform operations are narrow:

- `android.rs`: retains the real NativeActivity, runs permission requests and settings intents on the Java UI thread, and reports granted, denied, or permanently denied states.
- `qr_scanner.rs`: Camera2 capture and QR decoding with cancellation-safe result publication. The display rotation is read once per scan, and the repeating request starts after the capture session reports `onReady` (with a bounded fallback).
- `audio.rs`: Oboe input and the bounded PCM buffer.

The Slint UI contains the active remote workflow only.

## Release boundary

The mobile mock server validates this repository's client behavior, but it is not a substitute for the real desktop. Production release remains blocked until a desktop implementation passes the contract in `REMOTE-PROTOCOL-V4.md` and the two-AVD manual matrix in `ANDROID-RELEASE-CHECKLIST.md` is complete.
