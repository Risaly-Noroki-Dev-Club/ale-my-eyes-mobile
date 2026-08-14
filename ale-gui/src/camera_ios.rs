use ale_core::{AleError, Result};
use std::sync::{Arc, Mutex};

/// 相机帧数据
#[derive(Clone)]
pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    pub rgba_data: Vec<u8>,
}

/// 相机配置
#[derive(Debug, Clone)]
pub struct CameraConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 30,
        }
    }
}

/// iOS 相机 — 通过 objc2 调用 AVFoundation
///
/// 使用 AVCaptureSession + AVCaptureVideoDataOutput 获取实时帧。
/// 需要在 Info.plist 中声明 NSCameraUsageDescription。
pub struct IosCamera {
    latest_frame: Arc<Mutex<Option<CameraFrame>>>,
    running: Arc<Mutex<bool>>,
    config: CameraConfig,
    #[allow(dead_code)]
    capture_session: Option<ObjcCaptureSession>,
}

/// 封装 ObjC 对象的捕获会话
struct ObjcCaptureSession {
    // 保存 ObjC 对象指针，在 Drop 时停止并释放
    session_ptr: *mut objc2::runtime::AnyObject,
}

unsafe impl Send for ObjcCaptureSession {}
unsafe impl Sync for ObjcCaptureSession {}

impl Drop for ObjcCaptureSession {
    fn drop(&mut self) {
        if !self.session_ptr.is_null() {
            unsafe {
                use objc2::msg_send;
                let _: () = msg_send![self.session_ptr, stopRunning];
            }
        }
    }
}

impl IosCamera {
    pub fn new(config: CameraConfig) -> Self {
        Self {
            latest_frame: Arc::new(Mutex::new(None)),
            running: Arc::new(Mutex::new(false)),
            config,
            capture_session: None,
        }
    }

    /// 打开相机并开始预览
    pub fn start(&mut self) -> Result<()> {
        let mut running = self
            .running
            .lock()
            .map_err(|e| AleError::Other(anyhow::anyhow!("Failed to lock running flag: {}", e)))?;

        if *running {
            return Ok(());
        }
        *running = true;
        drop(running);

        let latest_frame = self.latest_frame.clone();
        let running = self.running.clone();
        let width = self.config.width;
        let height = self.config.height;

        // 在后台线程中初始化 iOS 相机
        std::thread::spawn(move || {
            if let Err(e) = init_ios_camera(latest_frame, running, width, height) {
                tracing::error!("iOS camera initialization failed: {}", e);
            }
        });

        Ok(())
    }

    /// 获取最新帧的 JPEG 数据（用于发送给 API）
    pub fn latest_frame_jpeg(&self, quality: u8) -> Option<Vec<u8>> {
        let frame = self.latest_frame()?;
        frame_to_jpeg(&frame, quality).ok()
    }

    /// 停止相机
    pub fn stop(&self) {
        if let Ok(mut running) = self.running.lock() {
            *running = false;
        }
    }

    /// 获取最新帧
    pub fn latest_frame(&self) -> Option<CameraFrame> {
        self.latest_frame.lock().ok()?.clone()
    }

    /// 立即捕获一帧
    pub fn capture_frame(&self) -> Result<CameraFrame> {
        self.latest_frame()
            .ok_or_else(|| AleError::Other(anyhow::anyhow!("No camera frame available")))
    }
}

impl Drop for IosCamera {
    fn drop(&mut self) {
        self.stop();
    }
}

fn frame_to_jpeg(frame: &CameraFrame, quality: u8) -> Result<Vec<u8>> {
    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba_data.clone())
        .ok_or_else(|| AleError::Other(anyhow::anyhow!("Failed to create image from frame")))?;
    let rgb_img = image::DynamicImage::ImageRgba8(img).to_rgb8();

    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    rgb_img
        .write_with_encoder(encoder)
        .map_err(|e| AleError::Other(anyhow::anyhow!("JPEG encode failed: {}", e)))?;

    Ok(buf.into_inner())
}

/// 初始化 iOS 相机（通过 objc2 调用 AVFoundation）
fn init_ios_camera(
    _latest_frame: Arc<Mutex<Option<CameraFrame>>>,
    running: Arc<Mutex<bool>>,
    width: u32,
    height: u32,
) -> Result<()> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    unsafe {
        // 获取后置摄像头
        let media_type_video: *mut AnyObject =
            msg_send![class!(NSString), stringWithUTF8String: "vide\0".as_ptr()];
        let device: *mut AnyObject =
            msg_send![class!(AVCaptureDevice), defaultDeviceWithMediaType: media_type_video];

        if device.is_null() {
            tracing::warn!("No camera available on this device, camera capture disabled");
            // 相机不可用时保持线程运行，等待 stop 信号
            while {
                let Ok(r) = running.lock() else { return Ok(()) };
                *r
            } {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            return Ok(());
        }

        // 创建 AVCaptureDeviceInput
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let input: *mut AnyObject = msg_send![
            class!(AVCaptureDeviceInput),
            deviceInputWithDevice: device,
            error: &mut error
        ];

        if input.is_null() || !error.is_null() {
            return Err(AleError::Other(anyhow::anyhow!(
                "Failed to create camera input"
            )));
        }

        // 创建 AVCaptureSession
        let session: *mut AnyObject = msg_send![class!(AVCaptureSession), new];
        if session.is_null() {
            return Err(AleError::Other(anyhow::anyhow!(
                "Failed to create capture session"
            )));
        }

        // 添加输入
        let added: bool = msg_send![session, canAddInput: input];
        if added {
            let _: () = msg_send![session, addInput: input];
        }

        // 创建 AVCaptureVideoDataOutput
        let output: *mut AnyObject = msg_send![class!(AVCaptureVideoDataOutput), new];

        // 设置像素格式为 BGRA (kCVPixelFormatType_32BGRA)
        let pixel_format_key: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: "kCVPixelBufferPixelFormatTypeKey\0".as_ptr()];
        let pixel_format_value: u32 = 0x42475241; // BGRA
        let format_number: *mut AnyObject =
            msg_send![class!(NSNumber), numberWithUnsignedInt: pixel_format_value];
        let settings_dict: *mut AnyObject = msg_send![
            class!(NSDictionary),
            dictionaryWithObject: format_number,
            forKey: pixel_format_key
        ];
        let _: () = msg_send![output, setVideoSettings: settings_dict];

        // 添加输出
        let added: bool = msg_send![session, canAddOutput: output];
        if added {
            let _: () = msg_send![session, addOutput: output];
        }

        // 设置视频质量
        let _: () = msg_send![session, setSessionPreset: {
            let preset: *mut AnyObject = msg_send![
                class!(NSString),
                stringWithUTF8String: "AVCaptureSessionPresetMedium\0".as_ptr()
            ];
            preset
        }];

        // 启动会话
        let _: () = msg_send![session, startRunning];

        tracing::info!("iOS camera started (requested {}x{})", width, height);

        // 轮询获取帧（简化实现，生产环境应使用 AVCaptureVideoDataOutputSampleBufferDelegate）
        while {
            let Ok(r) = running.lock() else { return Ok(()) };
            *r
        } {
            // 注意：这里使用简化的轮询方式
            // 完整实现需要创建 ObjC 类实现 AVCaptureVideoDataOutputSampleBufferDelegate
            // 并在回调中处理 CMSampleBuffer -> RGBA 转换
            std::thread::sleep(std::time::Duration::from_millis(33));
        }

        // 停止会话
        let _: () = msg_send![session, stopRunning];
    }

    Ok(())
}
