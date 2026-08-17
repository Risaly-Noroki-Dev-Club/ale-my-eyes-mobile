use crate::remote::{
    error_code, new_request_id, AudioChunk, AudioEnd, AudioFormat, AudioStart, CancelRequest,
    ClientHello, CommandPreview, ConfirmExecution, ExecutionStatus, PairingInfo, Ping, Pong,
    RemoteError, RemoteMessage, ServerHello, MAX_AUDIO_CHUNK_BYTES, MAX_RECORDING_SECONDS,
    MAX_SAMPLE_RATE_HZ, MIN_SAMPLE_RATE_HZ, REMOTE_PROTOCOL_VERSION,
};
use crate::remote_crypto::{client_finish_handshake, client_handshake_message, SecureChannel};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(test))]
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(90);
#[cfg(test)]
const PREVIEW_TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(not(test))]
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
const CONFIRM_TIMEOUT: Duration = Duration::from_millis(500);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_UNEXPECTED_RESPONSES: u8 = 3;
const MAX_RETIRED_REQUEST_IDS: usize = 64;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RemoteSessionError {
    pub code: String,
    pub message: String,
}

impl RemoteSessionError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RemoteSessionEvent {
    Disconnected(RemoteSessionError),
    ProtocolWarning(String),
}

#[derive(Clone)]
pub struct RemoteSession {
    commands: mpsc::Sender<SessionCommand>,
    events: watch::Sender<Option<RemoteSessionEvent>>,
    server_name: String,
}

impl RemoteSession {
    pub async fn connect(pairing: PairingInfo) -> Result<Self, RemoteSessionError> {
        let (mut socket, _) = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio_tungstenite::connect_async(pairing.websocket_url()),
        )
        .await
        .map_err(|_| error("CONNECT_TIMEOUT", "连接桌面端超时"))?
        .map_err(|value| error("CONNECT_FAILED", value.to_string()))?;

        let (noise, client_handshake) = client_handshake_message(&pairing.code)
            .map_err(|value| error("NOISE_HANDSHAKE_FAILED", value))?;
        send_raw(&mut socket, client_handshake).await?;
        let server_handshake = receive_binary(&mut socket, CONNECT_TIMEOUT).await?;
        let mut secure = client_finish_handshake(noise, &server_handshake)
            .map_err(|value| error("NOISE_HANDSHAKE_FAILED", value))?;
        let hello = receive_secure(&mut socket, &mut secure, CONNECT_TIMEOUT).await?;
        let ServerHello {
            protocol_version,
            device_name,
            session_id,
        } = match hello {
            RemoteMessage::ServerHello(hello) => hello,
            _ => return Err(error("INVALID_SERVER_HELLO", "桌面端握手响应无效")),
        };
        if protocol_version != REMOTE_PROTOCOL_VERSION {
            return Err(error(
                error_code::PROTOCOL_INCOMPATIBLE,
                format!(
                    "桌面端协议版本为 {protocol_version}，Android 客户端需要 v{REMOTE_PROTOCOL_VERSION}"
                ),
            ));
        }
        if session_id != pairing.session_id {
            return Err(error(
                "PAIRING_SESSION_MISMATCH",
                "二维码与当前桌面会话不匹配",
            ));
        }
        send_secure(
            &mut socket,
            &mut secure,
            &RemoteMessage::ClientHello(ClientHello {
                protocol_version: REMOTE_PROTOCOL_VERSION,
                device_name: "Android".to_string(),
            }),
        )
        .await?;

        let (commands, receiver) = mpsc::channel(64);
        let (events, _) = watch::channel(None);
        tokio::spawn(run_session(socket, secure, receiver, events.clone()));
        Ok(Self {
            commands,
            events,
            server_name: device_name,
        })
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<RemoteSessionEvent>> {
        self.events.subscribe()
    }

    pub async fn begin_audio(
        &self,
        sample_rate_hz: u32,
        channels: u16,
    ) -> Result<String, RemoteSessionError> {
        let (reply, response) = oneshot::channel();
        self.send_command(SessionCommand::BeginAudio {
            sample_rate_hz,
            channels,
            reply,
        })
        .await?;
        response.await.map_err(|_| closed_error())?
    }

