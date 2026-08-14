#![cfg(any(target_os = "android", target_os = "ios"))]

pub mod audio;
#[cfg(target_os = "ios")]
mod audit;
#[cfg(target_os = "ios")]
mod conversation;
pub mod file_picker;
pub mod tts_player;

#[cfg(target_os = "android")]
mod android;

#[cfg(target_os = "ios")]
mod ios;

#[cfg(target_os = "ios")]
pub mod camera_ios;

#[cfg(target_os = "ios")]
pub mod automation_ios;

#[cfg(target_os = "ios")]
pub mod tts_player_ios;

mod platform;
#[cfg(target_os = "android")]
mod remote_crypto;

#[cfg(target_os = "android")]
mod remote_client;

#[cfg(target_os = "android")]
mod qr_scanner;

#[cfg(target_os = "ios")]
use ale_core::actions::ActionPlan;
#[cfg(target_os = "ios")]
use ale_core::config::AppConfig;
#[cfg(target_os = "android")]
use ale_core::remote::CommandInput;
#[cfg(target_os = "ios")]
use ale_core::vad::{VadConfig, VadState, VoiceActivityDetector};
#[cfg(target_os = "ios")]
use ale_core::{AleEngine, AleEngineFactory};
#[cfg(target_os = "ios")]
use conversation::handle_question_response;
#[cfg(target_os = "ios")]
use platform::PlatformService;
use std::future::Future;
use std::sync::Arc;
#[cfg(target_os = "ios")]
use std::time::Instant;
use tokio::sync::Mutex;

#[cfg(target_os = "android")]
use base64::Engine;

slint::include_modules!();

pub struct AppState {
    #[cfg(target_os = "ios")]
    engine: Option<Arc<Mutex<AleEngine>>>,
    recorder: Option<audio::Recorder>,
    #[cfg(target_os = "ios")]
    recording_started: Option<Instant>,
    #[cfg(target_os = "ios")]
    vad_sample_offset: usize,
    #[cfg(target_os = "ios")]
    auto_speak: bool,
    #[cfg(target_os = "ios")]
    vad: VoiceActivityDetector,
    #[cfg(target_os = "ios")]
    vad_active: bool,
    #[cfg(target_os = "ios")]
    vad_frame_count: u64,
    #[cfg(target_os = "ios")]
    listening_enabled: bool,
    #[cfg(target_os = "ios")]
    platform: Option<Box<dyn PlatformService>>,
    #[cfg(target_os = "ios")]
    pending_plan: Option<ActionPlan>,
    #[cfg(target_os = "android")]
    pending_remote_request_id: Option<String>,
    #[cfg(target_os = "android")]
    remote_client: Option<remote_client::RemoteClient>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "ios")]
            engine: None,
            recorder: None,
            #[cfg(target_os = "ios")]
            recording_started: None,
            #[cfg(target_os = "ios")]
            vad_sample_offset: 0,
            #[cfg(target_os = "ios")]
            auto_speak: true,
            #[cfg(target_os = "ios")]
            vad: VoiceActivityDetector::with_default_config(),
            #[cfg(target_os = "ios")]
            vad_active: false,
            #[cfg(target_os = "ios")]
            vad_frame_count: 0,
            #[cfg(target_os = "ios")]
            listening_enabled: true,
            #[cfg(target_os = "ios")]
            platform: None,
            #[cfg(target_os = "ios")]
            pending_plan: None,
            #[cfg(target_os = "android")]
            pending_remote_request_id: None,
            #[cfg(target_os = "android")]
            remote_client: None,
        }
    }
}

pub fn setup_app(app: &AppWindow) {
    #[cfg(target_os = "android")]
    setup_android_app(app);
    #[cfg(target_os = "ios")]
    setup_local_app(app);
}

