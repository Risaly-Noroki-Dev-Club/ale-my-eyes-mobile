#![cfg(target_os = "android")]

use crate::remote_crypto;
use ale_core::remote::{
    ClientHello, CommandInput, CommandPreview, CommandRequest, ConfirmExecution, ExecutionStatus,
    PairingInfo, RemoteError, RemoteMessage, REMOTE_PROTOCOL_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub struct RemoteClient {
    url: String,
    code: String,
    session_id: String,
}

impl RemoteClient {
    pub fn from_pairing(pairing: &PairingInfo) -> Self {
        Self {
            url: pairing.websocket_url(),
            code: pairing.code.clone(),
            session_id: pairing.session_id.clone(),
        }
    }

    pub async fn test(&self) -> Result<String, String> {
        let (_, server_name) = self.connect().await?;
        Ok(server_name)
    }

    pub async fn send_command(&self, input: CommandInput) -> Result<CommandPreview, String> {
        let request_id = ale_core::remote::new_request_id();
        let (mut socket, mut secure) = self.connect().await?.0;
        let message = RemoteMessage::CommandRequest(CommandRequest {
            request_id: request_id.clone(),
            input,
        });
        send_secure(&mut socket, &mut secure, &message).await?;

        loop {
            match read_secure(&mut socket, &mut secure).await? {
                RemoteMessage::CommandPreview(preview) => return Ok(preview),
                RemoteMessage::Error(RemoteError { message, .. }) => return Err(message),
                _ => {}
            }
        }
    }

    pub async fn confirm(
        &self,
        request_id: String,
        approved: bool,
    ) -> Result<ExecutionStatus, String> {
        let (mut socket, mut secure) = self.connect().await?.0;
        send_secure(
            &mut socket,
            &mut secure,
            &RemoteMessage::ConfirmExecution(ConfirmExecution {
                request_id,
                approved,
            }),
        )
        .await?;

        loop {
            match read_secure(&mut socket, &mut secure).await? {
                RemoteMessage::ExecutionStatus(status) => return Ok(status),
                RemoteMessage::Error(RemoteError { message, .. }) => return Err(message),
                _ => {}
            }
        }
    }

    async fn connect(
        &self,
    ) -> Result<
        (
            (
                tokio_tungstenite::WebSocketStream<
                    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
                >,
                remote_crypto::SecureChannel,
            ),
            String,
        ),
        String,
    > {
        let (mut socket, _) =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(&self.url))
                .await
                .map_err(|_| "连接桌面端超时".to_string())?
                .map_err(|error| error.to_string())?;
        let (noise, client_handshake) = remote_crypto::client_handshake_message(&self.code)?;
        socket
            .send(Message::Binary(client_handshake))
            .await
            .map_err(|error| error.to_string())?;
        let server_handshake = tokio::time::timeout(CONNECT_TIMEOUT, socket.next())
            .await
            .map_err(|_| "桌面端握手超时".to_string())?
            .ok_or_else(|| "missing server handshake".to_string())?
            .map_err(|error| error.to_string())?
            .into_data();
        let mut secure = remote_crypto::client_finish_handshake(noise, &server_handshake)?;

        let hello = tokio::time::timeout(CONNECT_TIMEOUT, read_secure(&mut socket, &mut secure))
            .await
            .map_err(|_| "桌面端握手超时".to_string())??;
        let server_name = match hello {
            RemoteMessage::ServerHello(hello)
                if hello.protocol_version == REMOTE_PROTOCOL_VERSION
                    && hello.session_id == self.session_id =>
            {
                hello.device_name
            }
            RemoteMessage::ServerHello(_) => return Err("二维码与当前桌面会话不匹配".to_string()),
            _ => return Err("桌面端握手响应无效".to_string()),
        };

        send_secure(
            &mut socket,
            &mut secure,
            &RemoteMessage::ClientHello(ClientHello {
                protocol_version: REMOTE_PROTOCOL_VERSION,
                device_name: "Android".to_string(),
            }),
        )
        .await?;

        Ok(((socket, secure), server_name))
    }
}

async fn send_secure<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    secure: &mut remote_crypto::SecureChannel,
    message: &RemoteMessage,
) -> Result<(), String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let frame = secure.encrypt_message(message)?;
    socket
        .send(Message::Binary(frame))
        .await
        .map_err(|error| error.to_string())
}

async fn read_secure<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    secure: &mut remote_crypto::SecureChannel,
) -> Result<RemoteMessage, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let frame = socket
            .next()
            .await
            .ok_or_else(|| "remote closed".to_string())?
            .map_err(|error| error.to_string())?;
        if frame.is_binary() {
            return secure.decrypt_message(&frame.into_data());
        }
    }
}
