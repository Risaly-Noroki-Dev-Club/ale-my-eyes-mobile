use crate::camera_frame::{crop_scan_region, ScanRegion};

#[test]
fn decoder_accepts_inside_code_and_ignores_outside_code() {
    let qr = qrcode::QrCode::new(b"only-inside-the-box").unwrap();
    let module = 4;
    let qr_width = qr.width() * module;
    let decode = |left, top| {
        let mut frame = vec![255; 640 * 480];
        for y in 0..qr_width {
            for x in 0..qr_width {
                frame[(top + y) * 640 + left + x] =
                    if qr[(x / module, y / module)] == qrcode::Color::Dark {
                        0
                    } else {
                        255
                    };
            }
        }
        let (crop, width, height) = crop_scan_region(
            &frame,
            640,
            480,
            0,
            ScanRegion {
                view_width: 640.0,
                view_height: 480.0,
                box_size: 240.0,
                rotation: 0,
            },
        )
        .unwrap();
        quircs::Quirc::default()
            .identify(width, height, &crop)
            .filter_map(Result::ok)
            .filter_map(|code| code.decode().ok())
            .map(|code| code.payload)
            .collect::<Vec<_>>()
    };
    assert_eq!(decode(270, 190), vec![b"only-inside-the-box".to_vec()]);
    assert!(
        decode(20, 190).is_empty(),
        "a valid QR outside the box must not pair"
    );
    assert!(
        decode(150, 190).is_empty(),
        "a QR crossing the frame border must not pair"
    );
}
