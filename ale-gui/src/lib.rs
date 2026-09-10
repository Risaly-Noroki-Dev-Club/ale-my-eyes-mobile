#![cfg(target_os = "android")]

mod android;
mod audio;
mod operation_epoch;
mod preferences;
mod qr_scanner;

use ale_core::remote::{
    CommandPreview, DecisionRequest, PairingInfo, MAX_AUDIO_CHUNK_BYTES, MAX_RECORDING_SECONDS,
};
use ale_core::{RemoteSession, RemoteSessionEvent};
use slint::ComponentHandle;
use std::future::Future;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

slint::include_modules!();

struct AppState {
    session: Option<RemoteSession>,
    session_generation: u64,
    recorder: Option<audio::Recorder>,
    recording_request_id: Option<String>,
    pending_request_id: Option<String>,
    pending_decision: Option<DecisionRequest>,
    recording_started: Option<Instant>,
    cancelled_requests: std::collections::VecDeque<String>,
}

impl AppState {
    fn new() -> Self {
        Self {
            session: None,
            session_generation: 0,
            recorder: None,
            recording_request_id: None,
            pending_request_id: None,
            pending_decision: None,
            recording_started: None,
            cancelled_requests: std::collections::VecDeque::new(),
        }
    }

    fn clear_request(&mut self) {
        self.recorder.take();
        self.recording_request_id = None;
        self.pending_request_id = None;
        self.pending_decision = None;
        self.recording_started = None;
    }
}

pub fn setup_app(app: &AppWindow) {
    app.set_app_version(env!("CARGO_PKG_VERSION").into());
    app.set_protocol_version(
        format!("ale-my-eyes v{}", ale_core::remote::REMOTE_PROTOCOL_VERSION).into(),
    );
    app.set_font_scale(android::font_scale().unwrap_or(1.0));
    app.set_remote_status("扫描桌面端 v3 二维码以开始".into());
    if let Err(error) = android::initialize_tts() {
        tracing::warn!("Android TTS initialization failed: {}", error);
    }
    let state = Arc::new(Mutex::new(AppState::new()));
    let stream_gate = Arc::new(Mutex::new(()));
    let drain_running = Arc::new(AtomicBool::new(false));
    let qr_scan_cancel = Arc::new(std::sync::Mutex::new(
        None::<Arc<std::sync::atomic::AtomicBool>>,
    ));

    let recording_timer = Rc::new(slint::Timer::default());
    {
        let state = state.clone();
        let stream_gate = stream_gate.clone();
        let drain_running = drain_running.clone();
        let app_weak = app.as_weak();
        recording_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                if drain_running.swap(true, Ordering::AcqRel) {
                    return;
                }
                let state = state.clone();
                let stream_gate = stream_gate.clone();
                let drain_running = drain_running.clone();
                let app_weak = app_weak.clone();
                spawn_local_task(async move {
                    recording_tick(state, stream_gate, app_weak).await;
                    drain_running.store(false, Ordering::Release);
                });
            },
        );
    }

    wire_scan(
        app,
        state.clone(),
        stream_gate.clone(),
        qr_scan_cancel.clone(),
    );
    wire_recording(app, state.clone(), stream_gate.clone(), recording_timer);
    wire_confirmation(app, state.clone());
    wire_disconnect(app, state.clone(), qr_scan_cancel);
    wire_mobile_ui(app);

    let app_weak = app.as_weak();
    app.on_open_app_settings(move || {
        if let Err(error) = android::open_app_settings() {
            if let Some(app) = app_weak.upgrade() {
                app.set_remote_status(slint::format!("无法打开系统设置: {}", error));
            }
        }
    });
}

fn apply_preferences(app: &AppWindow, prefs: preferences::Preferences) {
    app.set_speak_results(prefs.speak_results);
    app.set_haptics(prefs.haptics);
    app.set_auto_scan(prefs.auto_scan);
    app.set_english_voice(prefs.english_voice);
}

fn feedback(app: &AppWindow) {
    if app.get_haptics() {
        android::vibrate();
    }
}

