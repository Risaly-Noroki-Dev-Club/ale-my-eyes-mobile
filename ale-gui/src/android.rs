use crate::AppWindow;
use slint::ComponentHandle;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static ANDROID_APP: Mutex<Option<slint::android::AndroidApp>> = Mutex::new(None);
static REQUESTED_PERMISSIONS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static ANDROID_TTS: Mutex<Option<jni::objects::GlobalRef>> = Mutex::new(None);
thread_local! {
    static WINDOW: std::cell::RefCell<Option<slint::Weak<AppWindow>>> = const { std::cell::RefCell::new(None) };
}
static FOREGROUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static PERMISSION_DIALOGS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct PermissionDialog;
impl Drop for PermissionDialog {
    fn drop(&mut self) {
        PERMISSION_DIALOGS.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

pub(crate) fn is_foreground() -> bool {
    FOREGROUND.load(std::sync::atomic::Ordering::Acquire)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionState {
    Granted,
    Denied,
    PermanentlyDenied,
}

#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    if let Ok(mut current_app) = ANDROID_APP.lock() {
        *current_app = Some(app.clone());
    }
    if let Err(error) = slint::android::init_with_event_listener(app, |event| {
        use slint::android::android_activity::{MainEvent, PollEvent};
        match event {
            PollEvent::Main(MainEvent::Pause) => {
                FOREGROUND.store(false, std::sync::atomic::Ordering::Release);
                // A runtime permission dialog pauses the Activity too. Keep its
                // pending request alive; actual backgrounding still emits Stop.
                if PERMISSION_DIALOGS.load(std::sync::atomic::Ordering::Acquire) == 0 {
                    WINDOW.with(|slot| {
                        if let Some(window) = slot.borrow().as_ref().and_then(|w| w.upgrade()) {
                            window.invoke_backgrounded();
                        }
                    });
                }
            }
            PollEvent::Main(MainEvent::Stop) => {
                FOREGROUND.store(false, std::sync::atomic::Ordering::Release);
                WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow().as_ref().and_then(|w| w.upgrade()) {
                        window.invoke_backgrounded();
                    }
                });
            }
            PollEvent::Main(MainEvent::Resume { .. }) => {
                FOREGROUND.store(true, std::sync::atomic::Ordering::Release);
                WINDOW.with(|slot| {
                    if let Some(window) = slot.borrow().as_ref().and_then(|w| w.upgrade()) {
                        window.set_font_scale(font_scale().unwrap_or(1.0));
                    }
                });
            }
            _ => {}
        }
    }) {
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
    let window = match AppWindow::new() {
        Ok(window) => window,
        Err(error) => {
            tracing::error!("Failed to create Android app window: {}", error);
            clear_android_app();
            return;
        }
    };
    crate::setup_app(&window);
    WINDOW.with(|slot| *slot.borrow_mut() = Some(window.as_weak()));
    if let Err(error) = window.run() {
        tracing::error!("Android app exited with error: {}", error);
    }
    clear_android_app();
    WINDOW.with(|slot| slot.borrow_mut().take());
}

fn with_android<T>(
    f: impl for<'local> FnOnce(
        &mut jni::JNIEnv<'local>,
        &jni::objects::JObject<'local>,
    ) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let app = current_app()?;
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let result = env.with_local_frame(32, |env| {
        let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
        f(env, &activity)
    });
    result.map_err(|e: jni::errors::Error| {
        clear_pending_exception(&mut env);
        e.to_string()
    })
}

pub(crate) fn load_preferences() -> Result<crate::preferences::Preferences, String> {
    with_android(|env, activity| {
        use jni::objects::{JObject, JString, JValue};
        let name = env.new_string("mobile_preferences")?;
        let prefs = env
            .call_method(
                activity,
                "getSharedPreferences",
                "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
                &[JValue::Object(&name), JValue::Int(0)],
            )?
            .l()?;
        let mut values = std::collections::HashMap::new();
        for (key, _) in crate::preferences::Preferences::default().entries() {
            let java_key = env.new_string(key)?;
            let value = env
                .call_method(
                    &prefs,
                    "getString",
                    "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                    &[JValue::Object(&java_key), JValue::Object(&JObject::null())],
                )?
                .l()?;
            if !value.is_null() {
                values.insert(
                    key.to_string(),
                    env.get_string(&JString::from(value))?.into(),
                );
            }
        }
        Ok(crate::preferences::Preferences::from_values(|key| {
            values.remove(key)
        }))
    })
}

