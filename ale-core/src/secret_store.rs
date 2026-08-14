use crate::{AleError, Result};

const SERVICE: &str = "com.alemyeyes.cloud-api";
const ACCOUNT: &str = "default";

/// Stores credentials outside the application configuration file.
pub trait SecretStore: Send + Sync {
    fn get_api_key(&self) -> Result<Option<String>>;
    fn set_api_key(&self, api_key: &str) -> Result<()>;
    fn delete_api_key(&self) -> Result<()>;
}

pub struct SystemSecretStore;

impl SystemSecretStore {
    fn entry(&self) -> Result<keyring::Entry> {
        initialize_platform_store()?;
        keyring::Entry::new(SERVICE, ACCOUNT)
            .map_err(|error| AleError::ConfigError(format!("无法初始化系统凭据库: {error}")))
    }
}

#[cfg(target_os = "ios")]
fn initialize_platform_store() -> Result<()> {
    static INITIALIZED: std::sync::OnceLock<std::result::Result<(), String>> =
        std::sync::OnceLock::new();

    match INITIALIZED.get_or_init(|| {
        let store = apple_native_keyring_store::protected::Store::new()
            .map_err(|error| error.to_string())?;
        keyring_core::set_default_store(store);
        Ok(())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(AleError::ConfigError(format!(
            "无法初始化 iOS Keychain: {error}"
        ))),
    }
}

#[cfg(target_os = "android")]
fn initialize_platform_store() -> Result<()> {
    static INITIALIZED: std::sync::OnceLock<std::result::Result<(), String>> =
        std::sync::OnceLock::new();

    match INITIALIZED.get_or_init(|| {
        let store =
            android_native_keyring_store::Store::new().map_err(|error| error.to_string())?;
        keyring_core::set_default_store(store);
        Ok(())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(AleError::ConfigError(format!(
            "无法初始化 Android Keystore: {error}"
        ))),
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn initialize_platform_store() -> Result<()> {
    Ok(())
}

impl SecretStore for SystemSecretStore {
    fn get_api_key(&self) -> Result<Option<String>> {
        match self.entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(AleError::ConfigError(format!(
                "无法读取系统凭据库: {error}"
            ))),
        }
    }

    fn set_api_key(&self, api_key: &str) -> Result<()> {
        self.entry()?.set_password(api_key).map_err(|error| {
            AleError::ConfigError(format!("无法保存 API Key 到系统凭据库: {error}"))
        })
    }

    fn delete_api_key(&self) -> Result<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(AleError::ConfigError(format!(
                "无法删除系统凭据库中的 API Key: {error}"
            ))),
        }
    }
}