fn wire_mobile_ui(app: &AppWindow) {
    let prefs = match android::load_preferences() {
        Ok(prefs) => prefs,
        Err(error) => {
            app.set_settings_message(format!("设置读取失败，已使用默认值：{error}").into());
            preferences::Preferences::default()
        }
    };
    apply_preferences(app, prefs);
    let current = Rc::new(std::cell::Cell::new(prefs));
    let weak = app.as_weak();
    app.on_change_preference(move |key, value| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let previous = current.get();
        let mut updated = previous;
        if !updated.set(key.as_str(), value) {
            return;
        }
        if key == "english_voice" {
            if let Err(error) = android::set_speech_language(value) {
                app.set_settings_message(error.into());
                return;
            }
        }
        if let Err(error) = android::save_preferences(updated) {
            if key == "english_voice" {
                let _ = android::set_speech_language(previous.english_voice);
            }
            app.set_settings_message(error.into());
            return;
        }
        current.set(updated);
        apply_preferences(&app, updated);
        app.set_settings_message("设置已保存".into());
        if key == "speak_results" && !value && !app.get_show_confirmation() {
            android::stop_tts();
        }
    });
    let weak = app.as_weak();
    app.on_change_page(move |page| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        if app.get_voice_recording() || app.get_input_pending() || app.get_show_confirmation() {
            return;
        }
        if app.get_current_page() == MobilePage::Scan
            && page != MobilePage::Scan
            && !app.get_remote_connected()
        {
            app.invoke_cancel_remote_scan();
        }
        app.set_current_page(page);
    });
    let weak = app.as_weak();
    app.on_backgrounded(move || {
        if let Some(app) = weak.upgrade() {
            if !app.get_remote_connected() {
                app.invoke_cancel_remote_scan();
            }
            if app.get_voice_recording() || app.get_input_pending() {
                app.invoke_cancel_request();
            }
            android::stop_tts();
        }
    });
    let weak = app.as_weak();
    app.on_back(move || {
        if let Some(app) = weak.upgrade() {
            if app.get_confirm_disconnect() {
                app.set_confirm_disconnect(false);
            } else if app.get_show_confirmation() {
                app.invoke_cancel_action();
            } else if app.get_voice_recording() || app.get_input_pending() {
                app.invoke_cancel_request();
            } else if !app.get_remote_connected()
                && (app.get_remote_scanning() || app.get_is_busy())
            {
                app.invoke_cancel_remote_scan();
            } else if app.get_current_page() != MobilePage::Scan {
                app.invoke_change_page(MobilePage::Scan);
            }
        }
    });
    // TTS initialization is asynchronous: retry readiness, never block the UI loop.
    let weak = app.as_weak();
    spawn_local_task(async move {
        for _ in 0..20 {
            let Some(app) = weak.upgrade() else {
                return;
            };
            if android::set_speech_language(app.get_english_voice()).is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if let Some(app) = weak.upgrade() {
            app.set_settings_message("朗读引擎不可用，请检查系统语音设置；仍可查看文字结果".into());
        }
    });
    // Deliberately scheduled exactly once per app instance, never on tab changes/resume.
    if prefs.auto_scan {
        let weak = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(300), move || {
            if let Some(app) = weak.upgrade() {
                if android::is_foreground()
                    && app.get_current_page() == MobilePage::Scan
                    && !app.get_remote_connected()
                    && !app.get_is_busy()
                {
                    app.invoke_scan_remote();
                }
            }
        });
    }
}

