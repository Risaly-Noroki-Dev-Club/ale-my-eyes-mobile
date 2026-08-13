use crate::AppWindow;
use slint::ComponentHandle;

/// Android 入口点 — Slint + android-activity 后端。
///
/// Android 客户端现在只作为局域网指令入口，不启动本机自动化、相机或前台服务。
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    if let Err(error) = slint::android::init(app) {
        tracing::error!("Failed to initialize Slint Android backend: {}", error);
        return;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!("Failed to create Android runtime: {}", error);
            return;
        }
    };
    let _runtime_guard = runtime.enter();

    // 1. 创建主窗口。权限只在用户触发对应功能时请求。
    let window = match AppWindow::new() {
        Ok(window) => window,
        Err(error) => {
            tracing::error!("Failed to create Android app window: {}", error);
            return;
        }
    };

    // 2. 初始化 Android 遥控端逻辑。
    crate::setup_app(&window);

    // 3. 运行事件循环（阻塞直到 Activity 销毁）
    if let Err(error) = window.run() {
        tracing::error!("Android app exited with error: {}", error);
    }

    tracing::info!("Android app shutdown complete");
}

/// Returns true when the camera can be opened. If permission is missing, this
/// requests it and lets the caller ask the user to tap Scan again afterwards.
pub(crate) fn ensure_camera_permission() -> Result<bool, String> {
    ensure_permission("android.permission.CAMERA", 1002)
}

pub(crate) fn ensure_microphone_permission() -> Result<bool, String> {
    ensure_permission("android.permission.RECORD_AUDIO", 1003)
}

fn ensure_permission(permission_name: &str, request_code: i32) -> Result<bool, String> {
    use jni::objects::JValue;

    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|error| format!("无法访问 Android 权限服务: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("无法检查权限: {error}"))?;
    let activity = unsafe { jni::objects::JObject::from_raw(ctx.context().cast()) };
    let permission = env
        .new_string(permission_name)
        .map_err(|error| format!("无法创建权限请求: {error}"))?;
    let granted = env
        .call_method(
            &activity,
            "checkSelfPermission",
            "(Ljava/lang/String;)I",
            &[JValue::Object(&permission)],
        )
        .and_then(|value| value.i())
        .map_err(|error| format!("无法检查权限: {error}"))?
        == 0;
    if granted {
        return Ok(true);
    }

    let array = env
        .new_object_array(1, "java/lang/String", jni::objects::JObject::null())
        .map_err(|error| format!("无法创建权限请求: {error}"))?;
    env.set_object_array_element(&array, 0, permission)
        .map_err(|error| format!("无法设置权限请求: {error}"))?;
    env.call_method(
        &activity,
        "requestPermissions",
        "([Ljava/lang/String;I)V",
        &[JValue::Object(&array), JValue::Int(request_code)],
    )
    .map_err(|error| format!("无法请求权限: {error}"))?;
    Ok(false)
}
