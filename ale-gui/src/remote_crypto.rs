#![cfg(target_os = "android")]

use ale_core::remote::RemoteMessage;
use snow::{Builder, TransportState};

const NOISE_PATTERN: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";

pub struct SecureChannel {
    transport: TransportState,
}

impl SecureChannel {
    pub fn encrypt_message(&mut self, message: &RemoteMessage) -> Result<Vec<u8>, String> {
        let payload = serde_json::to_vec(message).map_err(|error| error.to_string())?;
        let mut out = vec![0u8; payload.len() + 1024];
        let len = self
            .transport
            .write_message(&payload, &mut out)
            .map_err(|error| error.to_string())?;
        out.truncate(len);
        Ok(out)
    }

    pub fn decrypt_message(&mut self, frame: &[u8]) -> Result<RemoteMessage, String> {
        let mut out = vec![0u8; frame.len() + 1024];
        let len = self
            .transport
            .read_message(frame, &mut out)
            .map_err(|error| error.to_string())?;
        out.truncate(len);
        serde_json::from_slice(&out).map_err(|error| error.to_string())
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
    let params: snow::params::NoiseParams = NOISE_PATTERN
        .parse()
        .map_err(|error: snow::Error| error.to_string())?;
    Ok(params)
}

pub fn client_handshake_message(code: &str) -> Result<(snow::HandshakeState, Vec<u8>), String> {
    let psk = psk_from_code(code);
    let mut noise = Builder::new(noise_params()?)
        .psk(0, &psk)
        .build_initiator()
        .map_err(|error| error.to_string())?;
    let mut message = vec![0u8; 1024];
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
    let mut scratch = vec![0u8; 1024];
    noise
        .read_message(server_message, &mut scratch)
        .map_err(|error| error.to_string())?;
    let transport = noise
        .into_transport_mode()
        .map_err(|error| error.to_string())?;
    Ok(SecureChannel { transport })
}