fn wire_scan(
    app: &AppWindow,
    state: Arc<Mutex<AppState>>,
    stream_gate: Arc<Mutex<()>>,
    qr_scan_cancel: Arc<std::sync::Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>>,
) {
    let app_weak = app.as_weak();
    let scan_state = state.clone();
    let scan_cancel = qr_scan_cancel.clone();
    app.on_scan_remote(move || {
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        if app.get_remote_connected() || app.get_is_busy() || !android::is_foreground() {
            return;
        }
        app.set_is_busy(true);
        let token = Arc::new(AtomicBool::new(false));
        if let Ok(mut slot) = scan_cancel.lock() {
            if let Some(previous) = slot.replace(token.clone()) {
                previous.store(true, Ordering::Release);
            }
        }
        let app_weak = app_weak.clone();
        let state = scan_state.clone();
        let stream_gate = stream_gate.clone();
        let qr_scan_cancel = scan_cancel.clone();
        spawn_local_task(async move {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_show_settings_action(false);
            let permission = android::ensure_camera_permission().await;
            if token.load(Ordering::Acquire) {
                return;
            }
            app.set_is_busy(false);
            match permission {
                Ok(android::PermissionState::Granted) => {
                    start_qr_scan(app_weak, state, stream_gate, qr_scan_cancel, token);
                }
                Ok(android::PermissionState::Denied) => {
                    app.set_remote_status("相机权限已拒绝，请再次扫描并允许权限".into());
                }
                Ok(android::PermissionState::PermanentlyDenied) => {
                    app.set_show_settings_action(true);
                    app.set_remote_status("相机权限已永久拒绝，请在系统设置中允许".into());
                }
                Err(error) => {
                    app.set_remote_status(slint::format!("相机权限错误: {}", error));
                }
            }
        });
    });

    let app_weak = app.as_weak();
    app.on_cancel_remote_scan(move || {
        if let Ok(mut slot) = qr_scan_cancel.lock() {
            if let Some(cancel) = slot.take() {
                cancel.store(true, Ordering::Release);
            }
        }
        if let Some(app) = app_weak.upgrade() {
            app.set_remote_scanning(false);
            app.set_is_busy(false);
            app.set_remote_status("扫描已取消".into());
        }
    });
}

fn start_qr_scan(
    app_weak: slint::Weak<AppWindow>,
    state: Arc<Mutex<AppState>>,
    stream_gate: Arc<Mutex<()>>,
    qr_scan_cancel: Arc<std::sync::Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>>,
    cancelled: Arc<AtomicBool>,
) {
    let Some(app) = app_weak.upgrade() else {
        return;
    };
    app.set_is_busy(true);
    app.set_remote_connected(false);
    app.set_remote_scanning(true);
    app.set_remote_status("将电脑上的 v3 二维码放入框内".into());

    std::thread::spawn(move || {
        let preview_app = app_weak.clone();
        let scan_token = cancelled.clone();
        let preview_token = cancelled.clone();
        let scan_result = qr_scanner::scan_pairing_info(cancelled, move |gray, width, height| {
            let token = preview_token.clone();
            let _ = preview_app.upgrade_in_event_loop(move |app| {
                if token.load(Ordering::Acquire) {
                    return;
                }
                let mut pixels =
                    slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width as u32, height as u32);
                for (pixel, value) in pixels.make_mut_slice().iter_mut().zip(gray) {
                    *pixel = slint::Rgb8Pixel::new(value, value, value);
                }
                app.set_remote_scan_preview(slint::Image::from_rgb8(pixels));
            });
        });
        let should_publish = qr_scan_cancel.lock().is_ok_and(|slot| {
            if slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &scan_token))
            {
                true
            } else {
                false
            }
        });
        if !should_publish {
            return;
        }
        let _ = app_weak.upgrade_in_event_loop(move |app| {
            if scan_token.load(Ordering::Acquire) {
                return;
            }
            match scan_result {
                Ok(pairing) => {
                    app.set_remote_scanning(false);
                    app.set_remote_status("二维码有效，正在建立 v3 加密会话".into());
                    let app_weak = app.as_weak();
                    spawn_local_task(connect_session(
                        pairing,
                        state,
                        stream_gate,
                        app_weak,
                        scan_token,
                    ));
                }
                Err(error) => {
                    app.set_remote_scanning(false);
                    app.set_remote_status(slint::format!("扫描失败: {}", error));
                    app.set_is_busy(false);
                }
            }
        });
    });
}

