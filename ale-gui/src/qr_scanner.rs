#![cfg(target_os = "android")]

use ale_core::remote::PairingInfo;
use ndk::media::image_reader::{AcquireResult, ImageFormat, ImageReader};
use ndk::native_window::NativeWindow;
use ndk_sys as ffi;
use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const FRAME_WIDTH: i32 = 1280;
const FRAME_HEIGHT: i32 = 720;
const SCAN_TIMEOUT: Duration = Duration::from_secs(45);

pub fn scan_pairing_info(
    cancelled: Arc<AtomicBool>,
    preview: impl Fn(Vec<u8>, usize, usize) + Send + Sync + 'static,
) -> Result<PairingInfo, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let found = Arc::new(AtomicBool::new(false));
    let decoder = Arc::new(Mutex::new(quircs::Quirc::default()));
    let preview = Arc::new(preview);
    let preview_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut reader = ImageReader::new(FRAME_WIDTH, FRAME_HEIGHT, ImageFormat::YUV_420_888, 3)
        .map_err(|error| format!("无法创建扫码画面: {error}"))?;
    reader
        .set_image_listener(Box::new({
            let found = found.clone();
            let cancelled = cancelled.clone();
            let decoder = decoder.clone();
            let preview = preview.clone();
            let preview_counter = preview_counter.clone();
            move |reader| {
                if found.load(Ordering::Acquire) || cancelled.load(Ordering::Acquire) {
                    return;
                }
                let Ok(AcquireResult::Image(image)) = reader.acquire_latest_image() else {
                    return;
                };
                let Ok(gray) = copy_luminance(&image) else {
                    return;
                };
                let width = image.width().unwrap_or_default() as usize;
                let height = image.height().unwrap_or_default() as usize;
                if width == 0 || height == 0 {
                    return;
                }
                if preview_counter
                    .fetch_add(1, Ordering::Relaxed)
                    .is_multiple_of(6)
                {
                    let preview_width = width / 2;
                    let preview_height = height / 2;
                    let mut preview_gray = Vec::with_capacity(preview_width * preview_height);
                    for y in 0..preview_height {
                        for x in 0..preview_width {
                            preview_gray.push(gray[(y * 2) * width + x * 2]);
                        }
                    }
                    preview(preview_gray, preview_width, preview_height);
                }
                let Ok(mut decoder) = decoder.try_lock() else {
                    return;
                };
                for code in decoder.identify(width, height, &gray) {
                    let Ok(code) = code else { continue };
                    let Ok(decoded) = code.decode() else { continue };
                    let Ok(uri) = std::str::from_utf8(&decoded.payload) else {
                        continue;
                    };
                    let Ok(pairing) = PairingInfo::from_uri(uri) else {
                        continue;
                    };
                    if !found.swap(true, Ordering::AcqRel) {
                        let _ = sender.try_send(pairing);
                    }
                    break;
                }
            }
        }))
        .map_err(|error| format!("无法读取扫码画面: {error}"))?;

    let camera = CameraSession::open(reader)?;
    let deadline = std::time::Instant::now() + SCAN_TIMEOUT;
    let result = loop {
        if cancelled.load(Ordering::Acquire) {
            break Err("扫描已取消".to_string());
        }
        if std::time::Instant::now() >= deadline {
            break Err("扫描超时，请让桌面二维码完整出现在镜头中".to_string());
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(pairing) => break Ok(pairing),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("扫码相机意外停止".to_string());
            }
        }
    };
    drop(camera);
    result
}