    pub async fn send_audio_chunk(
        &self,
        request_id: String,
        pcm: Vec<u8>,
    ) -> Result<(), RemoteSessionError> {
        let (reply, response) = oneshot::channel();
        self.send_command(SessionCommand::AudioChunk {
            request_id,
            pcm,
            reply,
        })
        .await?;
        response.await.map_err(|_| closed_error())?
    }

    pub async fn finish_audio(
        &self,
        request_id: String,
    ) -> Result<CommandPreview, RemoteSessionError> {
        let (reply, response) = oneshot::channel();
        self.send_command(SessionCommand::FinishAudio { request_id, reply })
            .await?;
        response.await.map_err(|_| closed_error())?
    }

    pub async fn confirm(
        &self,
        request_id: String,
        approved: bool,
    ) -> Result<ExecutionStatus, RemoteSessionError> {
        let (reply, response) = oneshot::channel();
        self.send_command(SessionCommand::Confirm {
            request_id,
            approved,
            reply,
        })
        .await?;
        response.await.map_err(|_| closed_error())?
    }

    pub async fn cancel(&self, request_id: String) -> Result<(), RemoteSessionError> {
        let (reply, response) = oneshot::channel();
        self.send_command(SessionCommand::Cancel { request_id, reply })
            .await?;
        response.await.map_err(|_| closed_error())?
    }

    pub async fn shutdown(&self) {
        let _ = self.commands.send(SessionCommand::Shutdown).await;
    }

    async fn send_command(&self, command: SessionCommand) -> Result<(), RemoteSessionError> {
        tokio::time::timeout(SEND_TIMEOUT, self.commands.send(command))
            .await
            .map_err(|_| error("SEND_TIMEOUT", "远程会话发送队列超时"))?
            .map_err(|_| closed_error())
    }
}

enum SessionCommand {
    BeginAudio {
        sample_rate_hz: u32,
        channels: u16,
        reply: oneshot::Sender<Result<String, RemoteSessionError>>,
    },
    AudioChunk {
        request_id: String,
        pcm: Vec<u8>,
        reply: oneshot::Sender<Result<(), RemoteSessionError>>,
    },
    FinishAudio {
        request_id: String,
        reply: oneshot::Sender<Result<CommandPreview, RemoteSessionError>>,
    },
    Confirm {
        request_id: String,
        approved: bool,
        reply: oneshot::Sender<Result<ExecutionStatus, RemoteSessionError>>,
    },
    Cancel {
        request_id: String,
        reply: oneshot::Sender<Result<(), RemoteSessionError>>,
    },
    Shutdown,
}

struct AudioUpload {
    request_id: String,
    sample_rate_hz: u32,
    channels: u16,
    next_sequence: u32,
    total_pcm_bytes: u64,
    hasher: Sha256,
}

struct PendingPreview {
    request_id: String,
    deadline: Instant,
    reply: oneshot::Sender<Result<CommandPreview, RemoteSessionError>>,
}

struct PendingExecution {
    request_id: String,
    deadline: Instant,
    reply: oneshot::Sender<Result<ExecutionStatus, RemoteSessionError>>,
}