async fn connect_session(
    pairing: PairingInfo,
    state: Arc<Mutex<AppState>>,
    stream_gate: Arc<Mutex<()>>,
    app_weak: slint::Weak<AppWindow>,
    scan_token: Arc<AtomicBool>,
) {
    let address = format!("{}:{}", pairing.host, pairing.port);
    let result = RemoteSession::connect(pairing).await;
    if scan_token.load(Ordering::Acquire) {
        if let Ok(session) = result {
            session.shutdown().await;
        }
        return;
    }
    let Some(app) = app_weak.upgrade() else {
        return;
    };
    match result {
        Ok(session) => {
            let name = session.server_name().to_string();
            let mut events = session.subscribe();
            let generation = {
                let _gate = stream_gate.lock().await;
                if scan_token.load(Ordering::Acquire) {
                    session.shutdown().await;
                    return;
                }
                let mut state = state.lock().await;
                state.clear_request();
                state.session_generation = state.session_generation.wrapping_add(1);
                let generation = state.session_generation;
                state.session = Some(session);
                generation
            };
            app.set_remote_status(slint::format!("已连接: {}", name));
            app.set_remote_connected(true);
            app.set_device_name(name.into());
            app.set_device_address(address.into());
            app.set_ai_response("".into());
            app.set_action_steps("".into());
            feedback(&app);
            app.set_is_busy(false);
            app.set_show_settings_action(false);
            let state = state.clone();
            let app_weak = app.as_weak();
            spawn_local_task(async move {
                loop {
                    if events.changed().await.is_err() {
                        return;
                    }
                    let event = events.borrow_and_update().clone();
                    if state.lock().await.session_generation != generation {
                        return;
                    }
                    match event {
                        Some(RemoteSessionEvent::Disconnected(error)) => {
                            let mut state = state.lock().await;
                            if state.session_generation != generation {
                                return;
                            }
                            state.session = None;
                            state.clear_request();
                            drop(state);
                            if let Some(app) = app_weak.upgrade() {
                                reset_request_ui(&app);
                                app.set_remote_connected(false);
                                app.set_is_busy(false);
                                if error.code != "USER_CLOSED" {
                                    app.set_remote_status(slint::format!(
                                        "连接已断开: {}。请重新扫码",
                                        error.message
                                    ));
                                }
                            }
                            return;
                        }
                        Some(RemoteSessionEvent::ProtocolWarning(message)) => {
                            tracing::warn!("Remote protocol warning: {}", message);
                        }
                        Some(RemoteSessionEvent::Progress(progress)) => {
                            if state.lock().await.pending_request_id.as_deref()
                                != Some(progress.request_id.as_str())
                            {
                                continue;
                            }
                            if let Some(app) = app_weak.upgrade() {
                                app.set_remote_status(progress.message.into());
                            }
                        }
                        Some(RemoteSessionEvent::DecisionRequested(decision)) => {
                            let mut current = state.lock().await;
                            if current.session_generation != generation {
                                return;
                            }
                            current.pending_decision = Some(decision.clone());
                            if current.pending_request_id.as_deref()
                                != Some(decision.request_id.as_str())
                            {
                                current.pending_decision = None;
                                continue;
                            }
                            drop(current);
                            if let Some(app) = app_weak.upgrade() {
                                app.set_confirmation_title("需要你的决定".into());
                                app.set_confirmation_text(decision.prompt.clone().into());
                                app.set_confirmation_confirm_label("是".into());
                                app.set_confirmation_cancel_label("否".into());
                                app.set_show_confirmation(true);
                                app.set_is_busy(false);
                            }
                            if let Err(error) = android::speak(&decision.prompt, true) {
                                tracing::warn!("Android TTS failed: {}", error);
                            }
                        }
                        Some(RemoteSessionEvent::AssistantOutput(output)) => {
                            if let Some(id) = &output.request_id {
                                if state.lock().await.cancelled_requests.contains(id) {
                                    continue;
                                }
                            }
                            if let Some(app) = app_weak.upgrade() {
                                app.set_ai_response(output.display_text.into());
                                if app.get_speak_results() {
                                    if let Err(error) =
                                        android::speak(&output.speech_text, output.interrupt)
                                    {
                                        tracing::warn!("Android TTS failed: {}", error);
                                    }
                                }
                            }
                        }
                        None => {}
                    }
                }
            });
        }
        Err(error) => {
            app.set_remote_connected(false);
            app.set_is_busy(false);
            app.set_remote_status(
                if error.code == ale_core::remote::error_code::PROTOCOL_INCOMPATIBLE {
                    "桌面端版本过旧，需要支持远程协议 v3".into()
                } else {
                    slint::format!("连接失败: {}", error.message)
                },
            );
        }
    }
}