pub(crate) fn save_preferences(preferences: crate::preferences::Preferences) -> Result<(), String> {
    let saved = with_android(|env, activity| {
        use jni::objects::JValue;
        let name = env.new_string("mobile_preferences")?;
        let prefs = env
            .call_method(
                activity,
                "getSharedPreferences",
                "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
                &[JValue::Object(&name), JValue::Int(0)],
            )?
            .l()?;
        let editor = env
            .call_method(
                &prefs,
                "edit",
                "()Landroid/content/SharedPreferences$Editor;",
                &[],
            )?
            .l()?;
        for (key, value) in preferences.entries() {
            let key = env.new_string(key)?;
            let value = env.new_string(value.to_string())?;
            env.call_method(
                &editor,
                "putString",
                "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",
                &[JValue::Object(&key), JValue::Object(&value)],
            )?;
        }
        env.call_method(&editor, "commit", "()Z", &[])?.z()
    })?;
    if saved {
        Ok(())
    } else {
        Err("无法保存设置，请重试".into())
    }
}

pub(crate) fn set_speech_language(english: bool) -> Result<(), String> {
    let status = with_android(|env, _| {
        use jni::objects::JValue;
        let guard = ANDROID_TTS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(tts) = guard.as_ref() else {
            return Ok(-2);
        };
        let locale = env
            .get_static_field(
                "java/util/Locale",
                if english { "US" } else { "SIMPLIFIED_CHINESE" },
                "Ljava/util/Locale;",
            )?
            .l()?;
        let available = env
            .call_method(
                tts.as_obj(),
                "isLanguageAvailable",
                "(Ljava/util/Locale;)I",
                &[JValue::Object(&locale)],
            )?
            .i()?;
        if available < 0 {
            return Ok(available);
        }
        env.call_method(
            tts.as_obj(),
            "setLanguage",
            "(Ljava/util/Locale;)I",
            &[JValue::Object(&locale)],
        )?
        .i()
    })?;
    if status >= 0 {
        Ok(())
    } else {
        Err("朗读引擎尚未就绪或缺少所选语言，请检查系统语音设置后重试".into())
    }
}

pub(crate) fn vibrate() {
    if let Err(error) = with_android(|env, activity| {
        use jni::objects::JValue;
        let name = env.new_string("vibrator")?;
        let vibrator = env
            .call_method(
                activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        if !vibrator.is_null() && env.call_method(&vibrator, "hasVibrator", "()Z", &[])?.z()? {
            let effect = env
                .call_static_method(
                    "android/os/VibrationEffect",
                    "createOneShot",
                    "(JI)Landroid/os/VibrationEffect;",
                    &[JValue::Long(35), JValue::Int(-1)],
                )?
                .l()?;
            env.call_method(
                &vibrator,
                "vibrate",
                "(Landroid/os/VibrationEffect;)V",
                &[JValue::Object(&effect)],
            )?;
        }
        Ok(())
    }) {
        tracing::warn!("Haptic feedback unavailable: {error}");
    }
}

pub(crate) fn font_scale() -> Result<f32, String> {
    with_android(|env, activity| {
        let resources = env
            .call_method(
                activity,
                "getResources",
                "()Landroid/content/res/Resources;",
                &[],
            )?
            .l()?;
        let config = env
            .call_method(
                &resources,
                "getConfiguration",
                "()Landroid/content/res/Configuration;",
                &[],
            )?
            .l()?;
        Ok(env
            .get_field(&config, "fontScale", "F")?
            .f()?
            .clamp(1.0, 3.0))
    })
}

pub(crate) async fn ensure_camera_permission() -> Result<PermissionState, String> {
    ensure_permission("android.permission.CAMERA", 1002).await
}

pub(crate) async fn ensure_microphone_permission() -> Result<PermissionState, String> {
    ensure_permission("android.permission.RECORD_AUDIO", 1003).await
}

pub(crate) fn open_app_settings() -> Result<(), String> {
    let app = current_app()?;
    let request_app = app.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.run_on_java_main_thread(Box::new(move || {
        let result = open_settings_on_main_thread(&request_app);
        let _ = sender.send(result);
    }));
    receiver
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "打开系统设置超时".to_string())?
}

pub(crate) fn initialize_tts() -> Result<(), String> {
    let app = current_app()?;
    let request_app = app.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.run_on_java_main_thread(Box::new(move || {
        let result = create_tts_on_main_thread(&request_app);
        let _ = sender.send(result);
    }));
    let tts = receiver
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "初始化 Android TTS 超时".to_string())??;
    *ANDROID_TTS
        .lock()
        .map_err(|_| "无法保存 Android TTS 状态".to_string())? = Some(tts);
    Ok(())
}

