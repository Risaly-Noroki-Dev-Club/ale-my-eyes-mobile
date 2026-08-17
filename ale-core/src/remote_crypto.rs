use crate::remote::RemoteMessage;
use snow::{Builder, TransportState};

const NOISE_PATTERN: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
const CHUNK_MAGIC: &[u8; 4] = b"AME2";
const CHUNK_HEADER_LEN: usize = 20;
pub const MAX_NOISE_PLAINTEXT: usize = 48 * 1024;
pub const MAX_SECURE_MESSAGE_BYTES: usize = 1024 * 1024;

pub struct SecureChannel {
    transport: TransportState,
    incoming: Option<IncomingMessage>,
}

struct IncomingMessage {
    id: u64,
    total_chunks: u32,
    next_chunk: u32,
    payload: Vec<u8>,
}

impl SecureChannel {
    pub fn encrypt_message(&mut self, message: &RemoteMessage) -> Result<Vec<Vec<u8>>, String> {
        let payload = serde_json::to_vec(message).map_err(|error| error.to_string())?;
        if payload.len() > MAX_SECURE_MESSAGE_BYTES {
            return Err("MESSAGE_TOO_LARGE".to_string());
        }
        if payload.len() <= MAX_NOISE_PLAINTEXT {
            return Ok(vec![self.encrypt_payload(&payload)?]);
        }

        let chunk_payload_size = MAX_NOISE_PLAINTEXT - CHUNK_HEADER_LEN;
        let total_chunks = u32::try_from(payload.len().div_ceil(chunk_payload_size))
            .map_err(|_| "MESSAGE_TOO_LARGE".to_string())?;
        let message_id = rand::random::<u64>();
        let mut frames = Vec::with_capacity(total_chunks as usize);
        for (index, chunk) in payload.chunks(chunk_payload_size).enumerate() {
            let mut framed = Vec::with_capacity(CHUNK_HEADER_LEN + chunk.len());
            framed.extend_from_slice(CHUNK_MAGIC);
            framed.extend_from_slice(&message_id.to_be_bytes());
            framed.extend_from_slice(&(index as u32).to_be_bytes());
            framed.extend_from_slice(&total_chunks.to_be_bytes());
            framed.extend_from_slice(chunk);
            frames.push(self.encrypt_payload(&framed)?);
        }
        Ok(frames)
    }

    fn encrypt_payload(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = vec![0_u8; payload.len() + 16];
        let len = self
            .transport
            .write_message(payload, &mut out)
            .map_err(|error| error.to_string())?;
        out.truncate(len);
        Ok(out)
    }

    pub fn decrypt_frame(&mut self, frame: &[u8]) -> Result<Option<RemoteMessage>, String> {
        if frame.len() > MAX_NOISE_PLAINTEXT + 16 {
            return Err("MESSAGE_TOO_LARGE".to_string());
        }
        let mut out = vec![0_u8; frame.len()];
        let len = self
            .transport
            .read_message(frame, &mut out)
            .map_err(|error| error.to_string())?;
        out.truncate(len);

        if !out.starts_with(CHUNK_MAGIC) {
            if self.incoming.take().is_some() {
                return Err("INVALID_CHUNK_SEQUENCE".to_string());
            }
            return serde_json::from_slice(&out)
                .map(Some)
                .map_err(|error| error.to_string());
        }
        if out.len() < CHUNK_HEADER_LEN {
            self.incoming = None;
            return Err("INVALID_CHUNK_HEADER".to_string());
        }

        let id = u64::from_be_bytes(out[4..12].try_into().expect("fixed-size message id"));
        let index = u32::from_be_bytes(out[12..16].try_into().expect("fixed-size chunk index"));
        let total_chunks =
            u32::from_be_bytes(out[16..20].try_into().expect("fixed-size chunk count"));
        if total_chunks == 0 || index >= total_chunks {
            self.incoming = None;
            return Err("INVALID_CHUNK_SEQUENCE".to_string());
        }

        if index == 0 {
            if self.incoming.is_some() {
                self.incoming = None;
                return Err("INVALID_CHUNK_SEQUENCE".to_string());
            }
            self.incoming = Some(IncomingMessage {
                id,
                total_chunks,
                next_chunk: 0,
                payload: Vec::new(),
            });
        }
        let incoming = self
            .incoming
            .as_mut()
            .ok_or_else(|| "MISSING_CHUNK_START".to_string())?;
        if incoming.id != id
            || incoming.total_chunks != total_chunks
            || incoming.next_chunk != index
        {
            self.incoming = None;
            return Err("INVALID_CHUNK_SEQUENCE".to_string());
        }
        if incoming
            .payload
            .len()
            .saturating_add(out.len() - CHUNK_HEADER_LEN)
            > MAX_SECURE_MESSAGE_BYTES
        {
            self.incoming = None;
            return Err("MESSAGE_TOO_LARGE".to_string());
        }
        incoming.payload.extend_from_slice(&out[CHUNK_HEADER_LEN..]);
        incoming.next_chunk += 1;
        if incoming.next_chunk != incoming.total_chunks {
            return Ok(None);
        }

        let payload = self
            .incoming
            .take()
            .ok_or_else(|| "MISSING_CHUNK_START".to_string())?
            .payload;
        serde_json::from_slice(&payload)
            .map(Some)
            .map_err(|error| error.to_string())
    }
}

