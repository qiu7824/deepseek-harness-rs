//! Bounded native raster surfaces using the framework text shaper.
use crate::rust_text_renderer::{ZsRustTextEngine, ZsTextPixelRect, ZsTextRasterProfile};
use crate::{Color, TextStyle, ZsImageFrame, ZsImageFrameId};

/// Application-owned painter. It shares the framework's Unicode shaping and font fallback.
pub struct SurfacePainter {
    pub text: ZsRustTextEngine,
    revision: u64,
}
impl Default for SurfacePainter {
    fn default() -> Self {
        Self {
            text: ZsRustTextEngine::new()
                .with_layout_cache_limits(256, 4 * 1024 * 1024)
                .with_glyph_cache_byte_limit(4 * 1024 * 1024),
            revision: 0,
        }
    }
}
impl SurfacePainter {
    pub fn measure(&mut self, text: &str, style: &TextStyle, width: f32, scale: f32) -> f32 {
        let layout = self
            .text
            .layout(text, style, (width * scale).ceil() as i32, 0, scale);
        (layout.size().height as f32 / scale).max(style.line_height)
    }
    pub fn width(&mut self, text: &str, style: &TextStyle, scale: f32) -> f32 {
        self.text.layout(text, style, 0, 0, scale).size().width as f32 / scale
    }
    pub fn frame(&mut self, raster: RasterSurface) -> Result<ZsImageFrame, String> {
        self.revision = self.revision.checked_add(1).ok_or("frame ID overflow")?;
        ZsImageFrame::from_premultiplied_bgra8(
            ZsImageFrameId::new(self.revision),
            raster.pixmap.width(),
            raster.pixmap.height(),
            raster.pixmap.take(),
        )
        .map_err(|e| e.to_string())
    }
    pub fn text(
        &mut self,
        raster: &mut RasterSurface,
        value: &str,
        style: &TextStyle,
        x: f32,
        y: f32,
        width: f32,
        clip: [f32; 4],
    ) {
        let layout = self.text.layout(
            value,
            style,
            (width * raster.scale).ceil() as i32,
            0,
            raster.scale,
        );
        let w = raster.pixmap.width();
        let h = raster.pixmap.height();
        let s = raster.scale;
        self.text
            .composite_bgra_clipped(
                &layout,
                raster.pixmap.data_mut(),
                w,
                h,
                w as usize * 4,
                (x * s).round() as i32,
                (y * s).round() as i32,
                ZsTextPixelRect::new(
                    (clip[0] * s).round() as i32,
                    (clip[1] * s).round() as i32,
                    (clip[2].max(0.) * s).round() as u32,
                    (clip[3].max(0.) * s).round() as u32,
                ),
                style.color,
                ZsTextRasterProfile::grayscale(),
            )
            .expect("validated surface buffer");
    }
}