fn wire_recording(
    app: &AppWindow,
    state: Arc<Mutex<AppState>>,
    stream_gate: Arc<Mutex<()>>,
    recording_timer: Rc<slint::Timer>,
) {
    let input_epoch = Arc::new(operation_epoch::OperationEpoch::default());
    let start_epoch = input_epoch.clone();
    let toggle_state = state.clone();
    let toggle_gate = stream_gate.clone();
    let app_weak = app.as_weak();
    app.on_toggle_listening(move || {
        let _keep_timer_alive = recording_timer.clone();
        let state = toggle_state.clone();
        let stream_gate = toggle_gate.clone();
        let app_weak = app_weak.clone();
        let input_epoch = start_epoch.clone();
        spawn_local_task(async move {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if !app.get_remote_connected()
                || app.get_is_busy()
                || app.get_input_pending()
                || app.get_show_confirmation()
                || !android::is_foreground()
            {
                return;
            }
            let is_recording = state.lock().await.recorder.is_some();
            if is_recording {
                finalize_recording(state, stream_gate, app_weak).await;
                return;
            }
            app.set_show_settings_action(false);
            app.set_input_pending(true);
            let ticket = input_epoch.advance();
            let permission = android::ensure_microphone_permission().await;
            if !app.get_input_pending() || !input_epoch.is_current(ticket) {
                return;
            }
            if !android::is_foreground() || !app.get_remote_connected() {
                app.set_input_pending(false);
                return;
            }
            match permission {
                Ok(android::PermissionState::Granted) => {}
                Ok(android::PermissionState::Denied) => {
                    app.set_input_pending(false);
                    app.set_remote_status("麦克风权限已拒绝，请再次开始并允许权限".into());
                    return;
                }
                Ok(android::PermissionState::PermanentlyDenied) => {
                    app.set_input_pending(false);
                    app.set_show_settings_action(true);
                    app.set_remote_status("麦克风权限已永久拒绝，请在系统设置中允许".into());
                    return;
                }
                Err(error) => {
                    app.set_input_pending(false);
                    app.set_remote_status(slint::format!("麦克风权限错误: {}", error));
                    return;
                }
            }
            start_recording(state, stream_gate, &app, &input_epoch, ticket).await;
            if input_epoch.is_current(ticket) {
                app.set_input_pending(false);
            }
        });
    });

    let state = state.clone();
    let app_weak = app.as_weak();
    app.on_cancel_request(move || {
        input_epoch.advance();
        if let Some(app) = app_weak.upgrade() {
            app.set_input_pending(false);
            app.set_voice_recording(false);
            app.set_is_busy(true);
            feedback(&app);
        }
        let state = state.clone();
        let app_weak = app_weak.clone();
        spawn_local_task(async move {
            let (session, request_id, recorder) = {
                let mut state = state.lock().await;
                let session = state.session.clone();
                let recording_id = state.recording_request_id.take();
                let pending_id = state.pending_request_id.take();
                let request_id = recording_id.or(pending_id);
                if let Some(id) = &request_id {
                    state.cancelled_requests.push_back(id.clone());
                    if state.cancelled_requests.len() > 32 {
                        state.cancelled_requests.pop_front();
                    }
                }
                state.pending_decision = None;
                state.recording_started = None;
                (session, request_id, state.recorder.take())
            };
            drop(recorder);
            if let (Some(session), Some(request_id)) = (session, request_id) {
                let _ = session.cancel(request_id).await;
            }
            if let Some(app) = app_weak.upgrade() {
                reset_request_ui(&app);
                app.set_is_busy(false);
                app.set_remote_status("请求已取消".into());
            }
        });
    });
}

