//! Portable YUV_420_888 preview conversion. QR decoding keeps the full Y plane.
pub struct Plane<'a> {
    pub data: &'a [u8],
    pub row_stride: usize,
    pub pixel_stride: usize,
}

impl Plane<'_> {
    fn sample(&self, x: usize, y: usize) -> Result<i32, String> {
        if self.row_stride == 0 || self.pixel_stride == 0 {
            return Err("相机平面步长无效".into());
        }
        let index = y
            .checked_mul(self.row_stride)
            .and_then(|row| {
                x.checked_mul(self.pixel_stride)
                    .and_then(|col| row.checked_add(col))
            })
            .ok_or("相机平面尺寸溢出")?;
        self.data
            .get(index)
            .map(|&v| i32::from(v))
            .ok_or("相机平面数据不完整".into())
    }
}

pub fn color_preview(
    width: usize,
    height: usize,
    planes: [Plane<'_>; 3],
    rotation: u32,
) -> Result<(Vec<u8>, usize, usize), String> {
    if width < 2 || height < 2 || ![0, 90, 180, 270].contains(&rotation) {
        return Err("相机画面尺寸或方向无效".into());
    }
    let w = width / 2;
    let h = height / 2;
    let (out_w, out_h) = if rotation == 90 || rotation == 270 {
        (h, w)
    } else {
        (w, h)
    };
    let len = out_w
        .checked_mul(out_h)
        .and_then(|n| n.checked_mul(3))
        .ok_or("相机画面过大")?;
    let mut rgb = vec![0; len];
    for y in 0..h {
        for x in 0..w {
            let luma = (planes[0].sample(x * 2, y * 2)? - 16).max(0);
            let u = planes[1].sample(x, y)? - 128;
            let v = planes[2].sample(x, y)? - 128;
            // Android's usual limited-range BT.601 YUV preview.
            let channels = [
                298 * luma + 409 * v,
                298 * luma - 100 * u - 208 * v,
                298 * luma + 516 * u,
            ];
            let (dx, dy) = match rotation {
                90 => (h - 1 - y, x),
                180 => (w - 1 - x, h - 1 - y),
                270 => (y, w - 1 - x),
                _ => (x, y),
            };
            for (c, value) in channels.into_iter().enumerate() {
                rgb[(dy * out_w + dx) * 3 + c] = ((value + 128) >> 8).clamp(0, 255) as u8;
            }
        }
    }
    Ok((rgb, out_w, out_h))
}

pub fn choose_preview_size(configurations: &[i32]) -> Option<(i32, i32)> {
    configurations
        .chunks_exact(4)
        .filter(|c| c[0] == 35 && c[3] == 0 && c[1] > 0 && c[2] > 0)
        .min_by_key(|c| {
            let area = i64::from(c[1]) * i64::from(c[2]);
            (area > 1280 * 720, (area - 1280 * 720).abs())
        })
        .map(|c| (c[1], c[2]))
}

#[derive(Clone, Copy, Default)]
pub struct ScanRegion {
    pub view_width: f32,
    pub view_height: f32,
    pub box_size: f32,
    pub rotation: u32,
}

/// Crop in display space after the same rotation and center-cover transform
/// used by the preview. No pixel outside the visible rectangle reaches Quirc.
pub fn crop_scan_region(
    gray: &[u8],
    width: usize,
    height: usize,
    rotation: u32,
    region: ScanRegion,
) -> Option<(Vec<u8>, usize, usize)> {
    if region.rotation != rotation
        || width == 0
        || height == 0
        || gray.len() != width.checked_mul(height)?
        || ![0, 90, 180, 270].contains(&rotation)
        || ![region.view_width, region.view_height, region.box_size]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
    {
        return None;
    }
    let (rw, rh) = if rotation == 90 || rotation == 270 {
        (height, width)
    } else {
        (width, height)
    };
    let scale = (region.view_width / rw as f32).max(region.view_height / rh as f32);
    let side = (region.box_size / scale).min(rw.min(rh) as f32);
    let left = ((rw as f32 - side) / 2.0).ceil() as usize;
    let top = ((rh as f32 - side) / 2.0).ceil() as usize;
    let right = ((rw as f32 + side) / 2.0).floor() as usize;
    let bottom = ((rh as f32 + side) / 2.0).floor() as usize;
    if right <= left || bottom <= top {
        return None;
    }
    let mut crop = Vec::with_capacity((right - left) * (bottom - top));
    for y in top..bottom {
        for x in left..right {
            let (sx, sy) = match rotation {
                90 => (y, height - 1 - x),
                180 => (width - 1 - x, height - 1 - y),
                270 => (width - 1 - y, x),
                _ => (x, y),
            };
            crop.push(gray[sy * width + sx]);
        }
    }
    Some((crop, right - left, bottom - top))
}

pub fn camera_rank(logical: bool, focal_length: f32, sensor_width: f32) -> (bool, u32) {
    let distance = if focal_length > 0.0 && sensor_width > 0.0 {
        ((36.0 * focal_length / sensor_width - 28.0).abs() * 100.0) as u32
    } else {
        u32::MAX
    };
    (!logical, distance)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roi_excludes_every_pixel_outside_the_visible_box() {
        // A 12x8 image center-covers a tall 4x8 view: its side edges are cropped.
        let gray: Vec<u8> = (0..96).collect();
        let region = ScanRegion {
            view_width: 4.0,
            view_height: 8.0,
            box_size: 4.0,
            rotation: 0,
        };
        let (crop, w, h) = crop_scan_region(&gray, 12, 8, 0, region).unwrap();
        assert_eq!((w, h), (4, 4));
        let expected: Vec<u8> = (2..6)
            .flat_map(|y| (4..8).map(move |x| y * 12 + x))
            .collect();
        assert_eq!(crop, expected);
        // Rotation swaps source axes but must select the same physical center.
        for rotation in [90, 180, 270] {
            let region = ScanRegion {
                view_width: 8.0,
                view_height: 12.0,
                box_size: 4.0,
                rotation,
            };
            let (crop, w, h) = crop_scan_region(&gray, 12, 8, rotation, region).unwrap();
            assert!(w <= 4 && h <= 4);
            assert!(crop.iter().all(|v| expected.contains(v)));
        }
        assert!(
            crop_scan_region(&gray, 12, 8, 90, region).is_none(),
            "do not decode with stale pre-rotation geometry"
        );
        assert!(crop_scan_region(&gray, 12, 8, 0, ScanRegion::default()).is_none());
    }
    #[test]
    fn rear_logical_group_then_normal_field_of_view_wins() {
        assert!(camera_rank(true, 6.8, 9.8) < camera_rank(false, 6.8, 9.8));
        assert!(camera_rank(false, 6.8, 9.8) < camera_rank(false, 2.2, 6.4));
        assert!(camera_rank(false, 6.8, 9.8) < camera_rank(false, 19.0, 6.4));
        assert!(camera_rank(false, 6.8, 9.8) < camera_rank(false, 0.0, 0.0));
    }
    #[test]
    fn padded_interleaved_chroma_produces_color() {
        let y = [82; 22];
        let u = [90, 0, 128, 0, 0, 0, 90, 0, 128];
        let v = [240, 0, 128, 0, 0, 0, 240, 0, 128];
        let (rgb, w, h) = color_preview(
            4,
            4,
            [
                Plane {
                    data: &y,
                    row_stride: 6,
                    pixel_stride: 1,
                },
                Plane {
                    data: &u,
                    row_stride: 6,
                    pixel_stride: 2,
                },
                Plane {
                    data: &v,
                    row_stride: 6,
                    pixel_stride: 2,
                },
            ],
            0,
        )
        .unwrap();
        assert_eq!((w, h), (2, 2));
        assert!(rgb[0] > 250 && rgb[1] < 3 && rgb[2] < 3);
        assert_eq!(&rgb[..6], &rgb[6..]);
        assert_eq!(rgb[3], rgb[4]);
        assert_eq!(rgb[4], rgb[5]);
    }
    #[test]
    fn rotation_moves_pixels_and_rejects_truncated_planes() {
        let y = [16, 16, 235, 235, 16, 16, 235, 235];
        let chroma = [128, 128];
        let planes = || {
            [
                Plane {
                    data: &y,
                    row_stride: 4,
                    pixel_stride: 1,
                },
                Plane {
                    data: &chroma,
                    row_stride: 2,
                    pixel_stride: 1,
                },
                Plane {
                    data: &chroma,
                    row_stride: 2,
                    pixel_stride: 1,
                },
            ]
        };
        let (rgb, w, h) = color_preview(4, 2, planes(), 90).unwrap();
        assert_eq!((w, h), (1, 2));
        assert_eq!(rgb, [0, 0, 0, 255, 255, 255]);
        assert!(color_preview(8, 8, planes(), 0).is_err());
    }
    #[test]
    fn selects_only_advertised_yuv_outputs() {
        assert_eq!(
            choose_preview_size(&[35, 1920, 1080, 0, 35, 640, 480, 0, 35, 1280, 720, 1]),
            Some((640, 480))
        );
        assert_eq!(choose_preview_size(&[256, 1280, 720, 0]), None);
    }
}
