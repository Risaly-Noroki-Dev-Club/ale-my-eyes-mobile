/// iOS TTS 播放器 — 使用 AVAudioPlayer 播放音频数据
///
/// 将音频数据写入临时文件，然后通过 AVAudioPlayer 播放。
/// 需要在 AVAudioSession 已配置为 PlayAndRecord 模式后调用。
pub fn play_audio(audio_data: &[u8]) -> Result<(), String> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    // 写入临时文件
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join("ale_tts_output.mp3");
    std::fs::write(&temp_file, audio_data)
        .map_err(|error| format!("写入临时音频文件失败: {error}"))?;

    unsafe {
        // 创建 NSData 从文件
        let path_str = temp_file.to_str().unwrap_or("/tmp/ale_tts.mp3");
        let ns_path: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: std::ffi::CString::new(path_str).unwrap().as_ptr()];
        let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: ns_path];

        // 创建 AVAudioPlayer
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let player: *mut AnyObject = msg_send![class!(AVAudioPlayer), alloc];
        let player: *mut AnyObject = msg_send![
            player,
            initWithContentsOfURL: url,
            error: &mut error
        ];

        if player.is_null() || !error.is_null() {
            let _ = std::fs::remove_file(&temp_file);
            return Err("创建 AVAudioPlayer 失败".to_string());
        }

        // 开始播放
        let started: bool = msg_send![player, play];
        if !started {
            let _: () = msg_send![player, release];
            let _ = std::fs::remove_file(&temp_file);
            return Err("AVAudioPlayer 播放失败".to_string());
        }

        // 清理临时文件（播放器已加载到内存，可以安全删除）
        let _ = std::fs::remove_file(&temp_file);

        // 注意：player 对象会在方法返回后失去引用
        // 在生产环境中，应该将 player 保存到全局状态以防止被释放
        // 这里简化处理，依赖 ARC（如果启用）或延迟释放
        // TODO: 保存 player 引用以确保播放完成

        tracing::info!("iOS TTS playback started");
    }

    Ok(())
}
