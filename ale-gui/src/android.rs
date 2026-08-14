use crate::AppWindow;
use slint::ComponentHandle;
use std::sync::Mutex;

static ANDROID_APP: Mutex<Option<slint::android::AndroidApp>> = Mutex::new(None);

/// Android 入口点 — Slint + android-activity 后端。
///
/// Android 客户端现在只作为局域网指令入口，不启动本机自动化、相机或前台服务。
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    if let Ok(mut current_app) = ANDROID_APP.lock() {
        *current_app = Some(app.clone());
    }

    if let Err(error) = slint::android::init(app) {
        tracing::error!("Failed to initialize Slint Android backend: {}", error);
        clear_android_app();
        return;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!("Failed to create Android runtime: {}", error);
            clear_android_app();
            return;
        }
    };
    let _runtime_guard = runtime.enter();

    // 1. 创建主窗口。权限只在用户触发对应功能时请求。
    let window = match AppWindow::new() {
        Ok(window) => window,
        Err(error) => {
            tracing::error!("Failed to create Android app window: {}", error);
            clear_android_app();
            return;
        }
    };

    // 2. 初始化 Android 遥控端逻辑。
    crate::setup_app(&window);

    // 3. 运行事件循环（阻塞直到 Activity 销毁）
    if let Err(error) = window.run() {
        tracing::error!("Android app exited with error: {}", error);
    }

    clear_android_app();
    tracing::info!("Android app shutdown complete");
}

fn clear_android_app() {
    if let Ok(mut current_app) = ANDROID_APP.lock() {
        current_app.take();
    }
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

    let app = ANDROID_APP
        .lock()
        .map_err(|_| "无法访问 Android Activity".to_string())?
        .clone()
        .ok_or_else(|| "Android Activity 尚未就绪".to_string())?;
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }
        .map_err(|error| format!("无法访问 Android 权限服务: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("无法检查权限: {error}"))?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
    let permission = env
        .new_string(permission_name)
        .map_err(|error| format!("无法创建权限请求: {error}"))?;
    let granted = match env
        .call_method(
            &activity,
            "checkSelfPermission",
            "(Ljava/lang/String;)I",
            &[JValue::Object(&permission)],
        )
        .and_then(|value| value.i())
    {
        Ok(value) => value == 0,
        Err(error) => {
            clear_pending_exception(&mut env);
            return Err(format!("无法检查权限: {error}"));
        }
    };
    if granted {
        return Ok(true);
    }

    drop(env);
    let permission_name = permission_name.to_owned();
    let request_app = app.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.run_on_java_main_thread(Box::new(move || {
        let result = request_permission(&request_app, &permission_name, request_code);
        let _ = sender.send(result);
    }));
    receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .map_err(|_| "Android 权限请求超时".to_string())??;
    Ok(false)
}

fn request_permission(
    app: &slint::android::AndroidApp,
    permission_name: &str,
    request_code: i32,
) -> Result<(), String> {
    use jni::objects::JValue;

    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }
        .map_err(|error| format!("无法访问 Android 权限服务: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("无法请求权限: {error}"))?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
    let permission = env
        .new_string(permission_name)
        .map_err(|error| format!("无法创建权限请求: {error}"))?;
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
    .map_err(|error| {
        clear_pending_exception(&mut env);
        format!("无法请求权限: {error}")
    })?;
    Ok(())
}

fn clear_pending_exception(env: &mut jni::JNIEnv<'_>) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}