async fn run_session(
    mut socket: Socket,
    mut secure: SecureChannel,
    mut commands: mpsc::Receiver<SessionCommand>,
    events: watch::Sender<Option<RemoteSessionEvent>>,
) {
    let mut active_audio: Option<AudioUpload> = None;
    let mut pending_preview: Option<PendingPreview> = None;
    let mut pending_execution: Option<PendingExecution> = None;
    let mut retired_request_ids = VecDeque::new();
    let mut unexpected_responses = 0_u8;
    let mut last_seen = Instant::now();
    let mut heartbeat_nonce = 0_u64;
    let mut maintenance = tokio::time::interval(Duration::from_millis(250));
    maintenance.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let final_error = loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    break error("SESSION_CLOSED", "远程会话已关闭");
                };
                if let Some(value) = handle_command(
                    command,
                    &mut socket,
                    &mut secure,
                    &mut active_audio,
                    &mut pending_preview,
                    &mut pending_execution,
                ).await {
                    break value;
                }
            }
            frame = socket.next() => {
                let message = match frame {
                    Some(Ok(frame)) if frame.is_binary() => {
                        last_seen = Instant::now();
                        match secure.decrypt_frame(&frame.into_data()) {
                            Ok(Some(message)) => message,
                            Ok(None) => continue,
                            Err(value) => break error("DECRYPT_FAILED", value),
                        }
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(value)) => {
                        break error(error_code::CONNECTION_INTERRUPTED, value.to_string())
                    }
                    None => break closed_error(),
                };
                match handle_remote_message(
                    message,
                    &mut socket,
                    &mut secure,
                    &mut pending_preview,
                    &mut pending_execution,
                    &retired_request_ids,
                    &mut unexpected_responses,
                ).await {
                    Ok(()) => {}
                    Err(value) => break value,
                }
            }
            _ = maintenance.tick() => {
                let now = Instant::now();
                if pending_preview.as_ref().is_some_and(|pending| pending.deadline <= now) {
                    if let Some(pending) = pending_preview.take() {
                        retire_request_id(&mut retired_request_ids, pending.request_id.clone());
                        let _ = pending.reply.send(Err(error(error_code::REQUEST_TIMEOUT, "桌面端处理语音超时")));
                    }
                }
                if pending_execution.as_ref().is_some_and(|pending| pending.deadline <= now) {
                    if let Some(pending) = pending_execution.take() {
                        retire_request_id(&mut retired_request_ids, pending.request_id.clone());
                        let _ = pending.reply.send(Err(error(
                            error_code::CONFIRM_TIMEOUT,
                            "桌面端执行超时，执行结果未知；为避免重复操作，请先检查桌面端",
                        )));
                    }
                }
                if now.duration_since(last_seen) >= HEARTBEAT_TIMEOUT {
                    break error("HEARTBEAT_TIMEOUT", "桌面端心跳超时");
                }
            }
            _ = heartbeat.tick() => {
                heartbeat_nonce = heartbeat_nonce.wrapping_add(1);
                if let Err(value) = send_secure(
                    &mut socket,
                    &mut secure,
                    &RemoteMessage::Ping(Ping { nonce: heartbeat_nonce }),
                ).await {
                    break value;
                }
            }
        }
    };

    let _ = socket.close(None).await;
    if let Some(pending) = pending_preview.take() {
        let _ = pending.reply.send(Err(final_error.clone()));
    }
    if let Some(pending) = pending_execution.take() {
        let _ = pending.reply.send(Err(final_error.clone()));
    }
    events.send_replace(Some(RemoteSessionEvent::Disconnected(final_error)));
}