async fn start_recording(
    state: Arc<Mutex<AppState>>,
    stream_gate: Arc<Mutex<()>>,
    app: &AppWindow,
    input_epoch: &operation_epoch::OperationEpoch,
    ticket: u64,
) {
    android::stop_tts();
    let _gate = stream_gate.lock().await;
    let recorder = match audio::Recorder::start() {
        Ok(recorder) => recorder,
        Err(error) => {
            app.set_remote_status(slint::format!("无法开始录音: {}", error));
            return;
        }
    };
    let session = state.lock().await.session.clone();
    let Some(session) = session else {
        recorder.stop();
        app.set_remote_status("远程会话已经断开，请重新扫码".into());
        return;
    };
    let request_id = match session
        .begin_audio(recorder.sample_rate_hz(), recorder.channels())
        .await
    {
        Ok(request_id) => request_id,
        Err(error) => {
            recorder.stop();
            app.set_remote_status(slint::format!("无法开始远程录音: {}", error.message));
            return;
        }
    };
    let mut state = state.lock().await;
    if !input_epoch.is_current(ticket)
        || !app.get_input_pending()
        || !android::is_foreground()
        || state.session.is_none()
    {
        drop(state);
        recorder.stop();
        let _ = session.cancel(request_id).await;
        return;
    }
    state.recorder = Some(recorder);
    state.recording_request_id = Some(request_id.clone());
    state.pending_request_id = Some(request_id);
    state.recording_started = Some(Instant::now());
    drop(state);
    app.set_voice_recording(true);
    app.set_ai_response("".into());
    app.set_action_steps("".into());
    feedback(app);
    app.set_recording_time("00:00 / 01:00".into());
    app.set_remote_status("正在录音，再点一次即可发送".into());
}

async fn recording_tick(
    state: Arc<Mutex<AppState>>,
    stream_gate: Arc<Mutex<()>>,
    app_weak: slint::Weak<AppWindow>,
) {
    let _gate = stream_gate.lock().await;
    let state_handle = state.clone();
    let (session, request_id, pcm, elapsed, should_finish) = {
        let guard = state.lock().await;
        let Some(recorder) = guard.recorder.as_ref() else {
            return;
        };
        let elapsed = guard
            .recording_started
            .map(|started| started.elapsed())
            .unwrap_or_default();
        let pcm = match recorder.drain_pcm16(MAX_AUDIO_CHUNK_BYTES) {
            Ok(pcm) => pcm,
            Err(error) => {
                drop(guard);
                abort_recording(state_handle, app_weak, error).await;
                return;
            }
        };
        (
            guard.session.clone(),
            guard.recording_request_id.clone(),
            pcm,
            elapsed,
            elapsed >= Duration::from_secs(MAX_RECORDING_SECONDS),
        )
    };
    let (Some(session), Some(request_id)) = (session, request_id) else {
        return;
    };
    if !pcm.is_empty() {
        if let Err(error) = session.send_audio_chunk(request_id.clone(), pcm).await {
            abort_recording(state, app_weak, error.message).await;
            return;
        }
    }
    if let Some(app) = app_weak.upgrade() {
        let seconds = elapsed.as_secs().min(MAX_RECORDING_SECONDS);
        app.set_recording_time(format!("{:02}:{:02} / 01:00", seconds / 60, seconds % 60).into());
    }
    if should_finish {
        drop(_gate);
        finalize_recording(state, stream_gate, app_weak).await;
    }
}

