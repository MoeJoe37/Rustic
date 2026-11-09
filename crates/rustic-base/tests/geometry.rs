//! Unit-typed geometry and checked surface allocation (`base.geometry`).

use rustic_base::{
    Css, Device, Length, MAX_COORDINATE, Point2, Rect, Scale, Size2, SurfaceAllocation,
    SurfaceError, SurfaceLimits,
};

fn css_rect(x: f64, y: f64, w: f64, h: f64) -> Rect<Css> {
    Rect::new(Point2::new(x, y).unwrap(), Size2::new(w, h).unwrap()).unwrap()
}

const LIMITS: SurfaceLimits = SurfaceLimits {
    max_width: 1000,
    max_height: 800,
    max_bytes: 1000 * 800 * 4,
};

#[test]
fn values_are_finite_bounded_and_sizes_nonnegative() {
    assert!(Length::<Css>::new(f64::NAN).is_err());
    assert!(Length::<Css>::new(f64::INFINITY).is_err());
    assert!(Length::<Device>::new(MAX_COORDINATE).is_ok());
    assert!(Length::<Device>::new(-MAX_COORDINATE - 1.0).is_err());
    assert_eq!(
        Length::<Css>::new(-0.0).unwrap().get().to_bits(),
        0.0f64.to_bits()
    );
    assert!(Size2::<Css>::new(-1.0, 0.0).is_err());
    let big = Length::<Css>::new(MAX_COORDINATE).unwrap();
    assert!(big.checked_add(big).is_err());
    assert_eq!(big.checked_sub(big).unwrap(), Length::ZERO);
    // A rectangle's far edge must be in range too.
    let origin = Point2::<Css>::new(MAX_COORDINATE - 1.0, 0.0).unwrap();
    assert!(Rect::new(origin, Size2::new(2.0, 1.0).unwrap()).is_err());
}

#[test]
fn units_change_only_through_an_explicit_scale() {
    let scale = Scale::<Css, Device>::new(2.0).unwrap();
    let device: Rect<Device> = scale.rect(css_rect(1.5, 2.0, 10.0, 3.25)).unwrap();
    assert_eq!(device.origin(), Point2::new(3.0, 4.0).unwrap());
    assert_eq!(device.size(), Size2::new(20.0, 6.5).unwrap());
    for bad in [0.0, -1.0, 64.5, f64::NAN] {
        assert!(Scale::<Css, Device>::new(bad).is_err(), "{bad}");
    }
    // Scaling can leave the coordinate range.
    let huge = css_rect(MAX_COORDINATE / 2.0, 0.0, 1.0, 1.0);
    assert!(Scale::<Css, Device>::new(4.0).unwrap().rect(huge).is_err());
}

#[test]
fn surfaces_are_checked_before_allocation() {
    let a = SurfaceAllocation::check(100, 50, 4, LIMITS).unwrap();
    assert_eq!((a.width(), a.height(), a.bytes()), (100, 50, 20_000));
    assert_eq!(
        SurfaceAllocation::check(0, 5, 4, LIMITS),
        Err(SurfaceError::Empty)
    );
    assert_eq!(
        SurfaceAllocation::check(5, 5, 0, LIMITS),
        Err(SurfaceError::Empty)
    );
    assert_eq!(
        SurfaceAllocation::check(1001, 1, 4, LIMITS),
        Err(SurfaceError::Dimension)
    );
    // Within both axes but over the byte budget.
    let tight = SurfaceLimits {
        max_bytes: 999,
        ..LIMITS
    };
    assert_eq!(
        SurfaceAllocation::check(10, 25, 4, tight),
        Err(SurfaceError::Bytes)
    );
    // Overflowing multiplication is a budget failure, not a wrap.
    let unbounded = SurfaceLimits {
        max_width: u32::MAX,
        max_height: u32::MAX,
        max_bytes: usize::MAX,
    };
    assert_eq!(
        SurfaceAllocation::check(u32::MAX, u32::MAX, usize::MAX, unbounded),
        Err(SurfaceError::Bytes)
    );
}

#[test]
fn device_rectangles_round_outwards_to_whole_pixels() {
    let scale = Scale::<Css, Device>::new(1.5).unwrap();
    // 0.75..15.75 by 1.5..4.5 in device pixels covers 0..16 by 1..5.
    let rect = scale.rect(css_rect(0.5, 1.0, 10.0, 2.0)).unwrap();
    let a = SurfaceAllocation::covering(rect, 4, LIMITS).unwrap();
    assert_eq!((a.width(), a.height()), (16, 4));
    let empty = Rect::<Device>::new(
        Point2::new(3.0, 3.0).unwrap(),
        Size2::new(0.0, 2.0).unwrap(),
    )
    .unwrap();
    assert_eq!(
        SurfaceAllocation::covering(empty, 4, LIMITS),
        Err(SurfaceError::Empty)
    );
}