#[cfg(target_os = "ios")]
fn setup_local_app(app: &AppWindow) {
    let state = Arc::new(Mutex::new(AppState::new()));
    let app_weak = app.as_weak();
    #[cfg(target_os = "android")]
    let qr_scan_cancel = Arc::new(std::sync::Mutex::new(
        None::<Arc<std::sync::atomic::AtomicBool>>,
    ));

    // Initialize engine + start monitoring
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        spawn_local_task(async move {
            let result = create_engine().await;
            let mut st = state.lock().await;
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            match result {
                Ok((engine, config)) => {
                    apply_config_to_app(&app, &config);
                    let config_path = ale_core::config::ConfigFactory::create_default()
                        .config_path()
                        .to_string_lossy()
                        .to_string();
                    app.set_config_path(config_path.into());

                    st.engine = Some(engine);
                    // 应用弱语音模式 VAD 配置
                    if config.asr.weak_voice_mode {
                        let weak_vad = VadConfig::weak_voice();
                        st.vad = VoiceActivityDetector::new(weak_vad);
                        tracing::info!("Weak voice VAD mode enabled");
                    }
                    app.set_engine_ready(true);
                    app.set_status_text("就绪".into());
                    app.set_status_type("ready".into());

                    // 创建平台服务。Android 目前只作为局域网指令入口骨架。
                    let platform = platform::create_platform();
                    let capabilities = platform.capabilities();
                    app.set_capability_text(
                        format!(
                            "{}{}{}",
                            if capabilities.local_microphone {
                                "麦克风 + "
                            } else {
                                ""
                            },
                            if capabilities.image_capture {
                                "视觉"
                            } else {
                                "无本机视觉"
                            },
                            if capabilities.automation {
                                " + 自动化"
                            } else {
                                ""
                            }
                        )
                        .into(),
                    );
                    st.platform = Some(platform);

                    // Auto-start continuous listening
                    start_continuous_listening(&mut st, &app);
                }
                Err(error) => {
                    app.set_status_text(slint::format!("初始化失败: {}", error));
                    app.set_status_type("error".into());
                }
            }
        });
    }

    // VAD timer — checks for speech end every 100ms
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        let vad_timer = slint::Timer::default();
        vad_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(100),
            move || {
                let state = state.clone();
                let app_weak = app_weak.clone();
                spawn_local_task(async move {
                    let mut st = state.lock().await;
                    if !st.vad_active || st.recorder.is_none() {
                        return;
                    }

                    let mut vad_sample_offset = st.vad_sample_offset;
                    let samples = if let Some(ref recorder) = st.recorder {
                        recorder.samples_since(&mut vad_sample_offset)
                    } else {
                        return;
                    };
                    st.vad_sample_offset = vad_sample_offset;

                    if samples.is_empty() {
                        return;
                    }

                    let pcm = ale_core::vad::pcm16_bytes_to_f32(&samples);
                    let mut speech_ended = false;
                    for chunk in pcm.chunks(st.vad.config.frame_size) {
                        if chunk.len() == st.vad.config.frame_size {
                            let vad_state = st.vad.process_frame(chunk);
                            st.vad_frame_count += 1;
                            // 每 ~10 秒自动适应一次阈值（500帧 x 20ms = 10s）
                            if st.vad_frame_count % 500 == 0 {
                                st.vad.adapt_threshold();
                            }
                            if vad_state == VadState::SpeechEnded {
                                speech_ended = true;
                            }
                        }
                    }

                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    match st.vad.state() {
                        VadState::Speaking => app.set_vad_state("speaking".into()),
                        VadState::SpeechEnded => app.set_vad_state("speech_ended".into()),
                        VadState::Silent => app.set_vad_state("silent".into()),
                    }

                    if !speech_ended {
                        return;
                    }

                    // Speech ended — stop recording and process
                    #[cfg(target_os = "ios")]
                    let engine = st.engine.clone();
                    let recorder = st.recorder.take();
                    #[cfg(target_os = "ios")]
                    let auto_speak = st.auto_speak;
                    st.recording_started = None;
                    st.vad_active = false;
                    app.set_is_busy(true);
                    app.set_status_text("处理中...".into());
                    app.set_status_type("processing".into());

                    #[cfg(target_os = "ios")]
                    let engine = match engine {
                        Some(engine) => engine,
                        None => {
                            app.set_status_text("引擎未初始化".into());
                            app.set_status_type("error".into());
                            app.set_is_busy(false);
                            return;
                        }
                    };
                    let Some(recorder) = recorder else {
                        app.set_is_busy(false);
                        return;
                    };

                    let audio = match recorder.into_wav_bytes() {
                        Ok(a) => a,
                        Err(e) => {
                            app.set_status_text(slint::format!("录音失败: {}", e));
                            app.set_status_type("error".into());
                            app.set_is_busy(false);
                            return;
                        }
                    };

                    // Desktop captures the active screen; Android currently sends text-only input.
                    #[cfg(target_os = "ios")]
                    let image_data: Option<Vec<u8>> =
                        st.platform.as_ref().and_then(|p| p.capture_image());

                    drop(st);

                    #[cfg(target_os = "android")]
                    {
                        app.set_transcription("语音已发送到桌面端".into());
                        let wav_base64 = base64::engine::general_purpose::STANDARD.encode(&audio);
                        handle_remote_command(&state, &app, CommandInput::AudioWav { wav_base64 })
                            .await;
                        app.set_is_busy(false);
                        let mut st = state.lock().await;
                        start_continuous_listening(&mut st, &app);
                    }
                    #[cfg(not(target_os = "android"))]
                    {
                        // Transcribe audio
                        let transcription = {
                            let eng = engine.lock().await;
                            eng.transcribe(&audio).await
                        };

                        let Some(app) = app_weak.upgrade() else {
                            return;
                        };

                        let question = match transcription {
                            Ok(ref text) => {
                                app.set_transcription(text.clone().into());
                                text.clone()
                            }
                            Err(ref e) => {
                                app.set_transcription(slint::format!("转写失败: {}", e));
                                app.set_is_busy(false);
                                app.set_status_text("就绪".into());
                                app.set_status_type("ready".into());
                                let mut st = state.lock().await;
                                start_continuous_listening(&mut st, &app);
                                return;
                            }
                        };

                        handle_question_response(
                            &state,
                            &app,
                            &app_weak,
                            engine.clone(),
                            question,
                            image_data,
                            auto_speak,
                        )
                        .await;

                        app.set_is_busy(false);

                        // Restart listening
                        let mut st = state.lock().await;
                        start_continuous_listening(&mut st, &app);
                    }
                });
            },
        );
    }

    // Text submitted
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        app.on_text_submitted(move |text| {
            let question: String = text.into();
            if question.trim().is_empty() || question.chars().count() > 2_000 {
                return;
            }
            let state = state.clone();
            let app_weak = app_weak.clone();
            spawn_local_task(async move {
                let st = state.lock().await;
                #[cfg(target_os = "ios")]
                let engine = st.engine.clone();
                #[cfg(target_os = "ios")]
                let auto_speak = st.auto_speak;
                drop(st);

                #[cfg(target_os = "ios")]
                let Some(engine) = engine
                else {
                    return;
                };

                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                app.set_transcription(question.clone().into());
                app.set_is_busy(true);
                app.set_status_text("分析中...".into());
                app.set_status_type("processing".into());

                // Get screen image
                #[cfg(target_os = "ios")]
                let image_data = {
                    let st = state.lock().await;
                    st.platform.as_ref().and_then(|p| p.capture_image())
                };

                #[cfg(target_os = "android")]
                {
                    handle_remote_command(&state, &app, CommandInput::Text { text: question })
                        .await;
                }
                #[cfg(not(target_os = "android"))]
                {
                    handle_question_response(
                        &state,
                        &app,
                        &app_weak,
                        engine.clone(),
                        question,
                        image_data,
                        auto_speak,
                    )
                    .await;
                }

                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                app.set_is_busy(false);
            });
        });
    }

    // Confirm action
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        app.on_confirm_action(move || {
            let state = state.clone();
            let app_weak = app_weak.clone();
            spawn_local_task(async move {
                let mut st = state.lock().await;
                #[cfg(target_os = "android")]
                {
                    let client = st.remote_client.clone();
                    let request_id = st.pending_remote_request_id.take();
                    drop(st);
                    match (client, request_id) {
                        (Some(client), Some(request_id)) => {
                            match client.confirm(request_id, true).await {
                                Ok(status) => {
                                    let Some(app) = app_weak.upgrade() else {
                                        return;
                                    };
                                    app.set_show_confirmation(false);
                                    app.set_status_text(status.message.into());
                                    app.set_status_type("ready".into());
                                }
                                Err(error) => {
                                    let Some(app) = app_weak.upgrade() else {
                                        return;
                                    };
                                    app.set_show_confirmation(false);
                                    app.set_status_text(slint::format!("远程执行失败: {}", error));
                                    app.set_status_type("error".into());
                                }
                            }
                        }
                        _ => {
                            let Some(app) = app_weak.upgrade() else {
                                return;
                            };
                            app.set_show_confirmation(false);
                            app.set_status_text("未连接桌面端或没有待确认请求".into());
                            app.set_status_type("error".into());
                        }
                    }
                }
                #[cfg(not(target_os = "android"))]
                {
                    if let Some(plan) = st.pending_plan.take() {
                        // 统一的平台执行 — 不再需要 #[cfg] 分支
                        if let Some(ref platform) = st.platform {
                            if !platform.is_automation_ready() {
                                let Some(app) = app_weak.upgrade() else {
                                    return;
                                };
                                app.set_show_confirmation(false);
                                app.set_status_text("自动化引擎不可用".into());
                                app.set_status_type("error".into());
                            } else {
                                audit::record("approved", "local", &plan, None);
                                match platform.execute_plan(&plan, true) {
                                    Ok(result) => {
                                        audit::record("completed", "local", &plan, None);
                                        let Some(app) = app_weak.upgrade() else {
                                            return;
                                        };
                                        app.set_show_confirmation(false);
                                        app.set_status_text(slint::format!(
                                            "执行完成: {} 步",
                                            result.actions_executed
                                        ));
                                    }
                                    Err(e) => {
                                        audit::record(
                                            "failed",
                                            "local",
                                            &plan,
                                            Some(&e.to_string()),
                                        );
                                        let Some(app) = app_weak.upgrade() else {
                                            return;
                                        };
                                        app.set_show_confirmation(false);
                                        app.set_status_text(slint::format!("执行失败: {}", e));
                                        app.set_status_type("error".into());
                                    }
                                }
                            }
                        } else {
                            let Some(app) = app_weak.upgrade() else {
                                return;
                            };
                            app.set_show_confirmation(false);
                            app.set_status_text("平台服务未初始化".into());
                            app.set_status_type("error".into());
                        }
                    }
                } // end #[cfg(not(target_os = "android"))]
            });
        });
    }

    // Cancel action
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        app.on_cancel_action(move || {
            let state = state.clone();
            let app_weak = app_weak.clone();
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            spawn_local_task(async move {
                #[cfg(target_os = "android")]
                {
                    let (client, request_id) = {
                        let mut st = state.lock().await;
                        (
                            st.remote_client.clone(),
                            st.pending_remote_request_id.take(),
                        )
                    };
                    if let (Some(client), Some(request_id)) = (client, request_id) {
                        if let Err(error) = client.confirm(request_id, false).await {
                            let Some(app) = app_weak.upgrade() else {
                                return;
                            };
                            app.set_status_text(slint::format!("远程取消失败: {}", error));
                            app.set_status_type("error".into());
                        }
                    }
                }
                #[cfg(target_os = "ios")]
                if let Some(plan) = state.lock().await.pending_plan.take() {
                    audit::record("cancelled", "local", &plan, None);
                }
            });
            app.set_show_confirmation(false);
            app.set_confirmation_text("".into());
            app.set_action_steps("".into());
        });
    }

    // Pause or resume local microphone monitoring.
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        app.on_toggle_listening(move || {
            let state = state.clone();
            let app_weak = app_weak.clone();
            spawn_local_task(async move {
                let mut st = state.lock().await;
                st.listening_enabled = !st.listening_enabled;
                if !st.listening_enabled {
                    st.recorder = None;
                    st.vad.reset();
                    st.vad_active = false;
                }
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                app.set_listening_enabled(st.listening_enabled);
                if st.listening_enabled {
                    start_continuous_listening(&mut st, &app);
                } else {
                    app.set_status_text("监听已暂停".into());
                    app.set_status_type("paused".into());
                }
            });
        });
    }

    // Open settings
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        app.on_open_settings(move || {
            let state = state.clone();
            let app_weak = app_weak.clone();
            spawn_local_task(async move {
                let st = state.lock().await;
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                if let Some(ref engine) = st.engine {
                    let eng = engine.lock().await;
                    apply_config_to_app(&app, eng.config());
                }
                app.set_show_settings(true);
            });
        });
    }

    // Close settings
    {
        #[cfg(target_os = "android")]
        let qr_scan_cancel = qr_scan_cancel.clone();
        let app_weak = app_weak.clone();
        app.on_close_settings(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            #[cfg(target_os = "android")]
            if let Ok(mut slot) = qr_scan_cancel.lock() {
                if let Some(cancel) = slot.take() {
                    cancel.store(true, std::sync::atomic::Ordering::Release);
                }
            }
            app.set_show_settings(false);
        });
    }

    // Settings field callbacks
    {
        let app_weak = app_weak.clone();
        app.on_provider_changed(move |text| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_provider(text);
        });
    }
    {
        let app_weak = app_weak.clone();
        app.on_api_key_changed(move |text| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_api_key(text);
        });
    }
    {
        let app_weak = app_weak.clone();
        app.on_api_url_changed(move |text| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_api_url(text);
        });
    }
    {
        let app_weak = app_weak.clone();
        app.on_model_changed(move |text| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_model(text);
        });
    }
    {
        let app_weak = app_weak.clone();
        app.on_max_tokens_changed(move |text| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_max_tokens_str(text);
        });
    }
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        app.on_auto_speak_changed(move |value| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_auto_speak(value);
            let state = state.clone();
            spawn_local_task(async move {
                state.lock().await.auto_speak = value;
            });
        });
    }
    {
        let app_weak = app_weak.clone();
        app.on_toggle_api_key_visible(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_show_api_key(!app.get_show_api_key());
        });
    }
    {
        #[cfg(target_os = "android")]
        let state = state.clone();
        #[cfg(target_os = "android")]
        let qr_scan_cancel = qr_scan_cancel.clone();
        #[cfg(target_os = "android")]
        let app_weak = app_weak.clone();
        app.on_scan_remote(move || {
            #[cfg(target_os = "android")]
            {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                match android::ensure_camera_permission() {
                    Ok(true) => {}
                    Ok(false) => {
                        app.set_remote_status("请允许相机权限，然后再次点击扫描二维码".into());
                        return;
                    }
                    Err(error) => {
                        app.set_remote_status(slint::format!("相机权限错误: {}", error));
                        return;
                    }
                }
                app.set_is_busy(true);
                app.set_remote_connected(false);
                app.set_remote_scanning(true);
                app.set_remote_status("正在扫描桌面端二维码...".into());

                let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
                if let Ok(mut slot) = qr_scan_cancel.lock() {
                    if let Some(previous) = slot.replace(cancelled.clone()) {
                        previous.store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                let state = state.clone();
                let app_weak = app_weak.clone();
                let qr_scan_cancel = qr_scan_cancel.clone();
                std::thread::spawn(move || {
                    let preview_app = app_weak.clone();
                    let scan_token = cancelled.clone();
                    let scan_result =
                        qr_scanner::scan_pairing_info(cancelled, move |gray, width, height| {
                            let _ = preview_app.upgrade_in_event_loop(move |app| {
                                let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(
                                    width as u32,
                                    height as u32,
                                );
                                for (pixel, value) in pixels.make_mut_slice().iter_mut().zip(gray) {
                                    *pixel = slint::Rgb8Pixel::new(value, value, value);
                                }
                                app.set_remote_scan_preview(slint::Image::from_rgb8(pixels));
                            });
                        });
                    if let Ok(mut slot) = qr_scan_cancel.lock() {
                        if slot
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &scan_token))
                        {
                            slot.take();
                        }
                    }
                    let _ = app_weak.upgrade_in_event_loop(move |app| match scan_result {
                        Ok(pairing) => {
                            app.set_remote_scanning(false);
                            app.set_remote_status("二维码有效，正在建立加密连接...".into());
                            let client = remote_client::RemoteClient::from_pairing(&pairing);
                            let state = state.clone();
                            spawn_local_task(async move {
                                match client.test().await {
                                    Ok(name) => {
                                        state.lock().await.remote_client = Some(client);
                                        app.set_remote_status(slint::format!(
                                            "已加密连接: {}",
                                            name
                                        ));
                                        app.set_remote_connected(true);
                                    }
                                    Err(error) => {
                                        app.set_remote_status(slint::format!(
                                            "连接失败: {}",
                                            error
                                        ));
                                        app.set_remote_connected(false);
                                    }
                                }
                                app.set_is_busy(false);
                            });
                        }
                        Err(error) => {
                            app.set_remote_scanning(false);
                            app.set_remote_status(slint::format!("扫描失败: {}", error));
                            app.set_remote_connected(false);
                            app.set_is_busy(false);
                        }
                    });
                });
            }
        });
    }
    {
        #[cfg(target_os = "android")]
        let qr_scan_cancel = qr_scan_cancel.clone();
        #[cfg(target_os = "android")]
        let app_weak = app_weak.clone();
        app.on_cancel_remote_scan(move || {
            #[cfg(target_os = "android")]
            {
                if let Ok(mut slot) = qr_scan_cancel.lock() {
                    if let Some(cancel) = slot.take() {
                        cancel.store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                if let Some(app) = app_weak.upgrade() {
                    app.set_remote_status("正在停止扫描...".into());
                }
            }
        });
    }
    {
        #[cfg(target_os = "android")]
        let state = state.clone();
        #[cfg(target_os = "android")]
        let qr_scan_cancel = qr_scan_cancel.clone();
        let app_weak = app_weak.clone();
        app.on_disconnect_remote(move || {
            #[cfg(target_os = "android")]
            let state = state.clone();
            #[cfg(target_os = "android")]
            let qr_scan_cancel = qr_scan_cancel.clone();
            let app_weak = app_weak.clone();
            spawn_local_task(async move {
                #[cfg(target_os = "android")]
                {
                    let mut st = state.lock().await;
                    st.remote_client = None;
                    st.pending_remote_request_id = None;
                }
                #[cfg(target_os = "android")]
                if let Ok(mut slot) = qr_scan_cancel.lock() {
                    if let Some(cancel) = slot.take() {
                        cancel.store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                app.set_remote_connected(false);
                app.set_remote_scanning(false);
                app.set_remote_status("未连接桌面端".into());
            });
        });
    }

    // Save settings
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        app.on_save_settings(move || {
            let state = state.clone();
            let app_weak = app_weak.clone();
            spawn_local_task(async move {
                let st = state.lock().await;
                let engine = st.engine.clone();
                drop(st);

                let Some(engine) = engine else { return };
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                let config = {
                    let engine = engine.lock().await;
                    config_from_app(&app, engine.config())
                };

                app.set_is_busy(true);

                let result = save_settings(engine, config).await;
                let mut st = state.lock().await;
                let Some(app) = app_weak.upgrade() else {
                    return;
                };

                match result {
                    Ok((new_engine, new_config)) => {
                        st.engine = Some(new_engine);
                        // 应用弱语音模式 VAD 配置
                        if new_config.asr.weak_voice_mode {
                            let weak_vad = VadConfig::weak_voice();
                            st.vad = VoiceActivityDetector::new(weak_vad);
                        } else {
                            st.vad = VoiceActivityDetector::with_default_config();
                        }
                        // 重新创建平台服务。
                        let platform = platform::create_platform();
                        let capabilities = platform.capabilities();
                        app.set_capability_text(
                            format!(
                                "{}{}{}",
                                if capabilities.local_microphone {
                                    "麦克风 + "
                                } else {
                                    ""
                                },
                                if capabilities.image_capture {
                                    "视觉"
                                } else {
                                    "无本机视觉"
                                },
                                if capabilities.automation {
                                    " + 自动化"
                                } else {
                                    ""
                                }
                            )
                            .into(),
                        );
                        st.platform = Some(platform);
                        apply_config_to_app(&app, &new_config);
                        app.set_status_text("就绪".into());
                        app.set_status_type("ready".into());
                        app.set_show_settings(false);
                    }
                    Err(error) => {
                        app.set_status_text(slint::format!("保存失败: {}", error));
                        app.set_status_type("error".into());
                    }
                }
                app.set_is_busy(false);
            });
        });
    }

    // Test connection
    {
        let state = state.clone();
        let app_weak = app_weak.clone();
        app.on_test_connection(move || {
            let state = state.clone();
            let app_weak = app_weak.clone();
            spawn_local_task(async move {
                let st = state.lock().await;
                let engine = st.engine.clone();
                drop(st);

                let Some(engine) = engine else { return };
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                app.set_is_busy(true);

                let result = test_connection(engine).await;
                let Some(app) = app_weak.upgrade() else {
                    return;
                };

                match result {
                    Ok(true) => {
                        app.set_status_text("连接成功".into());
                        app.set_status_type("ready".into());
                    }
                    Ok(false) => {
                        app.set_status_text("连接失败".into());
                        app.set_status_type("error".into());
                    }
                    Err(e) => {
                        app.set_status_text(slint::format!("测试失败: {}", e));
                        app.set_status_type("error".into());
                    }
                }
                app.set_is_busy(false);
            });
        });
    }
}

#[cfg(target_os = "android")]
fn setup_android_app(app: &AppWindow) {
    app.set_remote_status("扫描桌面端二维码以开始".into());

    let state = Arc::new(Mutex::new(AppState::new()));
    let qr_scan_cancel = Arc::new(std::sync::Mutex::new(
        None::<Arc<std::sync::atomic::AtomicBool>>,
    ));

    {
        let state = state.clone();
        let qr_scan_cancel = qr_scan_cancel.clone();
        let app_weak = app.as_weak();
        app.on_scan_remote(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            match android::ensure_camera_permission() {
                Ok(true) => {}
                Ok(false) => {
                    app.set_remote_status("允许相机权限后，再点一次扫描".into());
                    return;
                }
                Err(error) => {
                    app.set_remote_status(slint::format!("相机权限错误: {}", error));
                    return;
                }
            }

            app.set_is_busy(true);
            app.set_remote_connected(false);
            app.set_remote_scanning(true);
            app.set_remote_status("将电脑上的二维码放入框内".into());
            let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            if let Ok(mut slot) = qr_scan_cancel.lock() {
                if let Some(previous) = slot.replace(cancelled.clone()) {
                    previous.store(true, std::sync::atomic::Ordering::Release);
                }
            }

            let state = state.clone();
            let app_weak = app_weak.clone();
            let qr_scan_cancel = qr_scan_cancel.clone();
            std::thread::spawn(move || {
                let preview_app = app_weak.clone();
                let scan_token = cancelled.clone();
                let scan_result =
                    qr_scanner::scan_pairing_info(cancelled, move |gray, width, height| {
                        let _ = preview_app.upgrade_in_event_loop(move |app| {
                            let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(
                                width as u32,
                                height as u32,
                            );
                            for (pixel, value) in pixels.make_mut_slice().iter_mut().zip(gray) {
                                *pixel = slint::Rgb8Pixel::new(value, value, value);
                            }
                            app.set_remote_scan_preview(slint::Image::from_rgb8(pixels));
                        });
                    });
                let should_publish = qr_scan_cancel.lock().is_ok_and(|mut slot| {
                    if slot
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &scan_token))
                    {
                        slot.take();
                        true
                    } else {
                        false
                    }
                });
                if !should_publish {
                    return;
                }
                let _ = app_weak.upgrade_in_event_loop(move |app| match scan_result {
                    Ok(pairing) => {
                        app.set_remote_scanning(false);
                        app.set_remote_status("二维码有效，正在建立加密连接".into());
                        let client = remote_client::RemoteClient::from_pairing(&pairing);
                        let state = state.clone();
                        spawn_local_task(async move {
                            match client.test().await {
                                Ok(name) => {
                                    state.lock().await.remote_client = Some(client);
                                    app.set_remote_status(slint::format!("已连接: {}", name));
                                    app.set_remote_connected(true);
                                }
                                Err(error) => {
                                    app.set_remote_status(slint::format!("连接失败: {}", error));
                                    app.set_remote_connected(false);
                                }
                            }
                            app.set_is_busy(false);
                        });
                    }
                    Err(error) => {
                        app.set_remote_scanning(false);
                        app.set_remote_status(slint::format!("扫描失败: {}", error));
                        app.set_is_busy(false);
                    }
                });
            });
        });
    }

    {
        let qr_scan_cancel = qr_scan_cancel.clone();
        let app_weak = app.as_weak();
        app.on_cancel_remote_scan(move || {
            if let Ok(mut slot) = qr_scan_cancel.lock() {
                if let Some(cancel) = slot.take() {
                    cancel.store(true, std::sync::atomic::Ordering::Release);
                }
            }
            if let Some(app) = app_weak.upgrade() {
                app.set_remote_scanning(false);
                app.set_is_busy(false);
                app.set_remote_status("扫描已取消".into());
            }
        });
    }

    {
        let state = state.clone();
        let app_weak = app.as_weak();
        app.on_toggle_listening(move || {
            let state = state.clone();
            let app_weak = app_weak.clone();
            spawn_local_task(async move {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                if !app.get_remote_connected() || app.get_is_busy() {
                    return;
                }

                let mut st = state.lock().await;
                if st.recorder.is_none() {
                    match android::ensure_microphone_permission() {
                        Ok(true) => {}
                        Ok(false) => {
                            app.set_remote_status("允许麦克风权限后，再点一次说话".into());
                            return;
                        }
                        Err(error) => {
                            app.set_remote_status(slint::format!("麦克风权限错误: {}", error));
                            return;
                        }
                    }
                    match audio::Recorder::start() {
                        Ok(recorder) => {
                            st.recorder = Some(recorder);
                            app.set_voice_recording(true);
                            app.set_remote_status("正在录音，再点一次即可发送".into());
                        }
                        Err(error) => {
                            app.set_remote_status(slint::format!("无法开始录音: {}", error));
                        }
                    }
                    return;
                }

                let recorder = st.recorder.take();
                drop(st);
                app.set_voice_recording(false);
                app.set_is_busy(true);
                app.set_remote_status("正在发送语音".into());
                let Some(recorder) = recorder else {
                    app.set_is_busy(false);
                    return;
                };
                match recorder.into_wav_bytes() {
                    Ok(audio) => {
                        let wav_base64 = base64::engine::general_purpose::STANDARD.encode(audio);
                        handle_remote_command(&state, &app, CommandInput::AudioWav { wav_base64 })
                            .await;
                    }
                    Err(error) => {
                        app.set_ai_response(slint::format!("录音失败: {}", error));
                        app.set_remote_status("录音失败，请重试".into());
                    }
                }
                app.set_is_busy(false);
            });
        });
    }

    {
        let state = state.clone();
        let qr_scan_cancel = qr_scan_cancel.clone();
        let app_weak = app.as_weak();
        app.on_disconnect_remote(move || {
            let state = state.clone();
            let qr_scan_cancel = qr_scan_cancel.clone();
            let app_weak = app_weak.clone();
            spawn_local_task(async move {
                if let Ok(mut slot) = qr_scan_cancel.lock() {
                    if let Some(cancel) = slot.take() {
                        cancel.store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                let mut st = state.lock().await;
                st.remote_client = None;
                st.pending_remote_request_id = None;
                st.recorder = None;
                drop(st);
                if let Some(app) = app_weak.upgrade() {
                    app.set_remote_connected(false);
                    app.set_remote_scanning(false);
                    app.set_voice_recording(false);
                    app.set_show_confirmation(false);
                    app.set_ai_response("".into());
                    app.set_action_steps("".into());
                    app.set_remote_status("扫描桌面端二维码以重新连接".into());
                }
            });
        });
    }

    {
        let state = state.clone();
        let app_weak = app.as_weak();
        app.on_confirm_action(move || {
            finish_remote_confirmation(state.clone(), app_weak.clone(), true);
        });
    }

    {
        let state = state.clone();
        let app_weak = app.as_weak();
        app.on_cancel_action(move || {
            finish_remote_confirmation(state.clone(), app_weak.clone(), false);
        });
    }
}

#[cfg(target_os = "android")]
fn finish_remote_confirmation(
    state: Arc<Mutex<AppState>>,
    app_weak: slint::Weak<AppWindow>,
    approved: bool,
) {
    spawn_local_task(async move {
        let (client, request_id) = {
            let mut st = state.lock().await;
            (
                st.remote_client.clone(),
                st.pending_remote_request_id.take(),
            )
        };
        let Some(app) = app_weak.upgrade() else {
            return;
        };
        app.set_is_busy(true);
        match (client, request_id) {
            (Some(client), Some(request_id)) => match client.confirm(request_id, approved).await {
                Ok(status) => {
                    app.set_ai_response(status.message.into());
                    app.set_remote_status(if approved {
                        "桌面端执行完成".into()
                    } else {
                        "操作已取消".into()
                    });
                }
                Err(error) => {
                    app.set_ai_response(slint::format!("远程操作失败: {}", error));
                    app.set_remote_status("远程操作失败".into());
                }
            },
            _ => app.set_remote_status("没有待确认的操作".into()),
        }
        app.set_show_confirmation(false);
        app.set_is_busy(false);
    });
}

fn spawn_local_task(future: impl Future<Output = ()> + 'static) {
    if let Err(error) = slint::spawn_local(future) {
        tracing::warn!("Failed to spawn UI task: {}", error);
    }
}

#[cfg(target_os = "ios")]
fn start_continuous_listening(st: &mut AppState, app: &AppWindow) {
    if !st.listening_enabled || st.recorder.is_some() {
        return;
    }
    match audio::Recorder::start() {
        Ok(recorder) => {
            st.recorder = Some(recorder);
            st.recording_started = Some(Instant::now());
            st.vad_sample_offset = 0;
            st.vad.reset();
            st.vad_frame_count = 0;
            st.vad_active = true;
            app.set_vad_state("silent".into());
            app.set_listening_enabled(true);
            app.set_status_text("监听中".into());
            app.set_status_type("listening".into());
        }
        Err(e) => {
            app.set_status_text(slint::format!("麦克风启动失败: {}", e));
            app.set_status_type("error".into());
        }
    }
}

#[cfg(target_os = "ios")]
fn apply_config_to_app(app: &AppWindow, config: &AppConfig) {
    app.set_provider(config.cloud_api.provider.clone().into());
    app.set_api_key(config.cloud_api.api_key.clone().into());
    app.set_api_url(config.cloud_api.api_url.clone().into());
    app.set_model(config.cloud_api.model.clone().into());
    app.set_max_tokens_str(config.cloud_api.max_tokens.to_string().into());
    app.set_auto_speak(config.ui.auto_speak);
}

#[cfg(target_os = "android")]
async fn handle_remote_command(state: &Arc<Mutex<AppState>>, app: &AppWindow, input: CommandInput) {
    let client = { state.lock().await.remote_client.clone() };
    let Some(client) = client else {
        app.set_ai_response("请先扫描桌面端二维码".into());
        app.set_remote_status("未连接桌面端".into());
        return;
    };

    match client.send_command(input).await {
        Ok(preview) => {
            app.set_ai_response(preview.response_text.into());
            app.set_action_steps(preview.action_steps.join("\n").into());
            if preview.has_plan {
                state.lock().await.pending_remote_request_id = Some(preview.request_id);
                app.set_confirmation_text(preview.confirmation_text.into());
                app.set_show_confirmation(true);
            } else {
                app.set_show_confirmation(false);
            }
            app.set_remote_status("桌面端已返回结果".into());
        }
        Err(error) => {
            app.set_ai_response(slint::format!("远程请求失败: {}", error));
            app.set_remote_status("远程请求失败，请检查两端网络".into());
        }
    }
}

#[cfg(target_os = "ios")]
fn config_from_app(app: &AppWindow, base: &AppConfig) -> AppConfig {
    let mut config = base.clone();
    config.cloud_api.provider = app.get_provider().to_string();
    config.cloud_api.api_key = app.get_api_key().to_string();
    config.cloud_api.api_url = app
        .get_api_url()
        .to_string()
        .trim_end_matches('/')
        .to_string();
    config.cloud_api.model = app.get_model().to_string();
    if let Ok(budget) = app.get_max_tokens_str().to_string().parse::<usize>() {
        config.cloud_api.max_tokens = budget;
    }
    config.ui.auto_speak = app.get_auto_speak();
    config
}

#[cfg(target_os = "ios")]
async fn create_engine() -> Result<(Arc<Mutex<AleEngine>>, AppConfig), String> {
    let engine = AleEngineFactory::create_default()
        .await
        .map_err(|error| error.to_string())?;
    let config = engine.config().clone();
    Ok((Arc::new(Mutex::new(engine)), config))
}

#[cfg(target_os = "ios")]
async fn save_settings(
    engine: Arc<Mutex<AleEngine>>,
    config: AppConfig,
) -> Result<(Arc<Mutex<AleEngine>>, AppConfig), String> {
    {
        let mut engine = engine.lock().await;
        engine
            .update_config(config)
            .map_err(|error| error.to_string())?;
    }
    create_engine().await
}

#[cfg(target_os = "ios")]
async fn test_connection(engine: Arc<Mutex<AleEngine>>) -> Result<bool, String> {
    let engine = engine.lock().await;
    ensure_api_key(engine.config())?;
    engine
        .test_cloud_api()
        .await
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "ios")]
fn ensure_api_key(config: &AppConfig) -> Result<(), String> {
    if config.cloud_api.api_key.trim().is_empty() {
        return Err("API key 未配置".to_string());
    }
    Ok(())
}