async fn handle_command(
    command: SessionCommand,
    socket: &mut Socket,
    secure: &mut SecureChannel,
    active_audio: &mut Option<AudioUpload>,
    pending_preview: &mut Option<PendingPreview>,
    pending_execution: &mut Option<PendingExecution>,
) -> Option<RemoteSessionError> {
    match command {
        SessionCommand::BeginAudio {
            sample_rate_hz,
            channels,
            reply,
        } => {
            let result = begin_audio(socket, secure, active_audio, sample_rate_hz, channels).await;
            let terminal = result
                .as_ref()
                .err()
                .filter(|value| is_transport_error(value))
                .cloned();
            let _ = reply.send(result);
            terminal
        }
        SessionCommand::AudioChunk {
            request_id,
            pcm,
            reply,
        } => {
            let result = send_audio(socket, secure, active_audio, &request_id, pcm).await;
            let terminal = result
                .as_ref()
                .err()
                .filter(|value| is_transport_error(value))
                .cloned();
            let _ = reply.send(result);
            terminal
        }
        SessionCommand::FinishAudio { request_id, reply } => {
            if pending_preview.is_some() {
                let _ = reply.send(Err(error("REQUEST_BUSY", "已有语音正在等待处理")));
                return None;
            }
            match finish_audio(socket, secure, active_audio, &request_id).await {
                Ok(()) => {
                    *pending_preview = Some(PendingPreview {
                        request_id,
                        deadline: Instant::now() + PREVIEW_TIMEOUT,
                        reply,
                    });
                    None
                }
                Err(value) => {
                    let terminal = is_transport_error(&value).then(|| value.clone());
                    let _ = reply.send(Err(value));
                    terminal
                }
            }
        }
        SessionCommand::Confirm {
            request_id,
            approved,
            reply,
        } => {
            if pending_execution.is_some() {
                let _ = reply.send(Err(error("REQUEST_BUSY", "已有确认正在等待处理")));
                return None;
            }
            let message = RemoteMessage::ConfirmExecution(ConfirmExecution {
                request_id: request_id.clone(),
                approved,
            });
            match send_secure(socket, secure, &message).await {
                Ok(()) => {
                    *pending_execution = Some(PendingExecution {
                        request_id,
                        deadline: Instant::now() + CONFIRM_TIMEOUT,
                        reply,
                    });
                    None
                }
                Err(value) => {
                    let _ = reply.send(Err(value.clone()));
                    Some(value)
                }
            }
        }
        SessionCommand::Cancel { request_id, reply } => {
            let message = RemoteMessage::CancelRequest(CancelRequest {
                request_id: request_id.clone(),
            });
            let result = send_secure(socket, secure, &message).await;
            if active_audio
                .as_ref()
                .is_some_and(|upload| upload.request_id == request_id)
            {
                active_audio.take();
            }
            if pending_preview
                .as_ref()
                .is_some_and(|pending| pending.request_id == request_id)
            {
                if let Some(pending) = pending_preview.take() {
                    let _ = pending
                        .reply
                        .send(Err(error(error_code::CANCELLED, "请求已取消")));
                }
            }
            if pending_execution
                .as_ref()
                .is_some_and(|pending| pending.request_id == request_id)
            {
                if let Some(pending) = pending_execution.take() {
                    let _ = pending
                        .reply
                        .send(Err(error(error_code::CANCELLED, "请求已取消")));
                }
            }
            let terminal = result.as_ref().err().cloned();
            let _ = reply.send(result);
            terminal
        }
        SessionCommand::Shutdown => Some(error("USER_CLOSED", "用户已断开远程会话")),
    }
}

async fn begin_audio(
    socket: &mut Socket,
    secure: &mut SecureChannel,
    active_audio: &mut Option<AudioUpload>,
    sample_rate_hz: u32,
    channels: u16,
) -> Result<String, RemoteSessionError> {
    if active_audio.is_some() {
        return Err(error("AUDIO_BUSY", "录音已经开始"));
    }
    if !(MIN_SAMPLE_RATE_HZ..=MAX_SAMPLE_RATE_HZ).contains(&sample_rate_hz) || channels != 1 {
        return Err(error(
            "UNSUPPORTED_AUDIO_FORMAT",
            "仅支持 8–96 kHz 单声道 PCM16",
        ));
    }
    let request_id = new_request_id();
    send_secure(
        socket,
        secure,
        &RemoteMessage::AudioStart(AudioStart {
            request_id: request_id.clone(),
            format: AudioFormat::PcmS16Le,
            sample_rate_hz,
            channels,
        }),
    )
    .await?;
    *active_audio = Some(AudioUpload {
        request_id: request_id.clone(),
        sample_rate_hz,
        channels,
        next_sequence: 0,
        total_pcm_bytes: 0,
        hasher: Sha256::new(),
    });
    Ok(request_id)
}