fn psk_from_code(code: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"ale-my-eyes-remote-v1");
    hasher.update(code.as_bytes());
    hasher.finalize().into()
}

fn noise_params() -> Result<snow::params::NoiseParams, String> {
    NOISE_PATTERN
        .parse()
        .map_err(|error: snow::Error| error.to_string())
}

pub fn client_handshake_message(code: &str) -> Result<(snow::HandshakeState, Vec<u8>), String> {
    let psk = psk_from_code(code);
    let mut noise = Builder::new(noise_params()?)
        .psk(0, &psk)
        .build_initiator()
        .map_err(|error| error.to_string())?;
    let mut message = vec![0_u8; 1024];
    let len = noise
        .write_message(&[], &mut message)
        .map_err(|error| error.to_string())?;
    message.truncate(len);
    Ok((noise, message))
}

pub fn client_finish_handshake(
    mut noise: snow::HandshakeState,
    server_message: &[u8],
) -> Result<SecureChannel, String> {
    let mut scratch = vec![0_u8; 1024];
    noise
        .read_message(server_message, &mut scratch)
        .map_err(|error| error.to_string())?;
    Ok(SecureChannel {
        transport: noise
            .into_transport_mode()
            .map_err(|error| error.to_string())?,
        incoming: None,
    })
}

