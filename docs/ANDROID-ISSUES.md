# Android Remediation Status

Updated: 2026-10-29 (mobile 修复批次；文末 AVD/真机证据是 2026-08 的历史记录，本批次未重新执行)

## Release status

The Android-only Misaka-Network v4 implementation and matching desktop v4 peer are present in their respective worktrees, but release is blocked. The required API 34/35 AVD manual interoperability workflow has not yet been completed. An item is marked resolved only when its code, automated checks, and applicable AVD evidence are all available.

| Original issue | Implementation status | Evidence still required | Release state |
| --- | --- | --- | --- |
| P0: whole Base64 WAV exceeds one Noise message | PCM16 `AudioStart`/`AudioChunk`/`AudioEnd`, 24,576-byte chunks, length and SHA-256 validation implemented | Real desktop v4 plus 1/10/60-second flows on both AVDs | Blocked |
| P1: preview and confirmation have no deadline | 90-second preview, 30-second confirmation, 5-second send and 8-second handshake deadlines implemented | Timeout UI recovery on both AVDs | Blocked |
| P1: desktop long previews cannot be decoded | Android and desktop now share bounded MSKN framing: 48 KiB per Noise plaintext and 1 MiB per complete JSON message; a 96 KiB mock preview test passes | Long real-desktop preview on both AVDs | Blocked |
| P1: desktop execution outlives Android confirmation timeout | Desktop automation runs outside the WebSocket loop with a 28-second cooperative deadline; Android reports an unknown outcome and ignores known late terminal responses | Timeout, heartbeat, and no-replay workflow on both AVDs | Blocked |
| P0: cross-end message order | Desktop follow-up messages (`AssistantOutput` after preview/terminal, `ProgressUpdate` after confirmation) are routed inside a bounded known-request window (180 s, 64 ids); handled messages reset the consecutive-strike counter; retired/expired requests are ignored without a strike; the mock v4 peer now replays the real desktop sequence | Real desktop preview -> confirm -> progress -> result run on both AVDs | Blocked |
| P1: responses ignore request ID | Exact request routing plus consecutive three-strike protocol disconnect implemented; known requests are retired on timeout/cancel | Wrong/old/duplicate-ID desktop interoperability run | Blocked |
| P1: each operation reconnects | One actor owns one persistent WebSocket/Noise session; no automatic reconnect or replay | Real desktop confirmation over the same connection | Blocked |
| P1: sample rate hard-coded | Oboe requests 48 kHz and reports the negotiated rate and channel count; 44.1/48 kHz host tests exist | AVD recording metadata/log evidence | Blocked |
| P1: recording and memory bounds diverge | Two-second non-dropping buffer, 60-second cap, timer, automatic end at 58 s, and last-chunk truncation so the upload never exceeds `rate * channels * 2 * 60` bytes | 60-second AVD recording on both devices | Blocked |
| P1: no protocol integration tests | Deterministic mock v4 peer covers handshake (including a whole-exchange deadline), older-version rejection, upload, empty audio cancellation, decisions, cancellation, timeouts, wrong IDs, disconnect, confirmation, session-level errors, late messages after cancel, and the desktop message order | Real desktop contract suite remains required | Blocked |
| P1: no installable release validation | APK verifier and sequential API 34/35 AVD smoke script previously passed with a temporary test certificate | Rebuild, production signing, and manual v4 workflow | Blocked |
| P2: arm64 only | Product scope is explicitly arm64-only; final APK contains only `arm64-v8a` | None for current scope | Resolved |
| P2: pairing is memory-only | Explicit security policy: rescan after start/disconnect and never persist the eight-character Crockford Base32 pairing code | UI behavior on both AVDs | Pending evidence |
| P2: stale scripts and docs | Desktop release/Java scripts and obsolete API/wiki docs removed; Android docs rewritten | None for code scope | Resolved |
| P2: unreachable platform and legacy code | iOS, local inference, old remote client, unused platform modules, and unused Slint screens removed; host and Android checks pass | None for code scope | Resolved |

