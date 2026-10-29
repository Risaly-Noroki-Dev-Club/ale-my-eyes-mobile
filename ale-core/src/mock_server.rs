use crate::remote::{
    AssistantOutput, AssistantOutputKind, AudioEnd, AudioFormat, AudioStart, CommandPreview,
    DecisionKind, DecisionRequest, ExecutionState, ExecutionStatus, PairingInfo, Ping, Pong,
    ProgressStage, ProgressUpdate, RemoteError, RemoteMessage, ServerHello, MAX_AUDIO_CHUNK_BYTES,
    MAX_RECORDING_SECONDS, MAX_SAMPLE_RATE_HZ, MIN_SAMPLE_RATE_HZ,
};
use crate::remote_crypto::{server_handshake_reply, SecureChannel};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockBehavior {
    Normal,
    WrongPreviewIds,
    LongPreview,
    SilentPreview,
    SilentConfirmation,
    LateConfirmation,
    DecisionFlow,
    OutcomeUnknown,
    NeedsUser,
    /// Replays the real desktop order: preview, preview speech, confirmation
    /// progress, terminal status, result speech. Two stale trailing speech
    /// messages keep the strike counter under test (a handled message must
    /// reset it, otherwise three rounds would disconnect).
    DesktopSequence,
    /// Delays the Noise handshake past the client's whole-handshake deadline.
    SlowHandshake,
    /// Sends protocol-legal session-level errors (`request_id: null`).
    NullSessionErrors,
    /// Sends queued messages for a request after the client cancelled it.
    LateAfterCancel,
}

