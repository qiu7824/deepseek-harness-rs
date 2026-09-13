use std::collections::HashMap;
use zsui::{Color, ZsImageFrame, ZsImageFrameId};
pub struct Vector {
    pub id: &'static str,
    pub svg: &'static str,
    pub width: f32,
    pub height: f32,
}
include!("vectors.rs");
#[derive(Default)]
pub struct Assets {
    cache: HashMap<String, (ZsImageFrame, u64)>,
    pub bytes: usize,
    clock: u64,
}
impl Assets {
    pub fn vector(
        &mut self,
        id: &str,
        size: f32,
        scale: f32,
        color: Color,
        dark: bool,
    ) -> Result<ZsImageFrame, String> {
        let a = VECTORS
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("Missing WebUI asset: {id}"))?;
        let h = (size * scale).round().max(1.) as u32;
        let w = ((h as f32) * a.width / a.height).round().max(1.) as u32;
        let key = format!("{id}:{w}:{h}:{}:{}:{}:{dark}", color.r, color.g, color.b);
        self.clock += 1;
        if let Some((f, t)) = self.cache.get_mut(&key) {
            *t = self.clock;
            return Ok(f.clone());
        }
        if u64::from(w) * u64::from(h) * 4 > 8 * 1024 * 1024 {
            return Err("SVG exceeds 8 MiB".into());
        }
        let source = a
            .svg
            .replace(
                "currentColor",
                &format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b),
            )
            .replace(
                "var(--dsw-alias-label-primary-inverted)",
                if dark { "#151517" } else { "#ffffff" },
            );
        let tree = resvg::usvg::Tree::from_str(&source, &resvg::usvg::Options::default())
            .map_err(|e| format!("{id}: {e}"))?;
        let mut p = resvg::tiny_skia::Pixmap::new(w, h).ok_or("SVG allocation")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(
                w as f32 / tree.size().width(),
                h as f32 / tree.size().height(),
            ),
            &mut p.as_mut(),
        );
        let mut bytes = p.take();
        for px in bytes.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let f =
            ZsImageFrame::from_premultiplied_bgra8(ZsImageFrameId::new(self.clock), w, h, bytes)
                .map_err(|e| e.to_string())?;
        self.insert(key, f.clone());
        Ok(f)
    }
    fn insert(&mut self, key: String, f: ZsImageFrame) {
        while self.bytes + f.decoded_bytes() > 8 * 1024 * 1024 {
            let Some(k) = self
                .cache
                .iter()
                .min_by_key(|(_, (_, t))| *t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((old, _)) = self.cache.remove(&k) {
                self.bytes -= old.decoded_bytes();
            }
        }
        self.bytes += f.decoded_bytes();
        self.cache.insert(key, (f, self.clock));
    }
    pub fn local_image(
        &mut self,
        path: &std::path::Path,
        width: f32,
        scale: f32,
    ) -> Result<ZsImageFrame, String> {
        let w = (width * scale).round().clamp(1., 2048.) as u32;
        let key = format!("{}:{w}", path.display());
        self.clock += 1;
        if let Some((f, t)) = self.cache.get_mut(&key) {
            *t = self.clock;
            return Ok(f.clone());
        }
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() > 16 * 1024 * 1024 {
            return Err("图片文件超过 16 MiB".into());
        }
        let mut r = image::ImageReader::open(path)
            .map_err(|e| e.to_string())?
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let mut limit = image::Limits::default();
        limit.max_image_width = Some(8192);
        limit.max_image_height = Some(8192);
        limit.max_alloc = Some(64 * 1024 * 1024);
        r.limits(limit);
        let decoded = r.decode().map_err(|e| e.to_string())?;
        let max_w = w.min((decoded.width() as f32 * scale).round() as u32);
        let max_h = 1024.min((decoded.height() as f32 * scale).round() as u32);
        let img = decoded.thumbnail(max_w.max(1), max_h.max(1)).to_rgba8();
        let f = ZsImageFrame::from_rgba8(
            ZsImageFrameId::new(self.clock),
            img.width(),
            img.height(),
            img.into_raw(),
        )
        .map_err(|e| e.to_string())?;
        if f.decoded_bytes() > 8 * 1024 * 1024 {
            return Err("图片解码超过缓存预算".into());
        }
        self.insert(key, f.clone());
        Ok(f)
    }
}