## Implemented protocol safeguards

- Protocol version is exactly 4 (Misaka-Network v4); older peers receive no fallback and the UI reports that the desktop is too old.
- Audio accepts mono PCM16 at 8–96 kHz and rejects invalid formats before upload.
- The client enforces frame-aligned chunks, total byte bounds, ordered sequence numbers, total frames, and SHA-256.
- The mock desktop validates missing, duplicate, out-of-order, bad-length, bad-hash, and over-limit input before producing a preview.
- Processing can be cancelled. Disconnect cancels local recording and pending UI and requires a new scan.
- Heartbeats run every 15 seconds and 30 seconds without inbound traffic closes the session.
- Every client-initiated request stays known for 180 seconds (bounded to 64 ids). Desktop follow-up `AssistantOutput` speech, `ProgressUpdate`, and `DecisionRequest` messages for a known request are routed to the UI instead of counting as protocol faults.
- Unknown or duplicate responses are logged and ignored, and only three *consecutive* unrouteable messages close the session. Any handled message resets that run, matching the desktop server.
- Requests retired by a local deadline, timeout, or cancel are logged and ignored without a protocol strike.
- `Error` messages with a `null` request_id are protocol-legal session-level errors: they are surfaced in the UI and never count as strikes.
- An empty recording sends `CancelRequest` before reporting `EMPTY_AUDIO`, so the desktop assembler cannot leak into the next request.
- The client stops recording at 58 seconds and truncates the final chunk so the upload can never exceed the 60-second byte limit.

## Acceptance evidence

Record exact results here only after executing the final worktree. A result is auditable only when the command, toolchain, date, and a log committed to the repository (or attached to the release record) are recorded; paths under the git-ignored `test-artifacts/` directory are **not** auditable on their own.

| Check | Result | Evidence |
| --- | --- | --- |
| `cargo fmt --all -- --check` | Passed | 2026-10-29, remote host (Ubuntu x86_64, Rust 1.95), log in the fix record `output/fixes-20261029/mobile/changes.md` (outside this repository) |
| `cargo test -p ale-core --all-features` | Passed, 38 tests | 2026-10-29 remote host; covers Noise, MSKN fragmentation/bounds, v4 golden fixture, audio, decisions, timeouts, cancellation, request routing, desktop message order, empty audio, session errors, disconnect, confirmation |
| `cargo clippy -p ale-core --all-targets --locked -- -D warnings` | Passed | 2026-10-29 remote host |
| `cargo check --workspace --locked` | Passed | 2026-10-29 remote host |
| Android arm64 target check | Not re-run in this batch | CI and `scripts/package-android.sh` pin NDK 27.3.13750724; the previous record used NDK 30.0.14904198 on a macOS host. The two environments are not equivalent: the release gate must re-run this check with the pinned CI NDK (no NDK was available on the 2026-10-29 remote host) |
| Release APK package and signature | Historical only, not reproven | 2026-08-17 record used a temporary test certificate; no APK artifact is committed, so it cannot be audited from this repository. Rebuild required before release |
| APK package/permission/ABI inspection | Historical only, not reproven | Same as above; rerun `scripts/verify-android-apk.sh` on the release APK |
| APK SHA-256 | Must be regenerated | Source changed after the previous test APK |
| Pixel 7 Pro API 34 smoke | Historical record only, not auditable | The referenced `test-artifacts/android-avd/...` directory is git-ignored and absent from the repository; rerun the sequential AVD smoke script and attach logs/screenshots to the release record |
| Pixel Tablet API 35 smoke | Historical record only, not auditable | Same as above |
| Manual v4 workflow on both AVDs | Not run | No emulator/KVM or Android device was available for this batch; every row is still release-blocking |

See `ANDROID-RELEASE-CHECKLIST.md` for the complete release gate.
