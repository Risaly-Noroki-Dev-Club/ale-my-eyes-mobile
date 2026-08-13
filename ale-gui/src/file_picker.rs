pub async fn pick_image() -> Result<(Vec<u8>, String), String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        pick_image_desktop().await
    }
    #[cfg(target_os = "android")]
    {
        pick_image_android().await
    }
    #[cfg(target_os = "ios")]
    {
        pick_image_ios().await
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn pick_image_desktop() -> Result<(Vec<u8>, String), String> {
    let file = rfd::AsyncFileDialog::new()
        .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
        .pick_file()
        .await
        .ok_or_else(|| "未选择图片".to_string())?;

    let path = file.path().to_path_buf();
    let bytes = tokio::fs::read(file.path())
        .await
        .map_err(|error| format!("读取图片失败: {error}"))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("未知文件")
        .to_string();

    Ok((bytes, file_name))
}

#[cfg(target_os = "android")]
async fn pick_image_android() -> Result<(Vec<u8>, String), String> {
    // On Android, we use a JNI-based approach to launch the system image picker
    // This is a simplified version - in production, you'd need to handle the
    // activity result callback through the Android activity lifecycle

    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|error| format!("获取 JVM 失败: {error}"))?;
    let activity = unsafe { jni::objects::JObject::from_raw(ctx.context().cast()) };
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| format!("附加线程失败: {error}"))?;

    // Create intent for image picking
    let intent_class = env
        .find_class("android/content/Intent")
        .map_err(|error| format!("找不到 Intent 类: {error}"))?;

    let action = env
        .get_static_field(&intent_class, "ACTION_GET_CONTENT", "Ljava/lang/String;")
        .map_err(|error| format!("获取 ACTION_GET_CONTENT 失败: {error}"))?;
    let action = action
        .l()
        .map_err(|error| format!("读取 action 失败: {error}"))?;

    let intent = env
        .new_object(
            &intent_class,
            "(Ljava/lang/String;)V",
            &[jni::objects::JValue::Object(&action)],
        )
        .map_err(|error| format!("创建 Intent 失败: {error}"))?;

    let mime_type = env
        .new_string("image/*")
        .map_err(|error| format!("创建字符串失败: {error}"))?;

    env.call_method(
        &intent,
        "setType",
        "(Ljava/lang/String;)Landroid/content/Intent;",
        &[jni::objects::JValue::from(&jni::objects::JObject::from(
            mime_type,
        ))],
    )
    .map_err(|error| format!("设置类型失败: {error}"))?;

    // Launch the picker with a request code
    env.call_method(
        &activity,
        "startActivityForResult",
        "(Landroid/content/Intent;I)V",
        &[
            jni::objects::JValue::Object(&intent),
            jni::objects::JValue::Int(1001),
        ],
    )
    .map_err(|error| format!("启动图片选择器失败: {error}"))?;

    // In a real implementation, we'd wait for the result via a channel
    // For now, return an error indicating the picker was launched
    Err("请在弹出的选择器中选择图片".to_string())
}

#[cfg(target_os = "ios")]
async fn pick_image_ios() -> Result<(Vec<u8>, String), String> {
    // iOS 图片选择器 — 通过 objc2 调用 UIImagePickerController
    // 注意：UIImagePickerController 必须在主线程上呈现
    // 这里使用简化实现，实际需要通过 dispatch_async 在主线程上执行

    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    unsafe {
        // 获取当前的 key window
        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        let windows: *mut AnyObject = msg_send![app, windows];
        let key_window: *mut AnyObject = msg_send![windows, objectAtIndex: 0i64];

        if key_window.is_null() {
            return Err("无法获取当前窗口".to_string());
        }

        // 获取 root view controller
        let root_vc: *mut AnyObject = msg_send![key_window, rootViewController];
        if root_vc.is_null() {
            return Err("无法获取 root view controller".to_string());
        }

        // 创建 UIImagePickerController
        let picker_class = class!(UIImagePickerController);
        let is_available: bool = msg_send![picker_class, isSourceTypeAvailable: 0i64]; // UIImagePickerControllerSourceTypePhotoLibrary
        if !is_available {
            return Err("图片选择器不可用".to_string());
        }

        let picker: *mut AnyObject = msg_send![picker_class, alloc];
        let picker: *mut AnyObject = msg_send![picker, init];
        let _: () = msg_send![picker, setSourceType: 0i64]; // Photo Library

        // 注意：需要设置 delegate 来接收选择结果
        // 这里简化处理，返回错误提示用户手动选择
        let completion: Option<&block2::Block<dyn Fn()>> = None;
        let _: () = msg_send![
            root_vc,
            presentViewController: picker,
            animated: true,
            completion: completion
        ];

        tracing::info!("iOS image picker presented");
    }

    // 简化实现：返回错误提示
    // 完整实现需要创建 UIImagePickerControllerDelegate 处理回调
    Err("请在弹出的选择器中选择图片（iOS）".to_string())
}
