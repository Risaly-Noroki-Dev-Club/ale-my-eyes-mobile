# Android Remediation Status

Updated: 2026-08-17

## Release status

The Android-only v2 implementation and matching desktop v2 peer are present in their respective worktrees, but release is blocked. The required API 34/35 AVD manual interoperability workflow has not yet been completed. An item is marked resolved only when its code, automated checks, and applicable AVD evidence are all available.

| Original issue | Implementation status | Evidence still required | Release state |
| --- | --- | --- | --- |
| P0: whole Base64 WAV exceeds one Noise message | PCM16 `AudioStart`/`AudioChunk`/`AudioEnd`, 24,576-byte chunks, length and SHA-256 validation implemented | Real desktop v2 plus 1/10/60-second flows on both AVDs | Blocked |
| P1: preview and confirmation have no deadline | 90-second preview, 30-second confirmation, 5-second send and 8-second handshake deadlines implemented | Timeout UI recovery on both AVDs | Blocked |
| P1: desktop long previews cannot be decoded | Android and desktop now share bounded AME2 framing: 48 KiB per Noise plaintext and 1 MiB per complete JSON message; a 96 KiB mock preview test passes | Long real-desktop preview on both AVDs | Blocked |
| P1: desktop execution outlives Android confirmation timeout | Desktop automation runs outside the WebSocket loop with a 28-second cooperative deadline; Android reports an unknown outcome and ignores known late terminal responses | Timeout, heartbeat, and no-replay workflow on both AVDs | Blocked |
| P1: responses ignore request ID | Exact request routing and cumulative three-strike protocol disconnect implemented | Wrong/old/duplicate-ID desktop interoperability run | Blocked |
| P1: each operation reconnects | One actor owns one persistent WebSocket/Noise session; no automatic reconnect or replay | Real desktop confirmation over the same connection | Blocked |
| P1: sample rate hard-coded | Oboe requests 48 kHz and reports the negotiated rate and channel count; 44.1/48 kHz host tests exist | AVD recording metadata/log evidence | Blocked |
| P1: recording and memory bounds diverge | Two-second non-dropping buffer, 60-second cap, timer, automatic end, and client/server byte limits implemented | 60-second AVD recording on both devices | Blocked |
| P1: no protocol integration tests | Deterministic mock v2 peer covers handshake, v1 rejection, upload, cancellation, timeouts, wrong IDs, disconnect, and confirmation | Real desktop contract suite remains required | Blocked |
| P1: no installable release validation | APK verifier and sequential API 34/35 AVD smoke script implemented and passed with a temporary test certificate | Production signing and manual v2 workflow | Blocked |
| P2: arm64 only | Product scope is explicitly arm64-only; final APK contains only `arm64-v8a` | None for current scope | Resolved |
| P2: pairing is memory-only | Explicit security policy: rescan after start/disconnect and never persist six-digit code | UI behavior on both AVDs | Pending evidence |
| P2: stale scripts and docs | Desktop release/Java scripts and obsolete API/wiki docs removed; Android docs rewritten | None for code scope | Resolved |
| P2: unreachable platform and legacy code | iOS, local inference, old remote client, unused platform modules, and unused Slint screens removed; host and Android checks pass | None for code scope | Resolved |

## Implemented protocol safeguards

- Protocol version is exactly 2; v1 receives no fallback and the UI reports that the desktop is too old.
- Audio accepts mono PCM16 at 8–96 kHz and rejects invalid formats before upload.
- The client enforces frame-aligned chunks, total byte bounds, ordered sequence numbers, total frames, and SHA-256.
- The mock desktop validates missing, duplicate, out-of-order, bad-length, bad-hash, and over-limit input before producing a preview.
- Processing can be cancelled. Disconnect cancels local recording and pending UI and requires a new scan.
- Heartbeats run every 15 seconds and 30 seconds without inbound traffic closes the session.
- Unknown and duplicate responses are logged, ignored, and counted for the session; the third closes it. Responses for a request retired by a local deadline are logged and ignored without a protocol strike.

## Acceptance evidence

Record exact results here only after executing the final worktree:

| Check | Result | Evidence |
| --- | --- | --- |
| `cargo fmt --all -- --check` | Passed | 2026-08-17 final worktree |
| `cargo test -p ale-core` | Passed, 26 tests | Noise, AME2 fragmentation and bounds, golden protocol fixture, audio, timeout, cancellation, request routing, disconnect, confirmation |
| `cargo check --workspace` | Passed | 2026-08-17 final worktree |
| Android arm64 target check | Passed | NDK 30.0.14904198, `aarch64-linux-android` |
| Release APK package and signature | Passed with temporary test certificate | APK Signature Scheme v2/v3; not production signing |
| APK package/permission/ABI inspection | Passed | `com.alemyeyes`, four required permissions, `arm64-v8a` only |
| APK SHA-256 | Passed | `ccd33eb95f7ab3e8fbd47d9988d61f454ea87c22422bb1baaff93cf34ad95a91` |
| Pixel 7 Pro API 34 smoke | Passed | `test-artifacts/android-avd/20260817T121906Z/Pixel_7_Pro` |
| Pixel Tablet API 35 smoke | Passed | `test-artifacts/android-avd/20260817T121906Z/Pixel_Tablet` |
| Manual v2 workflow on both AVDs | Not run | Screenshots and logs |

See `ANDROID-RELEASE-CHECKLIST.md` for the complete release gate.