pub(crate) fn speak(text: &str, interrupt: bool) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let app = current_app()?;
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }
        .map_err(|error| format!("无法访问 Android TTS: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("无法调用 Android TTS: {error}"))?;
    let guard = ANDROID_TTS
        .lock()
        .map_err(|_| "无法访问 Android TTS 状态".to_string())?;
    let tts = guard
        .as_ref()
        .ok_or_else(|| "Android TTS 尚未初始化".to_string())?;
    let speech = env
        .new_string(text)
        .map_err(|error| format!("无法创建播报文本: {error}"))?;
    let utterance_id = env
        .new_string(format!("ale-{}", uuid::Uuid::new_v4()))
        .map_err(|error| format!("无法创建播报 ID: {error}"))?;
    let bundle = jni::objects::JObject::null();
    let queue_mode = if interrupt { 0 } else { 1 };
    let status = env
        .call_method(
            tts.as_obj(),
            "speak",
            "(Ljava/lang/CharSequence;ILandroid/os/Bundle;Ljava/lang/String;)I",
            &[
                jni::objects::JValue::Object(&speech),
                jni::objects::JValue::Int(queue_mode),
                jni::objects::JValue::Object(&bundle),
                jni::objects::JValue::Object(&utterance_id),
            ],
        )
        .and_then(|value| value.i())
        .map_err(|error| {
            clear_pending_exception(&mut env);
            format!("Android TTS 播报失败: {error}")
        })?;
    if status == 0 {
        Ok(())
    } else {
        Err("Android TTS 尚未准备完成".to_string())
    }
}

pub(crate) fn stop_tts() {
    let Ok(app) = current_app() else {
        return;
    };
    let Ok(vm) = (unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }) else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let Ok(guard) = ANDROID_TTS.lock() else {
        return;
    };
    if let Some(tts) = guard.as_ref() {
        if env.call_method(tts.as_obj(), "stop", "()I", &[]).is_err() {
            clear_pending_exception(&mut env);
        }
    }
}

