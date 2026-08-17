# Remote Protocol v2

## Status

This document is the required desktop-side contract for Android `0.3.0`. Version 2 is intentionally incompatible with v1. A client or server receiving another version must close the WebSocket and must not fall back.

## Transport and pairing

The QR payload is:

```text
ale-my-eyes://pair?host=<ip>&port=<u16>&sid=<uuid>&code=<six-digits>&name=<display-name>
```

`host` must be a literal IPv4 or IPv6 address. Pairing values are ephemeral and must not be persisted by Android.

The client opens `ws://host:port` and exchanges binary WebSocket frames only. The secure channel uses `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s`. The 32-byte PSK is SHA-256 over the ASCII bytes `ale-my-eyes-remote-v1` followed immediately by the six-digit code. The historical label is intentionally retained so a v1 server can finish Noise and return a readable version mismatch; it does not indicate protocol compatibility.

Handshake order:

1. Client sends the Noise initiator handshake frame.
2. Server sends the Noise responder handshake frame.
3. Server sends encrypted `ServerHello`.
4. Client verifies version and session ID, then sends encrypted `ClientHello`.
5. All later application messages are JSON-serialized and sent as one or more independently Noise-encrypted binary WebSocket frames.

Noise plaintext in one WebSocket frame is at most 48 KiB. A complete JSON message is at most 1 MiB. Both peers must reject an oversized encrypted frame before allocating a decrypted or reassembly buffer.

JSON payloads of at most 48 KiB are encrypted directly. Larger payloads use AME2 framing. Every decrypted AME2 frame begins with this 20-byte big-endian header:

| Offset | Size | Value |
| --- | --- | --- |
| 0 | 4 | ASCII `AME2` |
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
| `command_request` | client to server | `request_id`, `input: {"input":"text","text":string}`; reserved, current Android UI does not emit it |
| `audio_start` | client to server | `request_id: uuid string`, `format: "pcm_s16le"`, `sample_rate_hz: u32`, `channels: u16` |
| `audio_chunk` | client to server | `request_id`, `sequence: u32`, `pcm_base64: string` |
| `audio_end` | client to server | `request_id`, `chunk_count: u32`, `total_frames: u64`, `sha256: lowercase hex string` |
| `cancel_request` | client to server | `request_id` |
| `command_preview` | server to client | `request_id`, `response_text`, `action_steps: string[]`, `confirmation_text`, `requires_confirmation: bool`, `has_plan: bool` |
| `confirm_execution` | client to server | `request_id`, `approved: bool` |
| `execution_status` | server to client | `request_id`, `state`, `message`, `actions_executed: integer` |
| `ping` | either | `nonce: u64` |
| `pong` | either | `nonce: u64` copied from `ping` |
| `error` | either | `request_id: string or null`, `code: string`, `message: string` |

`execution_status.state` is one of `preview_ready`, `executing`, `completed`, `failed`, or `cancelled`.

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

Desktop automation runs outside the WebSocket receive loop and has a 28-second execution deadline, leaving time to deliver `CONFIRM_TIMEOUT` before Android's 30-second deadline. The connection loop must continue answering `Ping` while automation runs. Deadline and cancellation checks occur before and after every action; an already-running operating-system input call may finish before cancellation is observed. Therefore a timeout is not proof that no side effect occurred. Android must present it as an unknown execution outcome and must not automatically retry. A terminal response arriving after Android has retired the request is logged and ignored without counting as a protocol violation.

## Stable error codes

The following codes are protocol-stable:

| Code | Meaning |
| --- | --- |
| `PROTOCOL_INCOMPATIBLE` | Peer is not protocol v2 |
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
cargo run -p ale-core --features mock-server --example mock_remote_v2
```

It prints a pairing URI and validates v2 audio ordering, size, length, and hash before returning deterministic previews and execution status. It performs no ASR or automation.