async fn finalize_recording(
    state: Arc<Mutex<AppState>>,
    stream_gate: Arc<Mutex<()>>,
    app_weak: slint::Weak<AppWindow>,
) {
    let _gate = stream_gate.lock().await;
    let (session, request_id, recorder) = {
        let mut state = state.lock().await;
        (
            state.session.clone(),
            state.recording_request_id.take(),
            state.recorder.take(),
        )
    };
    let (Some(session), Some(request_id), Some(recorder)) = (session, request_id, recorder) else {
        return;
    };
    if let Some(app) = app_weak.upgrade() {
        app.set_voice_recording(false);
        feedback(&app);
        app.set_is_busy(true);
        app.set_remote_status("正在完成语音上传".into());
    }
    let remaining = match recorder.finish_pcm_chunks(MAX_AUDIO_CHUNK_BYTES) {
        Ok(chunks) => chunks,
        Err(error) => {
            let _ = session.cancel(request_id).await;
            if let Some(app) = app_weak.upgrade() {
                reset_request_ui(&app);
                app.set_is_busy(false);
                app.set_remote_status(slint::format!("录音失败: {}", error));
            }
            return;
        }
    };
    for pcm in remaining {
        if state.lock().await.pending_request_id.as_deref() != Some(request_id.as_str()) {
            return;
        }
        if let Err(error) = session.send_audio_chunk(request_id.clone(), pcm).await {
            if state.lock().await.pending_request_id.as_deref() != Some(request_id.as_str()) {
                return;
            }
            let _ = session.cancel(request_id.clone()).await;
            if let Some(app) = app_weak.upgrade() {
                reset_request_ui(&app);
                app.set_is_busy(false);
                app.set_remote_status(slint::format!("语音发送失败: {}", error.message));
            }
            return;
        }
    }
    if let Some(app) = app_weak.upgrade() {
        app.set_remote_status("桌面端正在处理语音".into());
    }
    let result = session.finish_audio(request_id.clone()).await;
    if state.lock().await.pending_request_id.as_deref() != Some(request_id.as_str()) {
        return;
    }
    match result {
        Ok(preview) => publish_preview(&state, &app_weak, preview).await,
        Err(error) if error.code == ale_core::remote::error_code::CANCELLED => {}
        Err(error) => {
            state.lock().await.pending_request_id = None;
            if let Some(app) = app_weak.upgrade() {
                reset_request_ui(&app);
                app.set_remote_status(slint::format!("远程请求失败: {}", error.message));
            }
        }
    }
    if let Some(app) = app_weak.upgrade() {
        app.set_is_busy(false);
    }
}

async fn publish_preview(
    state: &Arc<Mutex<AppState>>,
    app_weak: &slint::Weak<AppWindow>,
    preview: CommandPreview,
) {
    let mut state = state.lock().await;
    if state.pending_request_id.as_deref() != Some(preview.request_id.as_str()) {
        return;
    }
    state.pending_request_id = preview.has_plan.then(|| preview.request_id.clone());
    drop(state);
    if let Some(app) = app_weak.upgrade() {
        app.set_confirmation_title("确认执行".into());
        app.set_confirmation_confirm_label("确认执行".into());
        app.set_confirmation_cancel_label("取消".into());
        app.set_ai_response(preview.response_text.into());
        app.set_action_steps(preview.action_steps.join("\n").into());
        app.set_confirmation_text(preview.confirmation_text.into());
        app.set_show_confirmation(preview.has_plan);
        app.set_remote_status("桌面端已返回结果".into());
    }
}

async fn abort_recording(
    state: Arc<Mutex<AppState>>,
    app_weak: slint::Weak<AppWindow>,
    message: String,
) {
    let (session, request_id, recorder) = {
        let mut state = state.lock().await;
        let request_id = state.recording_request_id.take();
        state.pending_request_id = None;
        state.recording_started = None;
        (state.session.clone(), request_id, state.recorder.take())
    };
    drop(recorder);
    if let (Some(session), Some(request_id)) = (session, request_id) {
        let _ = session.cancel(request_id).await;
    }
    if let Some(app) = app_weak.upgrade() {
        reset_request_ui(&app);
        app.set_is_busy(false);
        app.set_remote_status(slint::format!("录音已停止: {}", message));
    }
}

fn wire_confirmation(app: &AppWindow, state: Arc<Mutex<AppState>>) {
    let confirm_state = state.clone();
    let app_weak = app.as_weak();
    app.on_confirm_action(move || {
        finish_confirmation(confirm_state.clone(), app_weak.clone(), true);
    });
    let app_weak = app.as_weak();
    app.on_cancel_action(move || {
        finish_confirmation(state.clone(), app_weak.clone(), false);
    });
}

