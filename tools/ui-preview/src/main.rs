//! Render the actual production Slint tree without starting a native app or network session.
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel, SharedPixelBuffer};
use std::rc::Rc;
slint::include_modules!();

#[path = "../../../ale-gui/src/operation_epoch.rs"]
#[cfg(test)]
mod operation_epoch;
#[path = "../../../ale-gui/src/preferences.rs"]
#[cfg(test)]
mod preferences;

struct Platform(Rc<MinimalSoftwareWindow>);
impl slint::platform::Platform for Platform {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
    fn duration_since_start(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/ui-preview".into());
    std::fs::create_dir_all(&output)?;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Platform(window.clone())))?;
    let app = AppWindow::new()?;
    app.set_app_version("0.3.0".into());
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
            ("paired", MobilePage::Scan, true, false, false, false),
            ("voice", MobilePage::Voice, true, false, false, false),
            ("recording", MobilePage::Voice, true, true, false, false),
            ("processing", MobilePage::Voice, true, false, true, false),
            ("confirmation", MobilePage::Voice, true, false, false, true),
            ("settings", MobilePage::Settings, true, false, false, false),
        ] {
            app.set_current_page(page);
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
    println!("Rendered 35 production Slint states to {output}; sample data only, no Android runtime verification.");
    Ok(())
}
