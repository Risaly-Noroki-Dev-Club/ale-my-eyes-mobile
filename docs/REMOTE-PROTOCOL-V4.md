# Misaka-Network Remote Protocol v4

## Status

This document is the required desktop-side contract for Android `0.3.0`. Misaka-Network version 4 is intentionally incompatible with older versions. A client or server receiving another version must close the WebSocket and must not fall back.

Version 4 changes the protocol identity together with the message surface:

- QR scheme is `misaka-network://`.
- mDNS service type is `_misaka-network._tcp.local.` (desktop advertisement only).
- PSK label is `misaka-network-remote-v4`.
- Fragmentation magic is `MSKN`.
- `execution_status.state` adds `outcome_unknown` and `needs_user`.
- `command_preview` adds optional `task_mode` and `verification`.

## Transport and pairing

The QR payload is:

```text
misaka-network://pair?host=<ip>&port=<u16>&sid=<uuid>&code=<eight Crockford Base32 chars>&name=<display-name>
```

`host` must be a literal IPv4 or IPv6 address. Pairing values are ephemeral and must not be persisted by Android.

The client opens `ws://host:port` and exchanges binary WebSocket frames only. The secure channel uses `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s`. The pairing code is **eight characters of Crockford Base32** (alphabet `0123456789ABCDEFGHJKMNPQRSTVWXYZ`; the ambiguous letters `I`, `L`, `O` and `U` are never produced), i.e. 40 bits of entropy. The 32-byte PSK is SHA-256 over the ASCII bytes `misaka-network-remote-v4` followed immediately by the upper-case pairing code. Android accepts the code case-insensitively and upper-cases it before deriving the PSK; a code that is not exactly eight characters from that alphabet (including any ambiguous letter) is rejected before a connection is attempted. The label is versioned, so peers built for an older protocol cannot finish Noise; version mismatch error reporting therefore applies only to peers that complete the handshake. Peers that still use the pre-Crockford six-digit code cannot complete the handshake and must be upgraded together.

Handshake order:

1. Client sends the Noise initiator handshake frame.
2. Server sends the Noise responder handshake frame.
3. Server sends encrypted `ServerHello`.
4. Client verifies version and session ID, then sends encrypted `ClientHello`.
5. All later application messages are JSON-serialized and sent as one or more independently Noise-encrypted binary WebSocket frames.

Noise plaintext in one WebSocket frame is at most 48 KiB. A complete JSON message is at most 1 MiB. Both peers must reject an oversized encrypted frame before allocating a decrypted or reassembly buffer.

JSON payloads of at most 48 KiB are encrypted directly. Larger payloads use `MSKN` framing. Every decrypted `MSKN` frame begins with this 20-byte big-endian header:

| Offset | Size | Value |
| --- | --- | --- |
| 0 | 4 | ASCII `MSKN` |
| 4 | 8 | Random message ID (`u64`) |
| 12 | 4 | Zero-based chunk index (`u32`) |
| 16 | 4 | Total chunk count (`u32`, non-zero) |

The remaining bytes are JSON payload bytes. Each non-final fragment carries at most `48 KiB - 20` payload bytes. WebSocket ordering is authoritative: fragments must be contiguous, use one message ID and total count, and arrive with indices `0..total_chunks-1`. Missing, duplicate, interleaved, restarted, or out-of-order fragments terminate the connection as a protocol error. Reassembly must enforce the 1 MiB total before appending each fragment. JSON decoding occurs only after the final fragment.

## Message envelope

Every complete, optionally reassembled payload is one JSON object with a snake-case `type` discriminator. Fields shown below are required unless marked optional.