fn finish_confirmation(
    state: Arc<Mutex<AppState>>,
    app_weak: slint::Weak<AppWindow>,
    approved: bool,
) {
    spawn_local_task(async move {
        let (session, request_id, decision, generation) = {
            let mut state = state.lock().await;
            (
                state.session.clone(),
                state.pending_request_id.clone(),
                state.pending_decision.take(),
                state.session_generation,
            )
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        app.set_is_busy(true);
        app.set_show_confirmation(false);
        if let (Some(session), Some(decision)) = (session.clone(), decision) {
            let result = session
                .respond_to_decision(decision.request_id, decision.decision_id, approved)
                .await;
            if state.lock().await.session_generation != generation {
                return;
            }
            match result {
                Ok(()) => app.set_remote_status(if approved {
                    "已同意，桌面端继续处理".into()
                } else {
                    "已拒绝".into()
                }),
                Err(error) => {
                    app.set_remote_status(slint::format!("发送决定失败: {}", error.message))
                }
            }
            app.set_is_busy(false);
            return;
        }
        match (session, request_id) {
            (Some(session), Some(request_id)) => {
                let result = session.confirm(request_id.clone(), approved).await;
                {
                    let mut current = state.lock().await;
                    if current.session_generation != generation
                        || current.pending_request_id.as_deref() != Some(request_id.as_str())
                    {
                        return;
                    }
                    current.pending_request_id = None;
                }
                match result {
                    Ok(status) => {
                        app.set_ai_response(status.message.into());
                        app.set_remote_status(if approved {
                            "桌面端执行完成".into()
                        } else {
                            "操作已拒绝".into()
                        });
                    }
                    Err(error) => {
                        if error.code == ale_core::remote::error_code::CONFIRM_TIMEOUT {
                            app.set_remote_status(
                                "执行结果未知，请先检查桌面端，切勿重复确认".into(),
                            );
                        } else {
                            app.set_remote_status(slint::format!(
                                "远程操作失败: {}",
                                error.message
                            ));
                        }
                    }
                }
            }
            _ => app.set_remote_status("没有待确认的操作".into()),
        }
        app.set_is_busy(false);
    });
}

fn wire_disconnect(
    app: &AppWindow,
    state: Arc<Mutex<AppState>>,
    qr_scan_cancel: Arc<std::sync::Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>>,
) {
    let app_weak = app.as_weak();
    app.on_disconnect_remote(move || {
        if let Some(app) = app_weak.upgrade() {
            app.set_remote_connected(false);
            app.set_input_pending(false);
            app.set_is_busy(true);
        }
        if let Ok(mut slot) = qr_scan_cancel.lock() {
            if let Some(cancel) = slot.take() {
                cancel.store(true, Ordering::Release);
            }
        }
        let state = state.clone();
        let app_weak = app_weak.clone();
        spawn_local_task(async move {
            let session = {
                let mut state = state.lock().await;
                state.session_generation = state.session_generation.wrapping_add(1);
                state.clear_request();
                state.session.take()
            };
            if let Some(session) = session {
                session.shutdown().await;
            }
            android::stop_tts();
            if let Some(app) = app_weak.upgrade() {
                reset_request_ui(&app);
                app.set_remote_connected(false);
                app.set_remote_scanning(false);
                app.set_is_busy(false);
                app.set_remote_status("已断开；请重新扫描桌面端 v3 二维码".into());
            }
        });
    });
}

fn reset_request_ui(app: &AppWindow) {
    app.set_input_pending(false);
    app.set_confirm_disconnect(false);
    app.set_voice_recording(false);
    app.set_recording_time("00:00 / 01:00".into());
    app.set_show_confirmation(false);
    app.set_confirmation_title("确认执行".into());
    app.set_confirmation_confirm_label("确认执行".into());
    app.set_confirmation_cancel_label("取消".into());
}

fn spawn_local_task(future: impl Future<Output = ()> + 'static) {
    if let Err(error) = slint::spawn_local(future) {
        tracing::warn!("Failed to spawn UI task: {}", error);
    }
}
