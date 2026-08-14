use crate::AppWindow;
use slint::ComponentHandle;

/// iOS 入口点 — Slint + iOS 后端
///
/// 生命周期说明：
/// - iOS 应用启动时调用此函数
/// - Slint 的 `window.run()` 内部运行 iOS 事件循环
/// - 需要在 Info.plist 中声明相机和麦克风权限
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn ios_main() {
    // 配置 AVAudioSession（后台音频 + 录音模式）
    configure_audio_session();

    // 请求运行时权限
    request_permissions();

    // 创建主窗口
    let window = match AppWindow::new() {
        Ok(window) => window,
        Err(error) => {
            tracing::error!("Failed to create iOS app window: {}", error);
            return;
        }
    };

    // 启用移动端触控优化
    window.set_is_mobile(true);

    // 初始化应用逻辑（引擎、VAD、回调等）
    crate::setup_app(&window);

    // 运行事件循环
    if let Err(error) = window.run() {
        tracing::error!("iOS app exited with error: {}", error);
    }

    tracing::info!("iOS app shutdown complete");
}

/// 配置 AVAudioSession（录音 + 播放模式）
#[cfg(target_os = "ios")]
fn configure_audio_session() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    unsafe {
        let session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];
        if session.is_null() {
            tracing::error!("Failed to get AVAudioSession sharedInstance");
            return;
        }

        // 设置为 PlayAndRecord 模式（同时支持录音和播放）
        let mode_play_and_record: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: "AVAudioSessionModeDefault\0".as_ptr()];
        let category_play_and_record: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: "AVAudioSessionCategoryPlayAndRecord\0".as_ptr()];

        let mut error: *mut AnyObject = std::ptr::null_mut();
        let _: bool = msg_send![
            session,
            setCategory: category_play_and_record,
            mode: mode_play_and_record,
            options: 1u64,
            error: &mut error
        ];

        if !error.is_null() {
            let desc: *mut AnyObject = msg_send![error, localizedDescription];
            let c_str: *const std::ffi::c_char = msg_send![desc, UTF8String];
            if !c_str.is_null() {
                let msg = std::ffi::CStr::from_ptr(c_str).to_string_lossy();
                tracing::warn!("AVAudioSession setCategory error: {}", msg);
            }
        }

        // 激活音频会话
        let _: bool = msg_send![session, setActive: true, error: &mut error];
        if !error.is_null() {
            tracing::warn!("AVAudioSession setActive error");
        }

        tracing::info!("AVAudioSession configured for PlayAndRecord");
    }
}

/// 请求 iOS 运行时权限（相机、麦克风）
#[cfg(target_os = "ios")]
fn request_permissions() {
    use block2::RcBlock;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    unsafe {
        let microphone_callback = RcBlock::new(|granted: objc2::runtime::Bool| {
            tracing::info!("Microphone permission granted: {}", granted.as_bool());
        });
        let session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];
        let _: () = msg_send![session, requestRecordPermission: &*microphone_callback];
        tracing::info!("Microphone permission requested");

        let camera_callback = RcBlock::new(|granted: objc2::runtime::Bool| {
            tracing::info!("Camera permission granted: {}", granted.as_bool());
        });
        let media_type_video: *mut AnyObject =
            msg_send![class!(NSString), stringWithUTF8String: "vide\0".as_ptr()];
        let _: () = msg_send![
            class!(AVCaptureDevice),
            requestAccessForMediaType: media_type_video,
            completionHandler: &*camera_callback
        ];
        tracing::info!("Camera permission requested");
    }
}
