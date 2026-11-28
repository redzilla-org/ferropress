//! The reference's default image sub-sizes and its resize dimension rules.

/// Images wider or taller than this get a `-scaled` copy fitted into a square of this side.
pub const BIG_IMAGE_THRESHOLD: u32 = 2560;

/// One registered sub-size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubSize {
    /// Size name as stored in the attachment metadata.
    pub name: &'static str,
    /// Maximum width; `0` means unconstrained.
    pub width: u32,
    /// Maximum height; `0` means unconstrained.
    pub height: u32,
    /// Center-crop to the exact box instead of fitting inside it.
    pub crop: bool,
}

/// The reference's default sub-sizes in the order it creates them: its priority sizes first
/// (`medium`, `large`, `thumbnail`, `medium_large`), then the rest in registration order.
pub const DEFAULT_SIZES: [SubSize; 6] = [
    SubSize {
        name: "medium",
        width: 300,
        height: 300,
        crop: false,
    },
    SubSize {
        name: "large",
        width: 1024,
        height: 1024,
        crop: false,
    },
    SubSize {
        name: "thumbnail",
        width: 150,
        height: 150,
        crop: true,
    },
    SubSize {
        name: "medium_large",
        width: 768,
        height: 0,
        crop: false,
    },
    SubSize {
        name: "1536x1536",
        width: 1536,
        height: 1536,
        crop: false,
    },
    SubSize {
        name: "2048x2048",
        width: 2048,
        height: 2048,
        crop: false,
    },
];

/// Source rectangle and output size of one resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Left edge of the source rectangle.
    pub src_x: u32,
    /// Top edge of the source rectangle.
    pub src_y: u32,
    /// Source rectangle width.
    pub src_w: u32,
    /// Source rectangle height.
    pub src_h: u32,
    /// Output width.
    pub width: u32,
    /// Output height.
    pub height: u32,
}

/// The reference's resize plan for an `orig_w` x `orig_h` image into `dest_w` x `dest_h`, or
/// `None` when no file is made: a zero source, no bounds, or a result within one pixel of the
/// original in both dimensions.
#[must_use]
pub fn resize_plan(orig_w: u32, orig_h: u32, dest_w: u32, dest_h: u32, crop: bool) -> Option<Plan> {
    if orig_w == 0 || orig_h == 0 || (dest_w == 0 && dest_h == 0) {
        return None;
    }
    let plan = if crop {
        crop_plan(orig_w, orig_h, dest_w, dest_h)
    } else {
        let (width, height) = constrain(orig_w, orig_h, dest_w, dest_h);
        Plan {
            src_x: 0,
            src_y: 0,
            src_w: orig_w,
            src_h: orig_h,
            width,
            height,
        }
    };
    // The reference skips sizes virtually identical to the original.
    if plan.width.abs_diff(orig_w) <= 1 && plan.height.abs_diff(orig_h) <= 1 {
        return None;
    }
    Some(plan)
}

// Never upscale: the box is clamped to the original, then the largest centered source
// rectangle with the box's aspect ratio is cut out.
fn crop_plan(orig_w: u32, orig_h: u32, dest_w: u32, dest_h: u32) -> Plan {
    let (ow, oh) = (f64::from(orig_w), f64::from(orig_h));
    let aspect = ow / oh;
    let mut width = dest_w.min(orig_w);
    let mut height = dest_h.min(orig_h);
    if width == 0 {
        width = round(f64::from(height) * aspect);
    }
    if height == 0 {
        height = round(f64::from(width) / aspect);
    }
    let ratio = (f64::from(width) / ow).max(f64::from(height) / oh);
    let src_w = round(f64::from(width) / ratio);
    let src_h = round(f64::from(height) / ratio);
    Plan {
        src_x: (orig_w - src_w.min(orig_w)) / 2,
        src_y: (orig_h - src_h.min(orig_h)) / 2,
        src_w,
        src_h,
        width,
        height,
    }
}

