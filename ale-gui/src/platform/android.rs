use super::PlatformCapabilities;

/// Android 平台服务：仅作为局域网指令入口，不在本机执行自动化。
pub struct AndroidPlatform;

impl AndroidPlatform {
    pub fn new() -> Self {
        Self
    }
}

impl super::PlatformService for AndroidPlatform {
    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            image_capture: false,
            automation: false,
            local_microphone: true,
        }
    }
}
