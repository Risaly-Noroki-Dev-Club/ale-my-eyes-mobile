use ale_core::actions::{Action, ActionPlan};
use ale_core::{AleError, Result};

/// iOS 自动化引擎配置
#[derive(Debug, Clone)]
pub struct AutomationConfig {
    pub require_confirmation: bool,
    pub action_delay_ms: u64,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            require_confirmation: true,
            action_delay_ms: 100,
        }
    }
}

/// 自动化执行结果
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub success: bool,
    pub actions_executed: usize,
    pub error: Option<String>,
}

/// iOS 自动化引擎 — 目前不支持桌面自动化操作
///
/// iOS 沙箱限制了应用对系统 UI 的控制能力。
/// 可以执行的操作：打开 URL、文本转语音
/// 不支持的操作：鼠标点击、键盘输入、滚动、文件操作
pub struct IosAutomationEngine {
    config: AutomationConfig,
}

impl IosAutomationEngine {
    pub fn new(config: AutomationConfig) -> Result<Self> {
        Ok(Self { config })
    }

    /// 执行操作计划 — iOS 仅支持有限的操作
    pub fn execute_plan(&self, plan: &ActionPlan, approved: bool) -> Result<ExecutionResult> {
        plan.validate()?;
        if plan.requires_confirmation && !approved {
            return Err(AleError::Other(anyhow::anyhow!(
                "操作需要用户确认: {}",
                plan.explanation
            )));
        }

        let mut executed = 0;
        for action in &plan.actions {
            self.execute_action(action)?;
            executed += 1;
            if self.config.action_delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(
                    self.config.action_delay_ms,
                ));
            }
        }

        Ok(ExecutionResult {
            success: true,
            actions_executed: executed,
            error: None,
        })
    }

    /// 执行单个操作
    fn execute_action(&self, action: &Action) -> Result<()> {
        match action {
            Action::OpenUrl { url } => {
                self.open_url(url)?;
            }
            Action::Wait { ms } => {
                std::thread::sleep(std::time::Duration::from_millis(*ms));
            }
            _ => {
                return Err(AleError::Other(anyhow::anyhow!(
                    "iOS 不支持此自动化操作: {:?}",
                    action
                )));
            }
        }
        Ok(())
    }

    /// 打开 URL（通过 objc2 调用 UIApplication.openURL）
    fn open_url(&self, url: &str) -> Result<()> {
        if url.trim().is_empty() {
            return Err(AleError::Other(anyhow::anyhow!("URL 不能为空")));
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(AleError::Other(anyhow::anyhow!(
                "仅支持 http:// 和 https:// URL"
            )));
        }

        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};

        unsafe {
            let ns_url_str: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: std::ffi::CString::new(url).unwrap().as_ptr()];
            let ns_url: *mut AnyObject = msg_send![class!(NSURL), URLWithString: ns_url_str];

            if ns_url.is_null() {
                return Err(AleError::Other(anyhow::anyhow!("无效的 URL: {}", url)));
            }

            let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
            let empty_options: *mut AnyObject = msg_send![class!(NSDictionary), dictionary];
            let completion: Option<&block2::Block<dyn Fn(objc2::runtime::Bool)>> = None;
            let _: () = msg_send![
                app,
                openURL: ns_url,
                options: empty_options,
                completionHandler: completion
            ];
        }

        Ok(())
    }
}
