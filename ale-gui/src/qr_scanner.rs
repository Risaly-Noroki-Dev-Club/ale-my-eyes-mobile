#![cfg(target_os = "android")]

use crate::camera_frame::{
    camera_rank, choose_preview_size, color_preview, crop_scan_region, Plane, ScanRegion,
};
use ale_core::remote::PairingInfo;
use ndk::media::image_reader::{AcquireResult, ImageFormat, ImageReader};
use ndk::native_window::NativeWindow;
use ndk_sys as ffi;
use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const SCAN_TIMEOUT: Duration = Duration::from_secs(45);
// Cancelling and immediately retrying must not open a second camera before
// the previous session has completed synchronous device teardown.
static CAMERA_GATE: Mutex<()> = Mutex::new(());

struct CameraProfile {
    id: Vec<u8>,
    width: i32,
    height: i32,
    orientation: u32,
    logical: bool,
    rank: (bool, u32),
}

pub fn scan_pairing_info(
    cancelled: Arc<AtomicBool>,
    region: Arc<Mutex<ScanRegion>>,
    selected: impl Fn(String),
    preview: impl Fn(Vec<u8>, usize, usize, u32) + Send + Sync + 'static,
) -> Result<PairingInfo, String> {
    let _camera_guard = CAMERA_GATE.lock().map_err(|_| "相机状态不可用")?;
    if cancelled.load(Ordering::Acquire) {
        return Err("扫描已取消".into());
    }
    let manager = unsafe { ffi::ACameraManager_create() };
    if manager.is_null() {
        return Err("无法访问 Android 相机服务".into());
    }
    let profile = CameraSession::back_camera_profile(manager);
    unsafe { ffi::ACameraManager_delete(manager) };
    let profile = profile?;
    selected(
        if profile.logical {
            "后置相机 · 1×（系统多摄）"
        } else {
            "后置相机 · 标准视角"
        }
        .into(),
    );
    tracing::info!(
        "Selected rear camera ID: {}",
        String::from_utf8_lossy(&profile.id)
    );
    tracing::info!(
        "QR camera: {}x{}, sensor rotation {}",
        profile.width,
        profile.height,
        profile.orientation
    );
    // Query the display rotation once per scan instead of inside every camera
    // frame callback (that path did a JNI attach + WindowManager lookup per
    // frame). A rotation change mid-scan only affects preview orientation.
    let display_rotation = crate::android::display_rotation().unwrap_or(0);
    let (sender, receiver) = mpsc::sync_channel(1);
    let found = Arc::new(AtomicBool::new(false));
    let decoder = Arc::new(Mutex::new(quircs::Quirc::default()));
    let preview = Arc::new(preview);
    let preview_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut reader = ImageReader::new(profile.width, profile.height, ImageFormat::YUV_420_888, 3)
        .map_err(|error| format!("无法创建扫码画面: {error}"))?;
    reader
        .set_image_listener(Box::new({
            let found = found.clone();
            let cancelled = cancelled.clone();
            let decoder = decoder.clone();
            let preview = preview.clone();
            let preview_counter = preview_counter.clone();
            let orientation = profile.orientation;
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
                let rotation = (orientation + 360 - display_rotation) % 360;
                if preview_counter
                    .fetch_add(1, Ordering::Relaxed)
                    .is_multiple_of(6)
                {
                    match copy_color_preview(&image, rotation) {
                        Ok((rgb, w, h)) => preview(rgb, w, h, rotation),
                        Err(error) => tracing::warn!("Camera preview: {error}"),
                    }
                }
                let Some((gray, width, height)) = region
                    .lock()
                    .ok()
                    .and_then(|region| crop_scan_region(&gray, width, height, rotation, *region))
                else {
                    return;
                };
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

    let camera = CameraSession::open(reader, &profile.id)?;
    let deadline = std::time::Instant::now() + SCAN_TIMEOUT;
    let result = loop {
        if cancelled.load(Ordering::Acquire) {
            break Err("扫描已取消".to_string());
        }
        let error = camera.error.load(Ordering::Acquire);
        if error != 0 {
            break Err(if error == -1 {
                "相机已断开，请关闭其他占用相机的应用后重试".into()
            } else {
                format!("相机服务发生错误 ({error})，请重新扫描")
            });
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

fn copy_color_preview(
    image: &ndk::media::image_reader::Image,
    rotation: u32,
) -> Result<(Vec<u8>, usize, usize), String> {
    let plane = |index| -> Result<Plane<'_>, String> {
        Ok(Plane {
            data: image.plane_data(index).map_err(|e| e.to_string())?,
            row_stride: image.plane_row_stride(index).map_err(|e| e.to_string())? as usize,
            pixel_stride: image.plane_pixel_stride(index).map_err(|e| e.to_string())? as usize,
        })
    };
    color_preview(
        image.width().map_err(|e| e.to_string())? as usize,
        image.height().map_err(|e| e.to_string())? as usize,
        [plane(0)?, plane(1)?, plane(2)?],
        rotation,
    )
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
    error: Box<AtomicI32>,
    ready: Box<AtomicBool>,
}

impl CameraSession {
    fn open(reader: ImageReader, camera_id: &[u8]) -> Result<Self, String> {
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
            error: Box::new(AtomicI32::new(0)),
            ready: Box::new(AtomicBool::new(false)),
        };
        if camera.manager.is_null() {
            return Err("无法访问 Android 相机服务".to_string());
        }

        let mut device_callbacks = ffi::ACameraDevice_StateCallbacks {
            context: (&*camera.error as *const AtomicI32).cast_mut().cast(),
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
            context: (&*camera.ready as *const AtomicBool).cast_mut().cast(),
            onClosed: None,
            onReady: Some(session_ready),
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
        // Camera2 expects the repeating request to start after `onReady`. Wait
        // for it under a bounded deadline; if a device never reports ready we
        // fall back to the previous behaviour instead of failing the scan.
        let ready_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !camera.ready.load(Ordering::Acquire) {
            if camera.error.load(Ordering::Acquire) != 0 {
                return Err("相机会话在就绪前断开".to_string());
            }
            if std::time::Instant::now() >= ready_deadline {
                tracing::warn!("Camera session did not report onReady; starting capture");
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

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
        // Logical rear cameras use their normal 1x field of view; let Camera2
        // manage the physical sensors instead of opening arbitrary auxiliary IDs.
        let zoom = 1.0_f32;
        let _ = unsafe {
            ffi::ACaptureRequest_setEntry_float(
                camera.request,
                ffi::acamera_metadata_tag::ACAMERA_CONTROL_ZOOM_RATIO.0,
                1,
                &zoom,
            )
        };
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

    fn back_camera_profile(manager: *mut ffi::ACameraManager) -> Result<CameraProfile, String> {
        let mut list = ptr::null_mut();
        check("读取相机列表", unsafe {
            ffi::ACameraManager_getCameraIdList(manager, &mut list)
        })?;
        if list.is_null() {
            return Err("设备没有可用相机".to_string());
        }

        let result = unsafe {
            if (*list).numCameras <= 0 || (*list).cameraIds.is_null() {
                ffi::ACameraManager_deleteCameraIdList(list);
                return Err("设备没有可用相机".into());
            }
            let ids = std::slice::from_raw_parts((*list).cameraIds, (*list).numCameras as usize);
            let mut selected: Option<CameraProfile> = None;
            for &id in ids {
                if id.is_null() {
                    continue;
                }
                let bytes = CStr::from_ptr(id).to_bytes_with_nul().to_vec();
                let mut metadata = ptr::null_mut();
                if ffi::ACameraManager_getCameraCharacteristics(manager, id, &mut metadata)
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
                let back = status == ffi::camera_status_t::ACAMERA_OK
                    && entry.count > 0
                    && !entry.data.u8_.is_null()
                    && *entry.data.u8_
                        == ffi::acamera_metadata_enum_acamera_lens_facing::ACAMERA_LENS_FACING_BACK
                            .0 as u8;
                if !back {
                    ffi::ACameraMetadata_free(metadata);
                    continue;
                }
                let mut capabilities = std::mem::zeroed::<ffi::ACameraMetadata_const_entry>();
                let logical = ffi::ACameraMetadata_getConstEntry(
                    metadata,
                    ffi::acamera_metadata_tag::ACAMERA_REQUEST_AVAILABLE_CAPABILITIES.0,
                    &mut capabilities,
                ) == ffi::camera_status_t::ACAMERA_OK
                    && capabilities.count > 0
                    && !capabilities.data.u8_.is_null()
                    && std::slice::from_raw_parts(
                        capabilities.data.u8_,
                        capabilities.count as usize,
                    )
                    .contains(&11);
                let mut focal = std::mem::zeroed::<ffi::ACameraMetadata_const_entry>();
                let focal_length = if ffi::ACameraMetadata_getConstEntry(
                    metadata,
                    ffi::acamera_metadata_tag::ACAMERA_LENS_INFO_AVAILABLE_FOCAL_LENGTHS.0,
                    &mut focal,
                ) == ffi::camera_status_t::ACAMERA_OK
                    && focal.count > 0
                    && !focal.data.f.is_null()
                {
                    *focal.data.f
                } else {
                    0.0
                };
                let mut physical = std::mem::zeroed::<ffi::ACameraMetadata_const_entry>();
                let sensor_width = if ffi::ACameraMetadata_getConstEntry(
                    metadata,
                    ffi::acamera_metadata_tag::ACAMERA_SENSOR_INFO_PHYSICAL_SIZE.0,
                    &mut physical,
                ) == ffi::camera_status_t::ACAMERA_OK
                    && physical.count > 0
                    && !physical.data.f.is_null()
                {
                    *physical.data.f
                } else {
                    0.0
                };
                let mut orientation = std::mem::zeroed::<ffi::ACameraMetadata_const_entry>();
                let rotation = if ffi::ACameraMetadata_getConstEntry(
                    metadata,
                    ffi::acamera_metadata_tag::ACAMERA_SENSOR_ORIENTATION.0,
                    &mut orientation,
                ) == ffi::camera_status_t::ACAMERA_OK
                    && orientation.count > 0
                    && !orientation.data.i32_.is_null()
                {
                    (*orientation.data.i32_).rem_euclid(360) as u32
                } else {
                    0
                };
                let mut streams = std::mem::zeroed::<ffi::ACameraMetadata_const_entry>();
                if ffi::ACameraMetadata_getConstEntry(
                    metadata,
                    ffi::acamera_metadata_tag::ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS.0,
                    &mut streams,
                ) == ffi::camera_status_t::ACAMERA_OK
                    && streams.count > 0
                    && !streams.data.i32_.is_null()
                {
                    if let Some((width, height)) = choose_preview_size(std::slice::from_raw_parts(
                        streams.data.i32_,
                        streams.count as usize,
                    )) {
                        let profile = CameraProfile {
                            id: bytes,
                            width,
                            height,
                            orientation: rotation,
                            logical,
                            rank: camera_rank(logical, focal_length, sensor_width),
                        };
                        if selected
                            .as_ref()
                            .is_none_or(|old| (profile.rank, &profile.id) < (old.rank, &old.id))
                        {
                            selected = Some(profile);
                        }
                    }
                }
                ffi::ACameraMetadata_free(metadata);
            }
            selected
        };
        unsafe { ffi::ACameraManager_deleteCameraIdList(list) };
        result.ok_or_else(|| "没有可用的后置扫码相机；不会自动切换到前置相机".to_string())
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

unsafe extern "C" fn session_ready(
    context: *mut std::ffi::c_void,
    _session: *mut ffi::ACameraCaptureSession,
) {
    if !context.is_null() {
        // SAFETY: the context points at `CameraSession::ready`, which outlives
        // the capture session callbacks.
        (*context.cast::<AtomicBool>()).store(true, Ordering::Release);
    }
}

unsafe extern "C" fn camera_disconnected(
    context: *mut std::ffi::c_void,
    _device: *mut ffi::ACameraDevice,
) {
    if !context.is_null() {
        unsafe { &*context.cast::<AtomicI32>() }.store(-1, Ordering::Release);
    }
    tracing::warn!("QR scanner camera disconnected");
}

unsafe extern "C" fn camera_error(
    context: *mut std::ffi::c_void,
    _device: *mut ffi::ACameraDevice,
    error: i32,
) {
    if !context.is_null() {
        unsafe { &*context.cast::<AtomicI32>() }.store(error, Ordering::Release);
    }
    tracing::warn!("QR scanner camera error: {}", error);
}