async fn ensure_permission(
    permission_name: &'static str,
    request_code: i32,
) -> Result<PermissionState, String> {
    let app = current_app()?;
    if check_permission(&app, permission_name)? {
        return Ok(PermissionState::Granted);
    }
    if PERMISSION_DIALOGS.load(std::sync::atomic::Ordering::Acquire) > 0 {
        return Err("已有权限请求正在处理，请完成系统弹窗后重试".into());
    }
    let requested_before = REQUESTED_PERMISSIONS
        .lock()
        .map_err(|_| "无法读取权限状态".to_string())?
        .contains(&permission_name);
    if requested_before && !should_show_rationale(&app, permission_name)? {
        return Ok(PermissionState::PermanentlyDenied);
    }
    if let Ok(mut requested) = REQUESTED_PERMISSIONS.lock() {
        if !requested.contains(&permission_name) {
            requested.push(permission_name);
        }
    }

    PERMISSION_DIALOGS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let _dialog = PermissionDialog;
    let request_app = app.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.run_on_java_main_thread(Box::new(move || {
        let result = request_permission(&request_app, permission_name, request_code);
        let _ = sender.send(result);
    }));
    receiver
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Android 权限请求超时".to_string())??;

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut saw_pause = false;
    let mut resumed_at = None;
    while Instant::now() < deadline {
        if check_permission(&app, permission_name)? && is_foreground() {
            return Ok(PermissionState::Granted);
        }
        if !is_foreground() {
            saw_pause = true;
            resumed_at = None;
        } else if saw_pause {
            let resumed = resumed_at.get_or_insert_with(Instant::now);
            if resumed.elapsed() >= Duration::from_millis(500) {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if !requested_before || should_show_rationale(&app, permission_name)? {
        Ok(PermissionState::Denied)
    } else {
        Ok(PermissionState::PermanentlyDenied)
    }
}

pub(crate) fn display_rotation() -> Result<u32, String> {
    use jni::objects::JValue;
    let app = current_app()?;
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = (|| {
        let manager = env
            .call_method(
                &activity,
                "getWindowManager",
                "()Landroid/view/WindowManager;",
                &[] as &[JValue],
            )?
            .l()?;
        let display = env
            .call_method(
                &manager,
                "getDefaultDisplay",
                "()Landroid/view/Display;",
                &[],
            )?
            .l()?;
        env.call_method(&display, "getRotation", "()I", &[])?.i()
    })();
    result
        .map(|rotation| (rotation as u32 % 4) * 90)
        .map_err(|error: jni::errors::Error| {
            clear_pending_exception(&mut env);
            format!("无法读取屏幕方向: {error}")
        })
}

fn current_app() -> Result<slint::android::AndroidApp, String> {
    ANDROID_APP
        .lock()
        .map_err(|_| "无法访问 Android Activity".to_string())?
        .clone()
        .ok_or_else(|| "Android Activity 尚未就绪".to_string())
}

fn check_permission(
    app: &slint::android::AndroidApp,
    permission_name: &str,
) -> Result<bool, String> {
    use jni::objects::JValue;
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }
        .map_err(|error| format!("无法访问 Android 权限服务: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("无法检查权限: {error}"))?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
    let permission = env
        .new_string(permission_name)
        .map_err(|error| format!("无法创建权限请求: {error}"))?;
    env.call_method(
        &activity,
        "checkSelfPermission",
        "(Ljava/lang/String;)I",
        &[JValue::Object(&permission)],
    )
    .and_then(|value| value.i())
    .map(|value| value == 0)
    .map_err(|error| {
        clear_pending_exception(&mut env);
        format!("无法检查权限: {error}")
    })
}

fn should_show_rationale(
    app: &slint::android::AndroidApp,
    permission_name: &str,
) -> Result<bool, String> {
    use jni::objects::JValue;
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }
        .map_err(|error| format!("无法访问 Android 权限服务: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("无法读取权限状态: {error}"))?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
    let permission = env
        .new_string(permission_name)
        .map_err(|error| format!("无法创建权限名称: {error}"))?;
    env.call_method(
        &activity,
        "shouldShowRequestPermissionRationale",
        "(Ljava/lang/String;)Z",
        &[JValue::Object(&permission)],
    )
    .and_then(|value| value.z())
    .map_err(|error| {
        clear_pending_exception(&mut env);
        format!("无法读取权限状态: {error}")
    })
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

fn open_settings_on_main_thread(app: &slint::android::AndroidApp) -> Result<(), String> {
    use jni::objects::{JObject, JValue};
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }
        .map_err(|error| format!("无法访问 Android 设置: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("无法打开 Android 设置: {error}"))?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let action = env
        .new_string("android.settings.APPLICATION_DETAILS_SETTINGS")
        .map_err(|error| error.to_string())?;
    let intent_class = env
        .find_class("android/content/Intent")
        .map_err(|error| error.to_string())?;
    let intent = env
        .new_object(
            &intent_class,
            "(Ljava/lang/String;)V",
            &[JValue::Object(&action)],
        )
        .map_err(|error| error.to_string())?;
    let uri_class = env
        .find_class("android/net/Uri")
        .map_err(|error| error.to_string())?;
    let uri_text = env
        .new_string("package:com.alemyeyes")
        .map_err(|error| error.to_string())?;
    let uri = env
        .call_static_method(
            &uri_class,
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&uri_text)],
        )
        .and_then(|value| value.l())
        .map_err(|error| error.to_string())?;
    env.call_method(
        &intent,
        "setData",
        "(Landroid/net/Uri;)Landroid/content/Intent;",
        &[JValue::Object(&uri)],
    )
    .map_err(|error| error.to_string())?;
    env.call_method(
        &activity,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&intent)],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn create_tts_on_main_thread(
    app: &slint::android::AndroidApp,
) -> Result<jni::objects::GlobalRef, String> {
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }
        .map_err(|error| format!("无法访问 Android TTS: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("无法初始化 Android TTS: {error}"))?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
    let listener = jni::objects::JObject::null();
    let tts = env
        .new_object(
            "android/speech/tts/TextToSpeech",
            "(Landroid/content/Context;Landroid/speech/tts/TextToSpeech$OnInitListener;)V",
            &[
                jni::objects::JValue::Object(&activity),
                jni::objects::JValue::Object(&listener),
            ],
        )
        .map_err(|error| {
            clear_pending_exception(&mut env);
            format!("无法创建 Android TTS: {error}")
        })?;
    env.new_global_ref(tts)
        .map_err(|error| format!("无法保存 Android TTS: {error}"))
}

fn clear_android_app() {
    stop_tts();
    if let Ok(mut tts) = ANDROID_TTS.lock() {
        tts.take();
    }
    if let Ok(mut current_app) = ANDROID_APP.lock() {
        current_app.take();
    }
}

fn clear_pending_exception(env: &mut jni::JNIEnv<'_>) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}
