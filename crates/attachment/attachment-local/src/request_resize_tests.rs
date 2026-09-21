use super::*;
use image::imageops::FilterType;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

thread_local! { static ALLOCATIONS: Cell<Option<(usize,usize)>> = const { Cell::new(None) }; }
struct CountedAllocator;
fn record(bytes: usize) {
    let _ = ALLOCATIONS.try_with(|value| {
        if let Some((largest, total)) = value.get() {
            value.set(Some((largest.max(bytes), total.saturating_add(bytes))));
        }
    });
}
unsafe impl GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            record(size);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }
}
#[global_allocator]
static ALLOCATOR: CountedAllocator = CountedAllocator;

fn measured<T>(operation: impl FnOnce() -> T) -> (T, usize, usize) {
    ALLOCATIONS.with(|value| value.set(Some((0, 0))));
    let result = operation();
    let (largest, total) = ALLOCATIONS.with(|value| value.replace(None).unwrap());
    (result, largest, total)
}

fn patterned(width: u32, height: u32) -> DynamicImage {
    DynamicImage::ImageRgba8(image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([
            (x.wrapping_mul(37).wrapping_add(y.wrapping_mul(19)) % 256) as u8,
            (x.wrapping_mul(7).wrapping_add(y.wrapping_mul(71)) % 256) as u8,
            (x.wrapping_mul(53).wrapping_add(y.wrapping_mul(11)) % 256) as u8,
            (x.wrapping_mul(17).wrapping_add(y.wrapping_mul(23)) % 256) as u8,
        ])
    }))
}

#[test]
fn all_pixel_types_match_reference_lanczos_pixels_and_color_space() {
    let source = patterned(37, 29);
    let variants = vec![
        DynamicImage::ImageLuma8(source.to_luma8()),
        DynamicImage::ImageLumaA8(source.to_luma_alpha8()),
        DynamicImage::ImageRgb8(source.to_rgb8()),
        DynamicImage::ImageRgba8(source.to_rgba8()),
        DynamicImage::ImageLuma16(source.to_luma16()),
        DynamicImage::ImageLumaA16(source.to_luma_alpha16()),
        DynamicImage::ImageRgb16(source.to_rgb16()),
        DynamicImage::ImageRgba16(source.to_rgba16()),
        DynamicImage::ImageRgb32F(source.to_rgb32f()),
        DynamicImage::ImageRgba32F(source.to_rgba32f()),
        DynamicImage::ImageRgb32F(image::Rgb32FImage::from_fn(37, 29, |x, y| {
            image::Rgb([
                x as f32 / 12.0 - 0.5,
                y as f32 / 16.0,
                (x + y) as f32 / 40.0,
            ])
        })),
    ];
    for mut source in variants {
        source
            .set_color_space(image::metadata::Cicp::DISPLAY_P3)
            .unwrap();
        for (width, height) in [(19, 13), (1, 8), (17, 1), (55, 43), (37, 29)] {
            let expected = source.resize_exact(width, height, FilterType::Lanczos3);
            let actual = resize(source.clone(), width, height, None).unwrap();
            assert_eq!(actual.color(), expected.color());
            assert_eq!(actual.color_space(), expected.color_space());
            assert_eq!(
                actual.as_bytes(),
                expected.as_bytes(),
                "pixel type {:?}, dimensions {width}x{height}",
                source.color()
            );
        }
    }
}

#[test]
fn document_page_resampling_avoids_full_float_intermediate() {
    let source = patterned(1600, 2263);
    let (expected, old_largest, old_total) =
        measured(|| source.resize_exact(672, 951, FilterType::Lanczos3));
    let owned = source.clone();
    let (actual, new_largest, new_total) = measured(|| resize(owned, 672, 951, None).unwrap());
    assert_eq!(actual.as_bytes(), expected.as_bytes());
    let output_bytes = 672 * 951 * 4;
    assert!(
        old_largest >= 20 * 1024 * 1024,
        "reference no longer materializes the measured float raster"
    );
    assert!(
        new_largest <= output_bytes,
        "intermediate allocation exceeded the final raster: {new_largest}"
    );
    assert!(
        new_total <= output_bytes + 1024 * 1024,
        "row/weight working storage grew with page area: {new_total}"
    );
    println!(
        "Lanczos allocations: old_largest={old_largest},old_total={old_total},new_largest={new_largest},new_total={new_total},output={output_bytes}"
    );
}

#[test]
fn extreme_axes_use_bounded_cache_fallback_without_changing_pixels() {
    for (source, width, height) in [
        (patterned(131_073, 1), 3, 1),
        (patterned(7000, 3), 6900, 2),
        (patterned(1, 131_073), 1, 3),
    ] {
        let expected = source.resize_exact(width, height, FilterType::Lanczos3);
        let actual = resize(source, width, height, None).unwrap();
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
}

#[test]
fn unchanged_dimensions_reuse_storage_and_cancellation_interrupts_resampling() {
    let source = patterned(37, 29);
    let address = source.as_bytes().as_ptr();
    assert_eq!(
        resize(source, 37, 29, None).unwrap().as_bytes().as_ptr(),
        address
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let signal: AttachmentAbort = Arc::new(move || observed.fetch_add(1, Ordering::SeqCst) >= 1200);
    let error = resize(patterned(1600, 2263), 672, 951, Some(&signal)).unwrap_err();
    assert_eq!(error.code, "ATTACHMENT_ABORTED");
    assert_eq!(calls.load(Ordering::SeqCst), 1201);
}