pub struct MockRemoteServer {
    pub pairing: PairingInfo,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl MockRemoteServer {
    pub async fn start(protocol_version: u32, behavior: MockBehavior) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let pairing = PairingInfo {
            host: address.ip().to_string(),
            port: address.port(),
            session_id: uuid::Uuid::new_v4().to_string(),
            // Valid eight-character Crockford Base32 so the printed pairing URI
            // passes the real client validation (`PairingInfo::from_uri`).
            code: "M4SKN7Q2".to_string(),
            name: "Mock Desktop v4".to_string(),
        };
        let server_pairing = pairing.clone();
        let (shutdown, mut shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            tokio::select! {
                accepted = listener.accept() => {
                    if let Ok((stream, address)) = accepted {
                        if let Err(error) = handle_connection(
                            stream,
                            address,
                            server_pairing,
                            protocol_version,
                            behavior,
                        ).await {
                            tracing::warn!("Mock remote connection failed: {}", error);
                        }
                    }
                }
                _ = &mut shutdown_rx => {}
            }
        });
        Ok(Self {
            pairing,
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    pub async fn wait(mut self) {
        self.shutdown.take();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for MockRemoteServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn handle_connection(
    stream: TcpStream,
    _address: SocketAddr,
    pairing: PairingInfo,
    protocol_version: u32,
    behavior: MockBehavior,
) -> Result<(), String> {
    let mut socket = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|error| error.to_string())?;
    if behavior == MockBehavior::SlowHandshake {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    let handshake = socket
        .next()
        .await
        .ok_or_else(|| "missing handshake".to_string())?
        .map_err(|error| error.to_string())?
        .into_data();
    let (mut secure, reply) = server_handshake_reply(&pairing.code, &handshake)?;
    socket
        .send(Message::Binary(reply))
        .await
        .map_err(|error| error.to_string())?;
    send_secure(
        &mut socket,
        &mut secure,
        &RemoteMessage::ServerHello(ServerHello {
            protocol_version,
            device_name: pairing.name,
            session_id: pairing.session_id,
        }),
    )
    .await?;
    if protocol_version != crate::remote::REMOTE_PROTOCOL_VERSION {
        return Ok(());
    }

    let mut audio: Option<AudioAssembler> = None;
    let mut client_hello_received = false;
    let mut pending_decision: Option<(String, String)> = None;
    while let Some(frame) = socket.next().await {
        let frame = frame.map_err(|error| error.to_string())?;
        if !frame.is_binary() {
            continue;
        }
        let Some(message) = secure.decrypt_frame(&frame.into_data())? else {
            continue;
        };
        match message {
            RemoteMessage::ClientHello(hello) => {
                if client_hello_received
                    || hello.protocol_version != crate::remote::REMOTE_PROTOCOL_VERSION
                {
                    return Err("invalid ClientHello".to_string());
                }
                client_hello_received = true;
            }
            _ if !client_hello_received => return Err("ClientHello required".to_string()),
            RemoteMessage::AudioStart(start) => {
                if audio.is_some() {
                    send_error(
                        &mut socket,
                        &mut secure,
                        remote_error(Some(start.request_id), "AUDIO_BUSY", "已有音频请求"),
                    )
                    .await?;
                } else {
                    match AudioAssembler::new(start) {
                        Ok(assembler) => audio = Some(assembler),
                        Err(remote) => send_error(&mut socket, &mut secure, remote).await?,
                    }
                }
            }
            RemoteMessage::AudioChunk(chunk) => {
                let result = audio
                    .as_mut()
                    .ok_or_else(|| {
                        remote_error(
                            Some(chunk.request_id.clone()),
                            "UNKNOWN_AUDIO_REQUEST",
                            "音频请求不存在",
                        )
                    })
                    .and_then(|audio| audio.push(chunk));
                if let Err(remote) = result {
                    audio = None;
                    send_error(&mut socket, &mut secure, remote).await?;
                }
            }
            RemoteMessage::AudioEnd(end) => {
                let result = audio
                    .take()
                    .ok_or_else(|| {
                        remote_error(
                            Some(end.request_id.clone()),
                            "UNKNOWN_AUDIO_REQUEST",
                            "音频请求不存在",
                        )
                    })
                    .and_then(|audio| audio.finish(&end));
                match result {
                    Ok(()) if behavior == MockBehavior::SilentPreview => {}
                    Ok(()) if behavior == MockBehavior::WrongPreviewIds => {
                        for suffix in 0..3 {
                            send_secure(
                                &mut socket,
                                &mut secure,
                                &RemoteMessage::CommandPreview(preview(format!("wrong-{suffix}"))),
                            )
                            .await?;
                        }
                    }
                    Ok(()) if behavior == MockBehavior::LongPreview => {
                        let mut value = preview(end.request_id);
                        value.response_text = "x".repeat(96 * 1024);
                        send_secure(
                            &mut socket,
                            &mut secure,
                            &RemoteMessage::CommandPreview(value),
                        )
                        .await?;
                    }
                    Ok(()) if behavior == MockBehavior::DecisionFlow => {
                        let decision_id = uuid::Uuid::new_v4().to_string();
                        pending_decision = Some((end.request_id.clone(), decision_id.clone()));
                        send_secure(
                            &mut socket,
                            &mut secure,
                            &RemoteMessage::DecisionRequest(DecisionRequest {
                                request_id: end.request_id,
                                decision_id,
                                kind: DecisionKind::UseRemoteModel,
                                prompt: "use remote".to_string(),
                                expires_in_seconds: 30,
                            }),
                        )
                        .await?;
                    }
                    Ok(()) if behavior == MockBehavior::DesktopSequence => {
                        let request_id = end.request_id;
                        send_secure(
                            &mut socket,
                            &mut secure,
                            &RemoteMessage::CommandPreview(preview(request_id.clone())),
                        )
                        .await?;
                        // The real desktop always appends one speech message
                        // right after the preview.
                        send_secure(
                            &mut socket,
                            &mut secure,
                            &RemoteMessage::AssistantOutput(AssistantOutput {
                                request_id: Some(request_id),
                                kind: AssistantOutputKind::Confirmation,
                                display_text: "预览文本".to_string(),
                                speech_text: "预览播报".to_string(),
                                interrupt: false,
                                sensitive: false,
                            }),
                        )
                        .await?;
                    }
                    Ok(()) if behavior == MockBehavior::NullSessionErrors => {
                        for _ in 0..3 {
                            send_error(
                                &mut socket,
                                &mut secure,
                                remote_error(None, "INTERNAL_ERROR", "会话级错误"),
                            )
                            .await?;
                        }
                        send_secure(
                            &mut socket,
                            &mut secure,
                            &RemoteMessage::CommandPreview(preview(end.request_id)),
                        )
                        .await?;
                    }
                    Ok(()) => {
                        send_secure(
                            &mut socket,
                            &mut secure,
                            &RemoteMessage::CommandPreview(preview(end.request_id)),
                        )
                        .await?;
                    }
                    Err(remote) => send_error(&mut socket, &mut secure, remote).await?,
                }
            }
            RemoteMessage::ConfirmExecution(confirm)
                if behavior != MockBehavior::SilentConfirmation =>
            {
                if behavior == MockBehavior::DesktopSequence {
                    let request_id = confirm.request_id.clone();
                    // Confirmation -> progress -> terminal status -> result
                    // speech, mirroring the real desktop message order.
                    send_secure(
                        &mut socket,
                        &mut secure,
                        &RemoteMessage::ProgressUpdate(ProgressUpdate {
                            request_id: request_id.clone(),
                            stage: ProgressStage::Executing,
                            message: "正在执行".to_string(),
                        }),
                    )
                    .await?;
                    send_secure(
                        &mut socket,
                        &mut secure,
                        &RemoteMessage::ExecutionStatus(ExecutionStatus {
                            request_id: request_id.clone(),
                            state: if confirm.approved {
                                ExecutionState::Completed
                            } else {
                                ExecutionState::Cancelled
                            },
                            message: if confirm.approved {
                                "done"
                            } else {
                                "cancelled"
                            }
                            .to_string(),
                            actions_executed: usize::from(confirm.approved),
                        }),
                    )
                    .await?;
                    send_secure(
                        &mut socket,
                        &mut secure,
                        &RemoteMessage::AssistantOutput(AssistantOutput {
                            request_id: Some(request_id),
                            kind: AssistantOutputKind::Result,
                            display_text: "结果文本".to_string(),
                            speech_text: "结果播报".to_string(),
                            interrupt: true,
                            sensitive: false,
                        }),
                    )
                    .await?;
                    // Trailing unknown speech also exercises the strike reset:
                    // a handled message must clear the run of stray messages.
                    for suffix in 0..2 {
                        send_secure(
                            &mut socket,
                            &mut secure,
                            &RemoteMessage::AssistantOutput(AssistantOutput {
                                request_id: Some(format!("stale-{suffix}")),
                                kind: AssistantOutputKind::Information,
                                display_text: "stale".to_string(),
                                speech_text: "stale".to_string(),
                                interrupt: false,
                                sensitive: false,
                            }),
                        )
                        .await?;
                    }
                } else {
                    if behavior == MockBehavior::LateConfirmation {
                        tokio::time::sleep(std::time::Duration::from_millis(850)).await;
                    }
                    send_secure(
                        &mut socket,
                        &mut secure,
                        &RemoteMessage::ExecutionStatus(ExecutionStatus {
                            request_id: confirm.request_id,
                            state: if !confirm.approved {
                                ExecutionState::Cancelled
                            } else {
                                match behavior {
                                    MockBehavior::OutcomeUnknown => ExecutionState::OutcomeUnknown,
                                    MockBehavior::NeedsUser => ExecutionState::NeedsUser,
                                    _ => ExecutionState::Completed,
                                }
                            },
                            message: if !confirm.approved {
                                "cancelled"
                            } else {
                                match behavior {
                                    MockBehavior::OutcomeUnknown => "outcome unknown",
                                    MockBehavior::NeedsUser => "needs user",
                                    _ => "done",
                                }
                            }
                            .to_string(),
                            actions_executed: usize::from(confirm.approved),
                        }),
                    )
                    .await?;
                }
            }
            RemoteMessage::ConfirmExecution(_) => {}
            RemoteMessage::DecisionResponse(response) => {
                if pending_decision.as_ref()
                    != Some(&(response.request_id.clone(), response.decision_id.clone()))
                {
                    return Err("unexpected decision response".to_string());
                }
                pending_decision.take();
                if response.approved {
                    send_secure(
                        &mut socket,
                        &mut secure,
                        &RemoteMessage::CommandPreview(preview(response.request_id)),
                    )
                    .await?;
                } else {
                    send_error(
                        &mut socket,
                        &mut secure,
                        remote_error(Some(response.request_id), "CANCELLED", "decision rejected"),
                    )
                    .await?;
                }
            }
            RemoteMessage::CancelRequest(cancel) if behavior == MockBehavior::LateAfterCancel => {
                if audio
                    .as_ref()
                    .is_some_and(|value| value.request_id == cancel.request_id)
                {
                    audio = None;
                }
                for message in late_messages_after_cancel(&cancel.request_id) {
                    send_secure(&mut socket, &mut secure, &message).await?;
                }
            }
            RemoteMessage::CancelRequest(cancel)
                if audio
                    .as_ref()
                    .is_some_and(|value| value.request_id == cancel.request_id) =>
            {
                audio = None;
            }
            RemoteMessage::CancelRequest(_) => {}
            RemoteMessage::Ping(Ping { nonce }) => {
                send_secure(
                    &mut socket,
                    &mut secure,
                    &RemoteMessage::Pong(Pong { nonce }),
                )
                .await?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Messages a real desktop can still have queued when a cancel reaches it.
fn late_messages_after_cancel(request_id: &str) -> Vec<RemoteMessage> {
    vec![
        RemoteMessage::CommandPreview(preview(request_id.to_string())),
        RemoteMessage::ProgressUpdate(ProgressUpdate {
            request_id: request_id.to_string(),
            stage: ProgressStage::Executing,
            message: "late progress".to_string(),
        }),
        RemoteMessage::ExecutionStatus(ExecutionStatus {
            request_id: request_id.to_string(),
            state: ExecutionState::Completed,
            message: "late status".to_string(),
            actions_executed: 0,
        }),
        RemoteMessage::AssistantOutput(AssistantOutput {
            request_id: Some(request_id.to_string()),
            kind: AssistantOutputKind::Result,
            display_text: "late result".to_string(),
            speech_text: "late speech".to_string(),
            interrupt: false,
            sensitive: false,
        }),
        RemoteMessage::DecisionRequest(DecisionRequest {
            request_id: request_id.to_string(),
            decision_id: uuid::Uuid::new_v4().to_string(),
            kind: DecisionKind::UseRemoteModel,
            prompt: "late decision".to_string(),
            expires_in_seconds: 30,
        }),
    ]
}

fn preview(request_id: String) -> CommandPreview {
    CommandPreview {
        request_id,
        response_text: "mock preview".to_string(),
        action_steps: vec!["mock action".to_string()],
        confirmation_text: "confirm mock action".to_string(),
        requires_confirmation: true,
        has_plan: true,
        task_mode: Some("legacy".to_string()),
        verification: None,
    }
}

struct AudioAssembler {
    request_id: String,
    sample_rate_hz: u32,
    channels: u16,
    next_sequence: u32,
    pcm: Vec<u8>,
}

impl AudioAssembler {
    fn new(start: AudioStart) -> Result<Self, RemoteError> {
        if start.format != AudioFormat::PcmS16Le
            || !(MIN_SAMPLE_RATE_HZ..=MAX_SAMPLE_RATE_HZ).contains(&start.sample_rate_hz)
            || start.channels != 1
        {
            return Err(remote_error(
                Some(start.request_id),
                "UNSUPPORTED_AUDIO_FORMAT",
                "仅支持 8–96 kHz 单声道 PCM16",
            ));
        }
        Ok(Self {
            request_id: start.request_id,
            sample_rate_hz: start.sample_rate_hz,
            channels: start.channels,
            next_sequence: 0,
            pcm: Vec::new(),
        })
    }

    fn push(&mut self, chunk: crate::remote::AudioChunk) -> Result<(), RemoteError> {
        if chunk.request_id != self.request_id || chunk.sequence != self.next_sequence {
            return Err(remote_error(
                Some(chunk.request_id),
                "INVALID_AUDIO_SEQUENCE",
                "音频块序号无效",
            ));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(chunk.pcm_base64)
            .map_err(|_| {
                remote_error(
                    Some(chunk.request_id.clone()),
                    "INVALID_AUDIO_CHUNK",
                    "音频块不是有效 Base64",
                )
            })?;
        if decoded.is_empty()
            || decoded.len() > MAX_AUDIO_CHUNK_BYTES
            || decoded.len() % (usize::from(self.channels) * 2) != 0
        {
            return Err(remote_error(
                Some(chunk.request_id),
                "INVALID_AUDIO_CHUNK",
                "音频块尺寸无效",
            ));
        }
        let max_bytes = self.sample_rate_hz as usize
            * usize::from(self.channels)
            * 2
            * MAX_RECORDING_SECONDS as usize;
        if self.pcm.len().saturating_add(decoded.len()) > max_bytes {
            return Err(remote_error(
                Some(chunk.request_id),
                "AUDIO_TOO_LARGE",
                "音频超过 60 秒上限",
            ));
        }
        self.pcm.extend_from_slice(&decoded);
        self.next_sequence += 1;
        Ok(())
    }

    fn finish(self, end: &AudioEnd) -> Result<(), RemoteError> {
        let bytes_per_frame = u64::from(self.channels) * 2;
        let digest = format!("{:x}", Sha256::digest(&self.pcm));
        if end.request_id != self.request_id || end.chunk_count != self.next_sequence {
            return Err(remote_error(
                Some(end.request_id.clone()),
                "INVALID_AUDIO_SEQUENCE",
                "音频结束元数据与块序号不匹配",
            ));
        }
        if end.total_frames != self.pcm.len() as u64 / bytes_per_frame {
            return Err(remote_error(
                Some(end.request_id.clone()),
                "INVALID_AUDIO_LENGTH",
                "音频帧数不匹配",
            ));
        }
        if end.sha256 != digest {
            return Err(remote_error(
                Some(end.request_id.clone()),
                "AUDIO_HASH_MISMATCH",
                "音频完整性校验失败",
            ));
        }
        Ok(())
    }
}

async fn send_error(
    socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    secure: &mut SecureChannel,
    remote: RemoteError,
) -> Result<(), String> {
    send_secure(socket, secure, &RemoteMessage::Error(remote)).await
}

async fn send_secure(
    socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    secure: &mut SecureChannel,
    message: &RemoteMessage,
) -> Result<(), String> {
    for frame in secure.encrypt_message(message)? {
        socket
            .send(Message::Binary(frame))
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn remote_error(
    request_id: Option<String>,
    code: impl Into<String>,
    message: impl Into<String>,
) -> RemoteError {
    RemoteError {
        request_id,
        code: code.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{AudioChunk, AudioEnd};

    fn assembler() -> AudioAssembler {
        AudioAssembler::new(AudioStart {
            request_id: "request".to_string(),
            format: AudioFormat::PcmS16Le,
            sample_rate_hz: 48_000,
            channels: 1,
        })
        .unwrap()
    }

    #[test]
    fn rejects_missing_duplicate_and_out_of_order_chunks() {
        let mut value = assembler();
        let chunk = |sequence| AudioChunk {
            request_id: "request".to_string(),
            sequence,
            pcm_base64: base64::engine::general_purpose::STANDARD.encode([0_u8; 4]),
        };
        assert!(value.push(chunk(1)).is_err());
        assert!(value.push(chunk(0)).is_ok());
        assert!(value.push(chunk(0)).is_err());
    }

    #[test]
    fn rejects_bad_length() {
        let mut value = assembler();
        value
            .push(AudioChunk {
                request_id: "request".to_string(),
                sequence: 0,
                pcm_base64: base64::engine::general_purpose::STANDARD.encode([0_u8; 4]),
            })
            .unwrap();
        assert!(value
            .finish(&AudioEnd {
                request_id: "request".to_string(),
                chunk_count: 1,
                total_frames: 3,
                sha256: "bad".to_string(),
            })
            .is_err());
    }

    #[test]
    fn rejects_bad_hash() {
        let mut value = assembler();
        value
            .push(AudioChunk {
                request_id: "request".to_string(),
                sequence: 0,
                pcm_base64: base64::engine::general_purpose::STANDARD.encode([0_u8; 4]),
            })
            .unwrap();
        let error = value
            .finish(&AudioEnd {
                request_id: "request".to_string(),
                chunk_count: 1,
                total_frames: 2,
                sha256: "bad".to_string(),
            })
            .unwrap_err();
        assert_eq!(error.code, crate::remote::error_code::AUDIO_HASH_MISMATCH);
    }
}