/// The reference's fit of `width` x `height` inside `max_w` x `max_h` (`0` = unconstrained),
/// keeping the aspect ratio and correcting the one-pixel rounding shortfall.
#[must_use]
pub fn constrain(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if max_w == 0 && max_h == 0 {
        return (width, height);
    }
    let (w, h) = (f64::from(width), f64::from(height));
    let mut width_ratio = 1.0_f64;
    let mut height_ratio = 1.0_f64;
    let did_width = max_w > 0 && width > max_w;
    let did_height = max_h > 0 && height > max_h;
    if did_width {
        width_ratio = f64::from(max_w) / w;
    }
    if did_height {
        height_ratio = f64::from(max_h) / h;
    }
    let smaller = width_ratio.min(height_ratio);
    let larger = width_ratio.max(height_ratio);
    // An unconstrained side is 0, so the larger ratio "overflows" it and the smaller one wins.
    let ratio = if round(w * larger) > max_w || round(h * larger) > max_h {
        smaller
    } else {
        larger
    };
    let mut out_w = round(w * ratio).max(1);
    let mut out_h = round(h * ratio).max(1);
    if did_width && out_w + 1 == max_w {
        out_w = max_w;
    }
    if did_height && out_h + 1 == max_h {
        out_h = max_h;
    }
    (out_w, out_h)
}

// Half away from zero, as the reference's `round`; inputs are non-negative pixel counts.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pixel sizes are non-negative and far below u32::MAX"
)]
fn round(value: f64) -> u32 {
    value.round() as u32
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SIZES, Plan, constrain, resize_plan};

    fn dims(orig_w: u32, orig_h: u32) -> Vec<(&'static str, u32, u32)> {
        DEFAULT_SIZES
            .iter()
            .filter_map(|size| {
                resize_plan(orig_w, orig_h, size.width, size.height, size.crop)
                    .map(|plan| (size.name, plan.width, plan.height))
            })
            .collect()
    }

    #[test]
    fn landscape_photo_gets_every_size() {
        assert_eq!(
            dims(2600, 1950),
            [
                ("medium", 300, 225),
                ("large", 1024, 768),
                ("thumbnail", 150, 150),
                ("medium_large", 768, 576),
                ("1536x1536", 1536, 1152),
                ("2048x2048", 2048, 1536),
            ]
        );
    }

    #[test]
    fn small_images_skip_larger_and_identical_sizes() {
        assert_eq!(dims(200, 100), [("thumbnail", 150, 100)]);
        assert_eq!(dims(150, 150), Vec::new());
        assert_eq!(dims(301, 200), [("thumbnail", 150, 150)]);
        assert_eq!(
            dims(800, 600),
            [
                ("medium", 300, 225),
                ("thumbnail", 150, 150),
                ("medium_large", 768, 576)
            ]
        );
    }

    #[test]
    fn portrait_crop_takes_the_centered_square() {
        assert_eq!(
            resize_plan(600, 1000, 150, 150, true),
            Some(Plan {
                src_x: 0,
                src_y: 200,
                src_w: 600,
                src_h: 600,
                width: 150,
                height: 150
            })
        );
        assert_eq!(
            resize_plan(1000, 800, 150, 0, true).map(|p| (p.width, p.height)),
            Some((150, 120))
        );
        assert_eq!(
            resize_plan(1000, 800, 0, 80, true).map(|p| (p.width, p.height)),
            Some((100, 80))
        );
    }

    #[test]
    fn degenerate_inputs_make_no_plan() {
        assert_eq!(resize_plan(0, 10, 5, 5, false), None);
        assert_eq!(resize_plan(10, 0, 5, 5, true), None);
        assert_eq!(resize_plan(10, 10, 0, 0, false), None);
    }

    #[test]
    fn constrain_matches_the_reference_fit() {
        assert_eq!(constrain(4000, 3000, 0, 0), (4000, 3000));
        assert_eq!(constrain(4000, 3000, 2560, 2560), (2560, 1920));
        assert_eq!(constrain(1000, 3000, 768, 0), (768, 2304));
        assert_eq!(constrain(3000, 1000, 0, 300), (900, 300));
        assert_eq!(constrain(100, 50, 300, 300), (100, 50));
        assert_eq!(constrain(1000, 1, 300, 300), (300, 1));
        assert_eq!(constrain(1999, 1001, 1000, 1000), (1000, 501));
        // 1999 * (1000 / 2001) rounds to 999: the fudge restores the bound it fell one short of.
        assert_eq!(constrain(2001, 1999, 1000, 1000), (1000, 1000));
        assert_eq!(constrain(1999, 2001, 1000, 1000), (1000, 1000));
    }

    proptest::proptest! {
        #[test]
        fn fits_stay_inside_the_box(w in 1_u32..6000, h in 1_u32..6000, mw in 1_u32..3000, mh in 1_u32..3000) {
            let (out_w, out_h) = constrain(w, h, mw, mh);
            proptest::prop_assert!(out_w <= mw && out_h <= mh);
            proptest::prop_assert!(out_w <= w && out_h <= h);
            proptest::prop_assert!(out_w >= 1 && out_h >= 1);
        }
    }
}