async fn send_audio(
    socket: &mut Socket,
    secure: &mut SecureChannel,
    active_audio: &mut Option<AudioUpload>,
    request_id: &str,
    pcm: Vec<u8>,
) -> Result<(), RemoteSessionError> {
    if pcm.is_empty() {
        return Ok(());
    }
    let upload = active_audio
        .as_mut()
        .filter(|upload| upload.request_id == request_id)
        .ok_or_else(|| error("UNKNOWN_AUDIO_REQUEST", "录音请求不存在"))?;
    if pcm.len() > MAX_AUDIO_CHUNK_BYTES
        || !pcm.len().is_multiple_of(usize::from(upload.channels) * 2)
    {
        return Err(error("INVALID_AUDIO_CHUNK", "PCM 音频块尺寸无效"));
    }
    let max_bytes =
        u64::from(upload.sample_rate_hz) * u64::from(upload.channels) * 2 * MAX_RECORDING_SECONDS;
    if upload.total_pcm_bytes + pcm.len() as u64 > max_bytes {
        return Err(error(error_code::AUDIO_TOO_LARGE, "录音超过 60 秒上限"));
    }
    let message = RemoteMessage::AudioChunk(AudioChunk {
        request_id: request_id.to_string(),
        sequence: upload.next_sequence,
        pcm_base64: base64::engine::general_purpose::STANDARD.encode(&pcm),
    });
    send_secure(socket, secure, &message).await?;
    upload.hasher.update(&pcm);
    upload.total_pcm_bytes += pcm.len() as u64;
    upload.next_sequence = upload
        .next_sequence
        .checked_add(1)
        .ok_or_else(|| error(error_code::AUDIO_TOO_LARGE, "音频块数量超限"))?;
    Ok(())
}

async fn finish_audio(
    socket: &mut Socket,
    secure: &mut SecureChannel,
    active_audio: &mut Option<AudioUpload>,
    request_id: &str,
) -> Result<(), RemoteSessionError> {
    if active_audio
        .as_ref()
        .is_none_or(|upload| upload.request_id != request_id)
    {
        return Err(error("UNKNOWN_AUDIO_REQUEST", "录音请求不存在"));
    }
    let upload = active_audio.take().expect("active upload checked");
    if upload.total_pcm_bytes == 0 {
        return Err(error("EMPTY_AUDIO", "没有录到音频"));
    }
    let bytes_per_frame = u64::from(upload.channels) * 2;
    let message = RemoteMessage::AudioEnd(AudioEnd {
        request_id: upload.request_id,
        chunk_count: upload.next_sequence,
        total_frames: upload.total_pcm_bytes / bytes_per_frame,
        sha256: format!("{:x}", upload.hasher.finalize()),
    });
    send_secure(socket, secure, &message).await
}

async fn handle_remote_message(
    message: RemoteMessage,
    socket: &mut Socket,
    secure: &mut SecureChannel,
    pending_preview: &mut Option<PendingPreview>,
    pending_execution: &mut Option<PendingExecution>,
    retired_request_ids: &VecDeque<String>,
    unexpected_responses: &mut u8,
) -> Result<(), RemoteSessionError> {
    let matched = match message {
        RemoteMessage::CommandPreview(preview) => {
            if retired_request_ids.contains(&preview.request_id) {
                tracing::warn!(request_id = %preview.request_id, "Ignored late command preview");
                return Ok(());
            }
            if pending_preview
                .as_ref()
                .is_some_and(|pending| pending.request_id == preview.request_id)
            {
                let pending = pending_preview.take().expect("pending preview checked");
                let _ = pending.reply.send(Ok(preview));
                true
            } else {
                false
            }
        }
        RemoteMessage::ExecutionStatus(status) => {
            if retired_request_ids.contains(&status.request_id) {
                tracing::warn!(request_id = %status.request_id, "Ignored late execution status");
                return Ok(());
            }
            if pending_execution
                .as_ref()
                .is_some_and(|pending| pending.request_id == status.request_id)
            {
                let pending = pending_execution.take().expect("pending execution checked");
                let _ = pending.reply.send(Ok(status));
                true
            } else {
                false
            }
        }
        RemoteMessage::Error(remote)
            if remote
                .request_id
                .as_ref()
                .is_some_and(|request_id| retired_request_ids.contains(request_id)) =>
        {
            tracing::warn!(request_id = ?remote.request_id, code = %remote.code, "Ignored late remote error");
            return Ok(());
        }
        RemoteMessage::Error(remote) => {
            route_remote_error(remote, pending_preview, pending_execution)
        }
        RemoteMessage::Ping(Ping { nonce }) => {
            send_secure(socket, secure, &RemoteMessage::Pong(Pong { nonce })).await?;
            true
        }
        RemoteMessage::Pong(_) => true,
        _ => false,
    };
    if matched {
        return Ok(());
    }
    *unexpected_responses = unexpected_responses.saturating_add(1);
    tracing::warn!(
        count = *unexpected_responses,
        "Ignored remote message with an unknown, stale, or duplicate request ID"
    );
    if *unexpected_responses >= MAX_UNEXPECTED_RESPONSES {
        return Err(error(
            error_code::PROTOCOL_VIOLATION,
            "桌面端连续返回了无法关联的消息",
        ));
    }
    Ok(())
}