#[cfg(any(test, feature = "mock-server"))]
pub fn server_handshake_reply(
    code: &str,
    client_message: &[u8],
) -> Result<(SecureChannel, Vec<u8>), String> {
    let psk = psk_from_code(code);
    let mut noise = Builder::new(noise_params()?)
        .psk(0, &psk)
        .build_responder()
        .map_err(|error| error.to_string())?;
    let mut scratch = vec![0_u8; 1024];
    noise
        .read_message(client_message, &mut scratch)
        .map_err(|error| error.to_string())?;
    let mut reply = vec![0_u8; 1024];
    let len = noise
        .write_message(&[], &mut reply)
        .map_err(|error| error.to_string())?;
    reply.truncate(len);
    Ok((
        SecureChannel {
            transport: noise
                .into_transport_mode()
                .map_err(|error| error.to_string())?,
            incoming: None,
        },
        reply,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{Ping, RemoteMessage};

    fn channel_pair() -> (SecureChannel, SecureChannel) {
        let (initiator, first) = client_handshake_message("123456").unwrap();
        let (server, reply) = server_handshake_reply("123456", &first).unwrap();
        let client = client_finish_handshake(initiator, &reply).unwrap();
        (client, server)
    }

    fn encrypted_chunk(
        channel: &mut SecureChannel,
        id: u64,
        index: u32,
        total: u32,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut framed = Vec::with_capacity(CHUNK_HEADER_LEN + payload.len());
        framed.extend_from_slice(CHUNK_MAGIC);
        framed.extend_from_slice(&id.to_be_bytes());
        framed.extend_from_slice(&index.to_be_bytes());
        framed.extend_from_slice(&total.to_be_bytes());
        framed.extend_from_slice(payload);
        channel.encrypt_payload(&framed).unwrap()
    }

    #[test]
    fn noise_roundtrip() {
        let (initiator, first) = client_handshake_message("123456").unwrap();
        let (mut server, reply) = server_handshake_reply("123456", &first).unwrap();
        let mut client = client_finish_handshake(initiator, &reply).unwrap();
        let encrypted = client
            .encrypt_message(&RemoteMessage::Ping(Ping { nonce: 42 }))
            .unwrap()
            .pop()
            .unwrap();
        assert!(matches!(
            server.decrypt_frame(&encrypted).unwrap(),
            Some(RemoteMessage::Ping(Ping { nonce: 42 }))
        ));
    }

    #[test]
    fn fragmented_message_roundtrips() {
        let (initiator, first) = client_handshake_message("123456").unwrap();
        let (mut server, reply) = server_handshake_reply("123456", &first).unwrap();
        let mut client = client_finish_handshake(initiator, &reply).unwrap();
        let message = RemoteMessage::Error(crate::remote::RemoteError {
            request_id: Some("long-preview".to_string()),
            code: "TEST".to_string(),
            message: "x".repeat(96 * 1024),
        });
        let frames = client.encrypt_message(&message).unwrap();
        assert!(frames.len() > 1);
        let mut decoded = None;
        for frame in frames {
            if let Some(message) = server.decrypt_frame(&frame).unwrap() {
                decoded = Some(message);
            }
        }
        let Some(RemoteMessage::Error(remote)) = decoded else {
            panic!("expected reassembled error");
        };
        assert_eq!(remote.message.len(), 96 * 1024);
    }

    #[test]
    fn rejects_message_over_total_limit() {
        let (initiator, first) = client_handshake_message("123456").unwrap();
        let (_server, reply) = server_handshake_reply("123456", &first).unwrap();
        let mut client = client_finish_handshake(initiator, &reply).unwrap();
        let message = RemoteMessage::Error(crate::remote::RemoteError {
            request_id: None,
            code: "TEST".to_string(),
            message: "x".repeat(MAX_SECURE_MESSAGE_BYTES),
        });
        assert_eq!(
            client.encrypt_message(&message).unwrap_err(),
            "MESSAGE_TOO_LARGE"
        );
    }

    #[test]
    fn rejects_missing_duplicate_and_out_of_order_fragments() {
        let (mut sender, mut receiver) = channel_pair();
        let missing = encrypted_chunk(&mut sender, 1, 1, 2, b"tail");
        assert_eq!(
            receiver.decrypt_frame(&missing).unwrap_err(),
            "MISSING_CHUNK_START"
        );

        let first = encrypted_chunk(&mut sender, 2, 0, 2, b"first");
        assert!(receiver.decrypt_frame(&first).unwrap().is_none());
        let duplicate = encrypted_chunk(&mut sender, 2, 0, 2, b"first");
        assert_eq!(
            receiver.decrypt_frame(&duplicate).unwrap_err(),
            "INVALID_CHUNK_SEQUENCE"
        );

        let first = encrypted_chunk(&mut sender, 3, 0, 3, b"first");
        assert!(receiver.decrypt_frame(&first).unwrap().is_none());
        let out_of_order = encrypted_chunk(&mut sender, 3, 2, 3, b"third");
        assert_eq!(
            receiver.decrypt_frame(&out_of_order).unwrap_err(),
            "INVALID_CHUNK_SEQUENCE"
        );
    }

    #[test]
    fn rejects_reassembly_over_total_limit() {
        let (mut sender, mut receiver) = channel_pair();
        let payload = vec![b'x'; MAX_NOISE_PLAINTEXT - CHUNK_HEADER_LEN];
        let total = (MAX_SECURE_MESSAGE_BYTES / payload.len() + 2) as u32;
        let mut error = None;
        for index in 0..total {
            let frame = encrypted_chunk(&mut sender, 4, index, total, &payload);
            match receiver.decrypt_frame(&frame) {
                Ok(None) => {}
                Err(value) => {
                    error = Some(value);
                    break;
                }
                Ok(Some(_)) => panic!("oversized fragments must not decode"),
            }
        }
        assert_eq!(error.as_deref(), Some("MESSAGE_TOO_LARGE"));
    }
}