/// One BGRA buffer, bounded independently from retained text and image caches.
pub struct RasterSurface {
    pixmap: tiny_skia::Pixmap,
    pub scale: f32,
}
impl RasterSurface {
    pub fn new(width: f32, height: f32, scale: f32, color: Color) -> Result<Self, String> {
        if !width.is_finite() || !height.is_finite() || !scale.is_finite() || scale <= 0. {
            return Err("invalid surface dimensions".into());
        }
        let w = (width * scale).ceil().max(1.) as u32;
        let h = (height * scale).ceil().max(1.) as u32;
        if u64::from(w)
            .checked_mul(u64::from(h))
            .and_then(|pixels| pixels.checked_mul(4))
            .is_none_or(|bytes| bytes > 64 * 1024 * 1024)
        {
            return Err("surface exceeds 64 MiB".into());
        }
        let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("surface allocation failed")?;
        pixmap.fill(tiny_skia::Color::from_rgba8(
            color.b, color.g, color.r, color.a,
        ));
        Ok(Self { pixmap, scale })
    }
    pub fn rect(&mut self, b: [f32; 4], radius: f32, color: Color) {
        let [x, y, w, h] = b.map(|v| v * self.scale);
        if w <= 0. || h <= 0. {
            return;
        }
        let r = (radius * self.scale).max(0.).min(w / 2.).min(h / 2.);
        let mut path = tiny_skia::PathBuilder::new();
        path.move_to(x + r, y);
        path.line_to(x + w - r, y);
        path.quad_to(x + w, y, x + w, y + r);
        path.line_to(x + w, y + h - r);
        path.quad_to(x + w, y + h, x + w - r, y + h);
        path.line_to(x + r, y + h);
        path.quad_to(x, y + h, x, y + h - r);
        path.line_to(x, y + r);
        path.quad_to(x, y, x + r, y);
        path.close();
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(color.b, color.g, color.r, color.a);
        if let Some(path) = path.finish() {
            self.pixmap.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
    pub fn image(&mut self, frame: &ZsImageFrame, x: f32, y: f32) {
        if let Some(pixmap) = tiny_skia::PixmapRef::from_bytes(
            frame.premultiplied_bgra8(),
            frame.width(),
            frame.height(),
        ) {
            self.pixmap.draw_pixmap(
                (x * self.scale).round() as i32,
                (y * self.scale).round() as i32,
                pixmap,
                &tiny_skia::PixmapPaint::default(),
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
    pub fn image_clipped(&mut self, frame: &ZsImageFrame, x: f32, y: f32, clip: [f32; 4]) {
        let px = (x * self.scale).round() as i32;
        let py = (y * self.scale).round() as i32;
        let x0 = (clip[0] * self.scale).floor().max(0.) as i32;
        let y0 = (clip[1] * self.scale).floor().max(0.) as i32;
        let x1 = ((clip[0] + clip[2]) * self.scale)
            .ceil()
            .min(self.pixmap.width() as f32) as i32;
        let y1 = ((clip[1] + clip[3]) * self.scale)
            .ceil()
            .min(self.pixmap.height() as f32) as i32;
        let stride = self.pixmap.width() as usize * 4;
        let target = self.pixmap.data_mut();
        let source = frame.premultiplied_bgra8();
        for dy in py.max(y0)..py.saturating_add(frame.height() as i32).min(y1) {
            for dx in px.max(x0)..px.saturating_add(frame.width() as i32).min(x1) {
                let a = ((dy - py) as usize * frame.width() as usize + (dx - px) as usize) * 4;
                let b = dy as usize * stride + dx as usize * 4;
                let alpha = source[a + 3] as u32;
                for c in 0..4 {
                    target[b + c] = (source[a + c] as u32
                        + (target[b + c] as u32 * (255 - alpha) + 127) / 255)
                        .min(255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_and_excessive_surfaces() {
        assert!(RasterSurface::new(f32::NAN, 20., 1., Color::rgb(0, 0, 0)).is_err());
        assert!(RasterSurface::new(9000., 9000., 1., Color::rgb(0, 0, 0)).is_err());
        assert!(RasterSurface::new(f32::MAX, f32::MAX, 2., Color::rgb(0, 0, 0)).is_err());
    }
    #[test]
    fn alpha_and_channel_order_are_preserved() {
        let mut p = SurfacePainter::default();
        let mut r = RasterSurface::new(8., 8., 1., Color::rgb(255, 255, 255)).unwrap();
        r.rect([0., 0., 8., 8.], 0., Color::rgb(250, 10, 20));
        let f = p.frame(r).unwrap();
        assert_eq!(&f.premultiplied_bgra8()[..4], &[20, 10, 250, 255]);
    }
    #[test]
    fn image_clip_does_not_paint_outside_bounds() {
        let mut p = SurfacePainter::default();
        let f = ZsImageFrame::from_rgba8(ZsImageFrameId::new(1), 2, 2, vec![255; 16]).unwrap();
        let mut r = RasterSurface::new(4., 4., 1., Color::rgb(0, 0, 0)).unwrap();
        r.image_clipped(&f, 1., 1., [0., 0., 2., 2.]);
        r.image_clipped(&f, f32::MAX, f32::MAX, [0., 0., 4., 4.]);
        let f = p.frame(r).unwrap();
        assert_eq!(&f.premultiplied_bgra8()[20..24], &[255; 4]);
        assert_eq!(&f.premultiplied_bgra8()[24..28], &[0, 0, 0, 255]);
    }
}
