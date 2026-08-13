#[allow(unused_variables)]
pub fn play_audio(audio_data: &[u8]) -> Result<(), String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        play_audio_desktop(audio_data)
    }
    #[cfg(target_os = "android")]
    {
        play_audio_android(audio_data)
    }
    #[cfg(target_os = "ios")]
    {
        crate::tts_player_ios::play_audio(audio_data)
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn play_audio_desktop(audio_data: &[u8]) -> Result<(), String> {
    use std::io::Cursor;

    let cursor = Cursor::new(audio_data.to_vec());
    let source = rodio::Decoder::new(cursor).map_err(|error| format!("解析音频失败: {error}"))?;
    let (_stream, handle) =
        rodio::OutputStream::try_default().map_err(|error| format!("打开音频输出失败: {error}"))?;
    let sink = rodio::Sink::try_new(&handle).map_err(|error| format!("创建播放器失败: {error}"))?;
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
}

#[cfg(target_os = "android")]
fn play_audio_android(audio_data: &[u8]) -> Result<(), String> {
    // On Android, we use the system's MediaPlayer via JNI
    // Write audio to a temp file and play it
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join("ale_tts_output.mp3");
    std::fs::write(&temp_file, audio_data)
        .map_err(|error| format!("写入临时音频文件失败: {error}"))?;

    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|error| format!("获取 JVM 失败: {error}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("附加线程失败: {error}"))?;

    // Create MediaPlayer
    let mp_class = env
        .find_class("android/media/MediaPlayer")
        .map_err(|error| format!("找不到 MediaPlayer: {error}"))?;

    let mp = env
        .new_object(mp_class, "()V", &[])
        .map_err(|error| format!("创建 MediaPlayer 失败: {error}"))?;

    let path = env
        .new_string(temp_file.to_str().unwrap_or("/tmp/ale_tts.mp3"))
        .map_err(|error| format!("创建路径字符串失败: {error}"))?;

    env.call_method(
        &mp,
        "setDataSource",
        "(Ljava/lang/String;)V",
        &[jni::objects::JValue::from(&jni::objects::JObject::from(
            path,
        ))],
    )
    .map_err(|error| format!("设置数据源失败: {error}"))?;

    env.call_method(&mp, "prepare", "()V", &[])
        .map_err(|error| format!("准备播放失败: {error}"))?;

    env.call_method(&mp, "start", "()V", &[])
        .map_err(|error| format!("开始播放失败: {error}"))?;

    // Clean up temp file after a delay
    let _ = std::fs::remove_file(&temp_file);

    Ok(())
}
