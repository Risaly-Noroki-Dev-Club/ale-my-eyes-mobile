//! Render the actual production Slint tree without starting a native app or network session.
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel, SharedPixelBuffer};
use std::rc::Rc;
slint::include_modules!();

#[path = "../../../ale-gui/src/camera_frame.rs"]
#[cfg(test)]
mod camera_frame;
#[cfg(test)]
mod scan_tests;

#[path = "../../../ale-gui/src/operation_epoch.rs"]
#[cfg(test)]
mod operation_epoch;
#[path = "../../../ale-gui/src/preferences.rs"]
#[cfg(test)]
mod preferences;

struct Platform(Rc<MinimalSoftwareWindow>, std::time::Instant);
impl slint::platform::Platform for Platform {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
    fn duration_since_start(&self) -> std::time::Duration {
        self.1.elapsed()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/ui-preview".into());
    std::fs::create_dir_all(&output)?;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Platform(
        window.clone(),
        std::time::Instant::now(),
    )))?;
    let app = AppWindow::new()?;
    app.set_app_version("0.3.0 · rear-roi-3".into());
    app.set_camera_label("后置相机 · 1×（系统多摄）".into());
    // Synthetic color chart, not camera runtime evidence.
    let mut camera_chart = SharedPixelBuffer::<Rgb8Pixel>::new(640, 960);
    for (i, pixel) in camera_chart.make_mut_slice().iter_mut().enumerate() {
        let x = i % 640;
        let y = i / 640;
        *pixel = Rgb8Pixel::new(
            (x * 255 / 640) as u8,
            (y * 255 / 960) as u8,
            if (x / 80 + y / 80) % 2 == 0 { 180 } else { 60 },
        );
    }
    app.set_remote_scan_preview(slint::Image::from_rgb8(camera_chart));
    app.set_protocol_version("ale-my-eyes v3".into());
    app.set_device_name("桌面工作站".into());
    app.set_device_address("192.168.1.20:7380".into());
    app.show()?;
    // Exercise production hit targets and modal interception, without mocking layouts.
    let pages = Rc::new(std::cell::Cell::new(0));
    let observed = pages.clone();
    app.on_change_page(move |_| observed.set(observed.get() + 1));
    let recordings = Rc::new(std::cell::Cell::new(0));
    let observed = recordings.clone();
    app.on_toggle_listening(move || observed.set(observed.get() + 1));
    let backs = Rc::new(std::cell::Cell::new(0));
    let observed = backs.clone();
    app.on_back(move || observed.set(observed.get() + 1));
    window.set_size(PhysicalSize::new(402, 874));
    app.set_current_page(MobilePage::Voice);
    app.set_remote_connected(true);
    let click = |x, y| {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(402, 874);
        window.draw_if_needed(|renderer| {
            renderer.render(buffer.make_mut_slice(), 402);
        });
        image::save_buffer(
            format!("{output}/interaction.png"),
            buffer.as_bytes(),
            402,
            874,
            image::ColorType::Rgb8,
        )
        .unwrap();
        use slint::platform::{PointerEventButton, WindowEvent};
        let position = slint::LogicalPosition::new(x, y);
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    };
    click(335.0, 830.0);
    assert_eq!(pages.get(), 1, "settings navigation must be clickable");
    app.set_voice_recording(true);
    click(335.0, 830.0);
    assert_eq!(pages.get(), 1, "recording must lock navigation");
    app.set_voice_recording(false);
    click(201.0, 736.0);
    assert_eq!(recordings.get(), 1, "microphone must be clickable");
    app.set_show_confirmation(true);
    click(335.0, 830.0);
    click(201.0, 736.0);
    assert_eq!(pages.get(), 1, "modal must intercept navigation");
    assert_eq!(recordings.get(), 1, "modal must intercept microphone");
    window.dispatch_event(slint::platform::WindowEvent::KeyPressed {
        text: slint::platform::Key::Back.into(),
    });
    assert_eq!(
        backs.get(),
        1,
        "system Back must reach the application while a modal is open"
    );
    app.set_show_confirmation(false);
    // A screenshot alone cannot detect a ScrollView with drag panning disabled.
    // Use overflowing production settings and drag the content, not its scrollbar.
    for (case, width, height, font_scale, density, start_x, start_y) in [
        ("large-text", 402, 874, 2.0, 1.0, 200.0, 550.0),
        ("small-phone", 360, 640, 1.0, 1.0, 180.0, 400.0),
        ("switch-row", 360, 640, 1.0, 1.0, 300.0, 200.0),
        ("dense-phone", 360, 640, 1.0, 3.0, 180.0, 400.0),
        ("landscape", 874, 402, 1.0, 1.0, 437.0, 240.0),
    ] {
        window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
            scale_factor: density,
        });
        let physical_width = (width as f32 * density) as u32;
        let physical_height = (height as f32 * density) as u32;
        window.set_size(PhysicalSize::new(physical_width, physical_height));
        app.set_current_page(MobilePage::Scan);
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        let mut reset = SharedPixelBuffer::<Rgb8Pixel>::new(physical_width, physical_height);
        window.draw_if_needed(|renderer| {
            renderer.render(reset.make_mut_slice(), physical_width as usize);
        });
        app.set_current_page(MobilePage::Settings);
        app.set_font_scale(font_scale);
        let preferences = Rc::new(std::cell::Cell::new(0));
        let observed = preferences.clone();
        app.on_change_preference(move |_, _| observed.set(observed.get() + 1));
        let render = || {
            slint::platform::update_timers_and_animations();
            window.request_redraw();
            let mut pixels = SharedPixelBuffer::<Rgb8Pixel>::new(physical_width, physical_height);
            window.draw_if_needed(|renderer| {
                renderer.render(pixels.make_mut_slice(), physical_width as usize);
            });
            pixels
        };
        let before = render();
        let start = slint::LogicalPosition::new(start_x, start_y);
        let touch = |x, y, phase| {
            i_slint_core::window::WindowInner::from_pub(app.window()).process_touch_input(
                0,
                i_slint_core::lengths::LogicalPoint::new(x, y),
                phase,
            );
        };
        use i_slint_core::input::TouchPhase;
        touch(start.x, start.y, TouchPhase::Started);
        for step in 1..=12 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            touch(start_x, start_y - step as f32 * 12.0, TouchPhase::Moved);
            render();
        }
        touch(start_x, start_y - 144.0, TouchPhase::Ended);
        let after = render();
        let content_end = ((height as f32 - 110.0) * density) as usize;
        let stride = physical_width as usize;
        let changed = (30..content_end)
            .flat_map(|y| (20..stride - 20).map(move |x| (y * stride + x) * 3))
            .filter(|&i| before.as_bytes()[i..i + 3] != after.as_bytes()[i..i + 3])
            .count();
        assert!(
            changed > 1000,
            "{case}: dragging settings must move its content; changed {changed} pixels"
        );
        assert_eq!(
            preferences.get(),
            0,
            "scrolling must not change preferences"
        );
        assert_eq!(
            &before.as_bytes()[((height as f32 - 60.0) * density) as usize * stride * 3..],
            &after.as_bytes()[((height as f32 - 60.0) * density) as usize * stride * 3..],
            "bottom navigation must remain fixed while settings scroll"
        );
        image::save_buffer(
            format!("{output}/{case}-settings-scrolled.png"),
            after.as_bytes(),
            physical_width,
            physical_height,
            image::ColorType::Rgb8,
        )?;
        println!("Native touch settings regression passed: {case}");
    }
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
    println!("Production settings drag check passed: content moves, preferences unchanged, navigation fixed.");
    println!("Production UI hit-target checks passed: navigation, recording lock, microphone, modal interception.");
    for (label, size, scale) in [
        ("phone", PhysicalSize::new(402, 874), 1.0),
        ("large-text", PhysicalSize::new(360, 800), 2.0),
        ("medium-text", PhysicalSize::new(390, 844), 1.5),
        ("landscape", PhysicalSize::new(874, 402), 1.0),
        ("tablet", PhysicalSize::new(1024, 768), 1.0),
    ] {
        window.set_size(size);
        app.set_font_scale(scale);
        for (name, page, connected, recording, busy, confirmation) in [
            ("scan", MobilePage::Scan, false, false, false, false),
            (
                "camera-preview",
                MobilePage::Scan,
                false,
                false,
                false,
                false,
            ),
            ("paired", MobilePage::Scan, true, false, false, false),
            ("voice", MobilePage::Voice, true, false, false, false),
            ("recording", MobilePage::Voice, true, true, false, false),
            ("processing", MobilePage::Voice, true, false, true, false),
            ("confirmation", MobilePage::Voice, true, false, false, true),
            ("settings", MobilePage::Settings, true, false, false, false),
        ] {
            app.set_current_page(page);
            app.set_remote_scanning(name == "camera-preview");
            app.set_remote_connected(connected);
            app.set_voice_recording(recording);
            app.set_is_busy(busy);
            app.set_show_confirmation(confirmation);
            app.set_confirmation_text("即将在电脑上执行操作。请检查以下内容，然后选择确认或取消。\n打开浏览器并搜索天气。".into());
            app.set_remote_status(
                if connected {
                    "已连接到桌面工作站"
                } else {
                    "扫描桌面端二维码以开始"
                }
                .into(),
            );
            slint::platform::update_timers_and_animations();
            window.request_redraw();
            let mut pixels = SharedPixelBuffer::<Rgb8Pixel>::new(size.width, size.height);
            window.draw_if_needed(|renderer| {
                renderer.render(pixels.make_mut_slice(), size.width as usize);
            });
            image::save_buffer(
                format!("{output}/{label}-{name}.png"),
                pixels.as_bytes(),
                size.width,
                size.height,
                image::ColorType::Rgb8,
            )?;
        }
    }
    println!("Rendered 40 production Slint states to {output}; sample data only, no Android runtime verification.");
    Ok(())
}
