#[cfg(target_os = "ios")]
use ale_core::actions::ActionPlan;
#[cfg(target_os = "ios")]
use ale_core::Result;

/// 统一的自动化执行结果
#[cfg(target_os = "ios")]
pub struct ExecutionResult {
    pub actions_executed: usize,
}

#[cfg(target_os = "ios")]
#[derive(Debug, Clone, Copy)]
pub struct PlatformCapabilities {
    pub image_capture: bool,
    pub automation: bool,
    pub local_microphone: bool,
}

/// 移动端平台抽象。
#[cfg(target_os = "ios")]
pub trait PlatformService: Send + Sync {
    /// 捕获当前设备画面，返回 JPEG 字节。
    #[cfg(target_os = "ios")]
    fn capture_image(&self) -> Option<Vec<u8>>;

    /// 执行自动化操作计划
    #[cfg(target_os = "ios")]
    fn execute_plan(&self, plan: &ActionPlan, approved: bool) -> Result<ExecutionResult>;

    /// 自动化引擎是否就绪
    #[cfg(target_os = "ios")]
    fn is_automation_ready(&self) -> bool;

    fn capabilities(&self) -> PlatformCapabilities;
}

/// 为当前编译目标创建平台服务实例
#[cfg(target_os = "ios")]
pub fn create_platform() -> Box<dyn PlatformService> {
    Box::new(ios::IosPlatform::new())
}

#[cfg(target_os = "ios")]
mod ios;