fn retire_request_id(retired: &mut VecDeque<String>, request_id: String) {
    if retired.contains(&request_id) {
        return;
    }
    if retired.len() == MAX_RETIRED_REQUEST_IDS {
        retired.pop_front();
    }
    retired.push_back(request_id);
}

fn route_remote_error(
    remote: RemoteError,
    pending_preview: &mut Option<PendingPreview>,
    pending_execution: &mut Option<PendingExecution>,
) -> bool {
    let Some(request_id) = remote.request_id else {
        return false;
    };
    let value = error(remote.code, remote.message);
    if pending_preview
        .as_ref()
        .is_some_and(|pending| pending.request_id == request_id)
    {
        if let Some(pending) = pending_preview.take() {
            let _ = pending.reply.send(Err(value));
        }
        return true;
    }
    if pending_execution
        .as_ref()
        .is_some_and(|pending| pending.request_id == request_id)
    {
        if let Some(pending) = pending_execution.take() {
            let _ = pending.reply.send(Err(value));
        }
        return true;
    }
    false
}

async fn send_raw(socket: &mut Socket, frame: Vec<u8>) -> Result<(), RemoteSessionError> {
    tokio::time::timeout(SEND_TIMEOUT, socket.send(Message::Binary(frame)))
        .await
        .map_err(|_| error("SEND_TIMEOUT", "发送远程消息超时"))?
        .map_err(|value| error(error_code::CONNECTION_INTERRUPTED, value.to_string()))
}

async fn send_secure(
    socket: &mut Socket,
    secure: &mut SecureChannel,
    message: &RemoteMessage,
) -> Result<(), RemoteSessionError> {
    let frames = secure
        .encrypt_message(message)
        .map_err(|value| error("ENCRYPT_FAILED", value))?;
    for frame in frames {
        send_raw(socket, frame).await?;
    }
    Ok(())
}

async fn receive_binary(
    socket: &mut Socket,
    timeout: Duration,
) -> Result<Vec<u8>, RemoteSessionError> {
    tokio::time::timeout(timeout, async {
        loop {
            match socket.next().await {
                Some(Ok(frame)) if frame.is_binary() => return Ok(frame.into_data()),
                Some(Ok(_)) => continue,
                Some(Err(value)) => {
                    return Err(error(error_code::CONNECTION_INTERRUPTED, value.to_string()));
                }
                None => return Err(closed_error()),
            }
        }
    })
    .await
    .map_err(|_| error("HANDSHAKE_TIMEOUT", "桌面端握手超时"))?
}

async fn receive_secure(
    socket: &mut Socket,
    secure: &mut SecureChannel,
    timeout: Duration,
) -> Result<RemoteMessage, RemoteSessionError> {
    tokio::time::timeout(timeout, async {
        loop {
            let frame = receive_binary(socket, timeout).await?;
            match secure
                .decrypt_frame(&frame)
                .map_err(|value| error("DECRYPT_FAILED", value))?
            {
                Some(message) => return Ok(message),
                None => continue,
            }
        }
    })
    .await
    .map_err(|_| error("HANDSHAKE_TIMEOUT", "桌面端握手超时"))?
}

fn error(code: impl Into<String>, message: impl Into<String>) -> RemoteSessionError {
    RemoteSessionError::new(code, message)
}

fn closed_error() -> RemoteSessionError {
    error(
        error_code::CONNECTION_INTERRUPTED,
        "桌面端连接已断开，请重新扫码",
    )
}