fn copy_luminance(image: &ndk::media::image_reader::Image) -> Result<Vec<u8>, String> {
    let width = image.width().map_err(|error| error.to_string())? as usize;
    let height = image.height().map_err(|error| error.to_string())? as usize;
    let row_stride = image
        .plane_row_stride(0)
        .map_err(|error| error.to_string())? as usize;
    let pixel_stride = image
        .plane_pixel_stride(0)
        .map_err(|error| error.to_string())? as usize;
    let plane = image.plane_data(0).map_err(|error| error.to_string())?;
    if width == 0 || height == 0 || pixel_stride == 0 {
        return Err("相机返回了空画面".to_string());
    }

    let mut gray = vec![0_u8; width * height];
    for y in 0..height {
        let source_start = y * row_stride;
        let source_end = source_start + (width - 1) * pixel_stride;
        if source_end >= plane.len() {
            return Err("相机画面尺寸无效".to_string());
        }
        for x in 0..width {
            gray[y * width + x] = plane[source_start + x * pixel_stride];
        }
    }
    Ok(gray)
}

struct CameraSession {
    manager: *mut ffi::ACameraManager,
    device: *mut ffi::ACameraDevice,
    session: *mut ffi::ACameraCaptureSession,
    request: *mut ffi::ACaptureRequest,
    target: *mut ffi::ACameraOutputTarget,
    output: *mut ffi::ACaptureSessionOutput,
    container: *mut ffi::ACaptureSessionOutputContainer,
    _window: NativeWindow,
    _reader: ImageReader,
}

impl CameraSession {
    fn open(reader: ImageReader) -> Result<Self, String> {
        let window = reader
            .window()
            .map_err(|error| format!("无法创建相机输出: {error}"))?;
        let mut camera = Self {
            manager: unsafe { ffi::ACameraManager_create() },
            device: ptr::null_mut(),
            session: ptr::null_mut(),
            request: ptr::null_mut(),
            target: ptr::null_mut(),
            output: ptr::null_mut(),
            container: ptr::null_mut(),
            _window: window,
            _reader: reader,
        };
        if camera.manager.is_null() {
            return Err("无法访问 Android 相机服务".to_string());
        }

        let camera_id = camera.back_camera_id()?;
        let mut device_callbacks = ffi::ACameraDevice_StateCallbacks {
            context: ptr::null_mut(),
            onDisconnected: Some(camera_disconnected),
            onError: Some(camera_error),
        };
        check("打开后置相机", unsafe {
            ffi::ACameraManager_openCamera(
                camera.manager,
                camera_id.as_ptr().cast(),
                &mut device_callbacks,
                &mut camera.device,
            )
        })?;
        check("创建相机请求", unsafe {
            ffi::ACameraDevice_createCaptureRequest(
                camera.device,
                ffi::ACameraDevice_request_template::TEMPLATE_PREVIEW,
                &mut camera.request,
            )
        })?;

        let window_ptr = camera._window.ptr().as_ptr();
        check("创建相机目标", unsafe {
            ffi::ACameraOutputTarget_create(window_ptr, &mut camera.target)
        })?;
        check("绑定相机目标", unsafe {
            ffi::ACaptureRequest_addTarget(camera.request, camera.target)
        })?;
        check("创建相机会话输出", unsafe {
            ffi::ACaptureSessionOutput_create(window_ptr, &mut camera.output)
        })?;
        check("创建相机会话", unsafe {
            ffi::ACaptureSessionOutputContainer_create(&mut camera.container)
        })?;
        check("绑定相机会话输出", unsafe {
            ffi::ACaptureSessionOutputContainer_add(camera.container, camera.output)
        })?;

        let session_callbacks = ffi::ACameraCaptureSession_stateCallbacks {
            context: ptr::null_mut(),
            onClosed: None,
            onReady: None,
            onActive: None,
        };
        check("启动相机会话", unsafe {
            ffi::ACameraDevice_createCaptureSession(
                camera.device,
                camera.container,
                &session_callbacks,
                &mut camera.session,
            )
        })?;

        let autofocus =
            ffi::acamera_metadata_enum_acamera_control_af_mode::ACAMERA_CONTROL_AF_MODE_CONTINUOUS_PICTURE
                .0 as u8;
        let _ = unsafe {
            ffi::ACaptureRequest_setEntry_u8(
                camera.request,
                ffi::acamera_metadata_tag::ACAMERA_CONTROL_AF_MODE.0,
                1,
                &autofocus,
            )
        };
        let mut request = camera.request;
        check("开始读取相机画面", unsafe {
            ffi::ACameraCaptureSession_setRepeatingRequest(
                camera.session,
                ptr::null_mut(),
                1,
                &mut request,
                ptr::null_mut(),
            )
        })?;
        Ok(camera)
    }