| Type | Direction | Fields |
| --- | --- | --- |
| `client_hello` | client to server | `protocol_version: u32`, `device_name: string` |
| `server_hello` | server to client | `protocol_version: u32`, `device_name: string`, `session_id: uuid string` |
| `command_request` | client to server | `request_id`, nested `input: { input: "text", text: string }` |
| `audio_start` | client to server | `request_id: uuid string`, `format: "pcm_s16le"`, `sample_rate_hz: u32`, `channels: u16` |
| `audio_chunk` | client to server | `request_id`, `sequence: u32`, `pcm_base64: string` |
| `audio_end` | client to server | `request_id`, `chunk_count: u32`, `total_frames: u64`, `sha256: lowercase hex string` |
| `cancel_request` | client to server | `request_id` |
| `progress_update` | server to client | `request_id`, `stage`, `message` |
| `decision_request` | server to client | `request_id`, `decision_id`, `kind`, `prompt`, `expires_in_seconds` |
| `decision_response` | client to server | `request_id`, `decision_id`, `approved: bool` |
| `assistant_output` | server to client | optional `request_id`, `kind`, `display_text`, `speech_text`, `interrupt`, `sensitive` |
| `command_preview` | server to client | `request_id`, `response_text`, `action_steps: string[]`, `confirmation_text`, `requires_confirmation: bool`, `has_plan: bool`, optional `task_mode: "legacy" or "verified"`, optional `verification: string[]` |
| `confirm_execution` | client to server | `request_id`, `approved: bool` |
| `execution_status` | server to client | `request_id`, `state`, `message`, `actions_executed: integer` |
| `ping` | either | `nonce: u64` |
| `pong` | either | `nonce: u64` copied from `ping` |
| `error` | either | `request_id: string or null`, `code: string`, `message: string` |

`command_preview.task_mode` and `command_preview.verification` are optional v4 additions. A sender that omits them is still valid, and receivers must default to `null`. `task_mode` is `legacy` or `verified`. `verification` holds human-readable postconditions for the mobile confirmation card; it is display text, not a machine-checked contract.

`execution_status.state` is one of `preview_ready`, `executing`, `completed`, `failed`, `cancelled`, `outcome_unknown`, or `needs_user`.

- `completed`: the confirmed operation finished and its postconditions held.
- `failed`: the operation finished or was rejected by the desktop without an unknown side effect.
- `cancelled`: the request was explicitly cancelled or rejected before execution.
- `outcome_unknown`: execution timed out, was interrupted, or otherwise could have had an effect. Android must display `结果不确定（可能已执行，请勿自动重试）` and must never replay the request automatically.
- `needs_user`: a verified task requires a human decision (for example the target window changed). Android must display `需要人工接手`.

`progress_update.stage` remains a snake-case enum. Version 4 verified-task paths may emit the additional stages `verified_step`, `verified_verify`, and `verified_recover`; Android displays `progress_update.message` and does not depend on the stage string.

`decision_request.kind` is one of `use_remote_model`, `upload_full_screenshot`, or `risk_changed`. Android must retain visible yes/no controls until it responds, the request expires, or the session disconnects. Voice yes/no may be transcribed only by the desktop SenseVoice path; if that path is unavailable, the buttons remain mandatory and cloud ASR is not used for the decision.

`assistant_output.speech_text` is a separately sanitized value for Android system TTS. Passwords, keys, complete typed text, and sensitive paths must not appear in it. Confirmation and error output may interrupt ordinary speech; progress output is coalesced. Starting recording or disconnecting clears the speech and decision queues.

Example audio start:

```json
{"type":"audio_start","request_id":"34784ee8-4cc4-4fb4-a81b-9b3b9043e87c","format":"pcm_s16le","sample_rate_hz":48000,"channels":1}
```

## Audio rules

- Encoding is signed 16-bit little-endian PCM; no WAV container and no Opus.
- Exactly one channel is accepted.
- Actual sample rate must be 8,000 through 96,000 Hz inclusive.
- `sequence` starts at 0 and increases by exactly one.
- Decoded `pcm_base64` is non-empty, no more than 24,576 bytes, and aligned to a complete PCM frame.
- Total audio is no more than 60 seconds according to `sample_rate_hz`, `channels`, and decoded byte count.
- `chunk_count` equals the next expected sequence number.
- `total_frames` equals decoded bytes divided by `channels * 2`.
- `sha256` is the lowercase SHA-256 hex digest of the concatenated raw PCM bytes in sequence order.