fn is_transport_error(value: &RemoteSessionError) -> bool {
    matches!(
        value.code.as_str(),
        "CONNECTION_INTERRUPTED" | "SEND_TIMEOUT" | "ENCRYPT_FAILED"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_server::{MockBehavior, MockRemoteServer};

    async fn connect(behavior: MockBehavior) -> (MockRemoteServer, RemoteSession) {
        let server = MockRemoteServer::start(REMOTE_PROTOCOL_VERSION, behavior)
            .await
            .unwrap();
        let session = RemoteSession::connect(server.pairing.clone())
            .await
            .unwrap();
        (server, session)
    }

    async fn upload(
        session: &RemoteSession,
        sample_rate_hz: u32,
        seconds: u64,
    ) -> (String, CommandPreview) {
        let request_id = session.begin_audio(sample_rate_hz, 1).await.unwrap();
        let mut remaining = sample_rate_hz as usize * 2 * seconds as usize;
        while remaining > 0 {
            let len = remaining.min(MAX_AUDIO_CHUNK_BYTES);
            session
                .send_audio_chunk(request_id.clone(), vec![0x2a; len])
                .await
                .unwrap();
            remaining -= len;
        }
        let preview = session.finish_audio(request_id.clone()).await.unwrap();
        (request_id, preview)
    }

    #[tokio::test]
    async fn rejects_v1_without_fallback() {
        let server = MockRemoteServer::start(1, MockBehavior::Normal)
            .await
            .unwrap();
        let error = match RemoteSession::connect(server.pairing.clone()).await {
            Ok(_) => panic!("v1 server must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.code, error_code::PROTOCOL_INCOMPATIBLE);
    }

    #[tokio::test]
    async fn uploads_1_10_and_60_second_audio() {
        let (_server, session) = connect(MockBehavior::Normal).await;
        for seconds in [1, 10, 60] {
            let (request_id, preview) = upload(&session, 48_000, seconds).await;
            assert_eq!(preview.request_id, request_id);
        }
    }

    #[tokio::test]
    async fn accepts_44_1_and_48_khz_metadata() {
        let (_server, session) = connect(MockBehavior::Normal).await;
        for sample_rate_hz in [44_100, 48_000] {
            let (request_id, preview) = upload(&session, sample_rate_hz, 1).await;
            assert_eq!(preview.request_id, request_id);
        }
    }

    #[tokio::test]
    async fn receives_fragmented_long_preview() {
        let (_server, session) = connect(MockBehavior::LongPreview).await;
        let (_, preview) = upload(&session, 48_000, 1).await;
        assert_eq!(preview.response_text.len(), 96 * 1024);
    }

    #[tokio::test]
    async fn rejects_audio_beyond_sixty_seconds() {
        let (_server, session) = connect(MockBehavior::Normal).await;
        let request_id = session.begin_audio(8_000, 1).await.unwrap();
        let mut remaining = 8_000_usize * 2 * MAX_RECORDING_SECONDS as usize;
        while remaining > 0 {
            let len = remaining.min(MAX_AUDIO_CHUNK_BYTES);
            session
                .send_audio_chunk(request_id.clone(), vec![0; len])
                .await
                .unwrap();
            remaining -= len;
        }
        let error = session
            .send_audio_chunk(request_id.clone(), vec![0; 2])
            .await
            .unwrap_err();
        assert_eq!(error.code, error_code::AUDIO_TOO_LARGE);
        session.cancel(request_id).await.unwrap();
    }

    #[tokio::test]
    async fn cancel_clears_active_audio_without_replay() {
        let (_server, session) = connect(MockBehavior::Normal).await;
        let request_id = session.begin_audio(48_000, 1).await.unwrap();
        session
            .send_audio_chunk(request_id.clone(), vec![0; 2_000])
            .await
            .unwrap();
        session.cancel(request_id).await.unwrap();
        let (_, preview) = upload(&session, 48_000, 1).await;
        assert_eq!(preview.response_text, "mock preview");
    }

    #[tokio::test]
    async fn wrong_finish_id_does_not_discard_active_audio() {
        let (_server, session) = connect(MockBehavior::Normal).await;
        let request_id = session.begin_audio(48_000, 1).await.unwrap();
        session
            .send_audio_chunk(request_id.clone(), vec![0; 2_000])
            .await
            .unwrap();
        let error = session.finish_audio("wrong".to_string()).await.unwrap_err();
        assert_eq!(error.code, "UNKNOWN_AUDIO_REQUEST");
        let preview = session.finish_audio(request_id.clone()).await.unwrap();
        assert_eq!(preview.request_id, request_id);
    }

    #[tokio::test]
    async fn preview_and_confirmation_have_deadlines() {
        let (_server, session) = connect(MockBehavior::SilentPreview).await;
        let request_id = session.begin_audio(48_000, 1).await.unwrap();
        session
            .send_audio_chunk(request_id.clone(), vec![0; 2_000])
            .await
            .unwrap();
        let error = session.finish_audio(request_id).await.unwrap_err();
        assert_eq!(error.code, error_code::REQUEST_TIMEOUT);

        let (_server, session) = connect(MockBehavior::SilentConfirmation).await;
        let (request_id, _) = upload(&session, 48_000, 1).await;
        let error = session.confirm(request_id, true).await.unwrap_err();
        assert_eq!(error.code, error_code::CONFIRM_TIMEOUT);
    }

    #[tokio::test]
    async fn late_execution_statuses_do_not_count_as_protocol_violations() {
        let (_server, session) = connect(MockBehavior::LateConfirmation).await;
        for _ in 0..3 {
            let (request_id, _) = upload(&session, 48_000, 1).await;
            let error = session.confirm(request_id, true).await.unwrap_err();
            assert_eq!(error.code, error_code::CONFIRM_TIMEOUT);
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        let request_id = session.begin_audio(48_000, 1).await.unwrap();
        session.cancel(request_id).await.unwrap();
    }

    #[tokio::test]
    async fn confirms_and_rejects_over_the_same_session() {
        let (_server, session) = connect(MockBehavior::Normal).await;
        let (request_id, _) = upload(&session, 48_000, 1).await;
        let approved = session.confirm(request_id, true).await.unwrap();
        assert_eq!(approved.state, crate::remote::ExecutionState::Completed);

        let (request_id, _) = upload(&session, 48_000, 1).await;
        let rejected = session.confirm(request_id, false).await.unwrap();
        assert_eq!(rejected.state, crate::remote::ExecutionState::Cancelled);
    }

    #[tokio::test]
    async fn three_wrong_request_ids_disconnect_the_session() {
        let (_server, session) = connect(MockBehavior::WrongPreviewIds).await;
        let mut events = session.subscribe();
        let request_id = session.begin_audio(48_000, 1).await.unwrap();
        session
            .send_audio_chunk(request_id.clone(), vec![0; 2_000])
            .await
            .unwrap();
        let error = session.finish_audio(request_id).await.unwrap_err();
        assert_eq!(error.code, error_code::PROTOCOL_VIOLATION);
        events.changed().await.unwrap();
        let event = events.borrow_and_update().clone().unwrap();
        assert!(matches!(
            event,
            RemoteSessionEvent::Disconnected(RemoteSessionError { code, .. })
                if code == error_code::PROTOCOL_VIOLATION
        ));
    }

    #[tokio::test]
    async fn connection_interruption_fails_pending_request() {
        let (server, session) = connect(MockBehavior::SilentPreview).await;
        let request_id = session.begin_audio(48_000, 1).await.unwrap();
        session
            .send_audio_chunk(request_id.clone(), vec![0; 2_000])
            .await
            .unwrap();
        let pending = tokio::spawn({
            let session = session.clone();
            async move { session.finish_audio(request_id).await }
        });
        tokio::task::yield_now().await;
        drop(server);
        let error = pending.await.unwrap().unwrap_err();
        assert_eq!(error.code, error_code::CONNECTION_INTERRUPTED);
    }

    #[tokio::test]
    async fn late_subscriber_observes_disconnect() {
        let (server, session) = connect(MockBehavior::Normal).await;
        drop(server);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut events = session.subscribe();
        if events.borrow().is_none() {
            tokio::time::timeout(Duration::from_secs(1), events.changed())
                .await
                .unwrap()
                .unwrap();
        }
        assert!(matches!(
            events.borrow().as_ref(),
            Some(RemoteSessionEvent::Disconnected(_))
        ));
    }
}