    fn back_camera_id(&self) -> Result<Vec<u8>, String> {
        let mut list = ptr::null_mut();
        check("读取相机列表", unsafe {
            ffi::ACameraManager_getCameraIdList(self.manager, &mut list)
        })?;
        if list.is_null() {
            return Err("设备没有可用相机".to_string());
        }

        let result = unsafe {
            let ids = std::slice::from_raw_parts((*list).cameraIds, (*list).numCameras as usize);
            let mut fallback = None;
            let mut selected = None;
            for &id in ids {
                if id.is_null() {
                    continue;
                }
                let bytes = CStr::from_ptr(id).to_bytes_with_nul().to_vec();
                fallback.get_or_insert_with(|| bytes.clone());
                let mut metadata = ptr::null_mut();
                if ffi::ACameraManager_getCameraCharacteristics(self.manager, id, &mut metadata)
                    != ffi::camera_status_t::ACAMERA_OK
                    || metadata.is_null()
                {
                    continue;
                }
                let mut entry = std::mem::zeroed::<ffi::ACameraMetadata_const_entry>();
                let status = ffi::ACameraMetadata_getConstEntry(
                    metadata,
                    ffi::acamera_metadata_tag::ACAMERA_LENS_FACING.0,
                    &mut entry,
                );
                if status == ffi::camera_status_t::ACAMERA_OK
                    && entry.count > 0
                    && !entry.data.u8_.is_null()
                    && *entry.data.u8_
                        == ffi::acamera_metadata_enum_acamera_lens_facing::ACAMERA_LENS_FACING_BACK
                            .0 as u8
                {
                    selected = Some(bytes);
                }
                ffi::ACameraMetadata_free(metadata);
                if selected.is_some() {
                    break;
                }
            }
            selected.or(fallback)
        };
        unsafe { ffi::ACameraManager_deleteCameraIdList(list) };
        result.ok_or_else(|| "设备没有可用相机".to_string())
    }
}

impl Drop for CameraSession {
    fn drop(&mut self) {
        unsafe {
            if !self.session.is_null() {
                let _ = ffi::ACameraCaptureSession_stopRepeating(self.session);
                ffi::ACameraCaptureSession_close(self.session);
            }
            if !self.request.is_null() {
                ffi::ACaptureRequest_free(self.request);
            }
            if !self.target.is_null() {
                ffi::ACameraOutputTarget_free(self.target);
            }
            if !self.output.is_null() {
                ffi::ACaptureSessionOutput_free(self.output);
            }
            if !self.container.is_null() {
                ffi::ACaptureSessionOutputContainer_free(self.container);
            }
            if !self.device.is_null() {
                let _ = ffi::ACameraDevice_close(self.device);
            }
            if !self.manager.is_null() {
                ffi::ACameraManager_delete(self.manager);
            }
        }
    }
}

fn check(action: &str, status: ffi::camera_status_t) -> Result<(), String> {
    if status == ffi::camera_status_t::ACAMERA_OK {
        Ok(())
    } else {
        Err(format!("{action}失败 (Camera2 {})", status.0))
    }
}

unsafe extern "C" fn camera_disconnected(
    _context: *mut std::ffi::c_void,
    _device: *mut ffi::ACameraDevice,
) {
    tracing::warn!("QR scanner camera disconnected");
}

unsafe extern "C" fn camera_error(
    _context: *mut std::ffi::c_void,
    _device: *mut ffi::ACameraDevice,
    error: i32,
) {
    tracing::warn!("QR scanner camera error: {}", error);
}