The server must keep a bounded per-request assembler. It must not invoke ASR until `AudioEnd` passes sequence, length, limit, and hash validation. Any failure releases accumulated audio and returns an error.

## Request state

Only one audio upload is active per Android session. Responses are correlated by exact request ID. Unknown, expired, or duplicate IDs must not mutate another request.

After a valid `AudioEnd`, the server either returns `CommandPreview` or `Error`. If `has_plan` is true, the plan remains pending until `ConfirmExecution`, `CancelRequest`, session disconnect, or server expiry. `approved=false` is an explicit rejection and returns a terminal `ExecutionStatus` with state `cancelled`.

`CancelRequest` is idempotent. It prevents any later ASR result or not-yet-started automation from being published or executed for that request. Active automation must observe a cooperative cancellation token between actions. A disconnected session cancels every active and pending request. Neither side may automatically replay a request on a new connection.

## Timing

Android enforces:

- WebSocket plus Noise handshake: 8 seconds.
- Send/backpressure operation: 5 seconds.
- Preview after `AudioEnd`: 90 seconds.
- Final status after `ConfirmExecution`: 30 seconds.
- `Ping`: every 15 seconds.
- Disconnect: 30 seconds with no inbound message.

The desktop should expire state no later than the corresponding client deadline and should answer `Ping` immediately.

Desktop automation runs outside the WebSocket receive loop and has a 28-second execution deadline, leaving time to deliver `CONFIRM_TIMEOUT` before Android's 30-second deadline. The connection loop must continue answering `Ping` while automation runs. Deadline and cancellation checks occur before and after every action; an already-running operating-system input call may finish before cancellation is observed. Therefore a timeout is not proof that no side effect occurred. Android must present it as an unknown execution outcome (`outcome_unknown` or the local `CONFIRM_TIMEOUT` equivalent) and must not automatically retry. A terminal response arriving after Android has retired the request is logged and ignored without counting as a protocol violation.

Verified tasks run inside the same confirmation window: the desktop must report `completed`, `failed`, `cancelled`, `outcome_unknown`, or `needs_user` within 30 seconds of confirmation, otherwise Android treats the outcome as unknown.

## Stable error codes

The following codes are protocol-stable:

| Code | Meaning |
| --- | --- |
| `PROTOCOL_INCOMPATIBLE` | Peer is not Misaka-Network protocol v4 |
| `REQUEST_TIMEOUT` | Preview was not produced before its deadline |
| `CONFIRM_TIMEOUT` | Execution status was not produced before its deadline |
| `AUDIO_TOO_LARGE` | Audio exceeds the 60-second or byte limit |
| `INVALID_AUDIO_SEQUENCE` | Chunk sequence or final chunk count is missing, duplicate, or out of order |
| `AUDIO_HASH_MISMATCH` | Final PCM SHA-256 does not match |
| `CANCELLED` | Request was cancelled or rejected |
| `CONNECTION_INTERRUPTED` | Transport closed before completion |
| `PROTOCOL_VIOLATION` | Repeated unrouteable or invalid messages |

Implementations may also use `UNSUPPORTED_AUDIO_FORMAT`, `INVALID_AUDIO_CHUNK`, `INVALID_AUDIO_LENGTH`, `UNKNOWN_AUDIO_REQUEST`, `AUDIO_BUSY`, `EMPTY_AUDIO`, and `INTERNAL_ERROR`. Error text is diagnostic and may be localized; logic must use `code`.

## Deterministic reference peer

Run the host mock server with:

```bash
cargo run -p ale-core --features mock-server --example mock_remote_v4
```

It prints a Misaka-Network v4 pairing URI and validates v4 audio ordering, size, length, and hash before returning deterministic decisions, previews, and execution status. It performs no ASR or automation.

The test suite additionally uses the mock peer's confirmation behaviors to return the v4 terminal states `outcome_unknown` and `needs_user`.
