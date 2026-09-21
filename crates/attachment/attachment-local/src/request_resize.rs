//! Separable Lanczos-3 with row-sized intermediate storage.
//! Integer rounding, kernel phase and clipping match image's Lanczos3 resize.

use dsh_attachment::{AttachmentAbort, AttachmentError};
use image::{DynamicImage, ImageBuffer, Pixel};

const ROW_BUDGET: usize = 2 * 1024 * 1024;
const AXIS_BUDGET: usize = 256 * 1024;
const WEIGHT_BUDGET: usize = 64 * 1024;

fn cancelled(signal: Option<&AttachmentAbort>) -> Result<(), AttachmentError> {
    if signal.is_some_and(|signal| signal()) {
        Err(AttachmentError::new(
            "ATTACHMENT_ABORTED",
            "attachment read cancelled",
        ))
    } else {
        Ok(())
    }
}

fn sinc(value: f32) -> f32 {
    if value == 0.0 {
        1.0
    } else {
        let angle = value * std::f32::consts::PI;
        angle.sin() / angle
    }
}
fn lanczos(value: f32) -> f32 {
    if value.abs() < 3.0 {
        sinc(value) * sinc(value / 3.0)
    } else {
        0.0
    }
}

struct Kernel {
    start: u32,
    end: u32,
    center: f32,
    scale: f32,
    sum: f32,
    weights: Vec<f32>,
}
impl Kernel {
    fn new(
        index: u32,
        source: u32,
        destination: u32,
        signal: Option<&AttachmentAbort>,
    ) -> Result<Self, AttachmentError> {
        let ratio = source as f32 / destination as f32;
        let scale = ratio.max(1.0);
        let midpoint = (index as f32 + 0.5) * ratio;
        let start =
            ((midpoint - 3.0 * scale).floor() as i64).clamp(0, i64::from(source) - 1) as u32;
        let end = ((midpoint + 3.0 * scale).ceil() as i64)
            .clamp(i64::from(start) + 1, i64::from(source)) as u32;
        let center = midpoint - 0.5;
        let cached = u64::from(end - start) * 4 <= WEIGHT_BUDGET as u64;
        let mut weights = if cached {
            Vec::with_capacity((end - start) as usize)
        } else {
            Vec::new()
        };
        let mut sum = 0.0;
        for position in start..end {
            if (position - start) % 4096 == 0 {
                cancelled(signal)?;
            }
            let weight = lanczos((position as f32 - center) / scale);
            sum += weight;
            if cached {
                weights.push(weight);
            }
        }
        for weight in &mut weights {
            *weight /= sum;
        }
        Ok(Self {
            start,
            end,
            center,
            scale,
            sum,
            weights,
        })
    }
    fn weight(&self, position: u32) -> f32 {
        if self.weights.is_empty() {
            lanczos((position as f32 - self.center) / self.scale) / self.sum
        } else {
            self.weights[(position - self.start) as usize]
        }
    }
}

trait Sample: image::Primitive {
    fn float(self) -> f32;
    fn sampled(value: f32) -> Self;
}
impl Sample for u8 {
    fn float(self) -> f32 {
        self as f32
    }
    fn sampled(value: f32) -> Self {
        value.clamp(0.0, 255.0).round() as Self
    }
}
impl Sample for u16 {
    fn float(self) -> f32 {
        self as f32
    }
    fn sampled(value: f32) -> Self {
        value.clamp(0.0, 65535.0).round() as Self
    }
}
impl Sample for f32 {
    fn float(self) -> f32 {
        self
    }
    fn sampled(value: f32) -> Self {
        value.clamp(0.0, 1.0)
    }
}

fn vertical<P: Pixel + 'static>(
    source: &ImageBuffer<P, Vec<P::Subpixel>>,
    x: u32,
    kernel: &Kernel,
    signal: Option<&AttachmentAbort>,
) -> Result<[f32; 4], AttachmentError>
where
    P::Subpixel: Sample,
{
    let mut result = [0.0; 4];
    for y in kernel.start..kernel.end {
        if (y - kernel.start) % 4096 == 0 {
            cancelled(signal)?;
        }
        let weight = kernel.weight(y);
        let pixel = source.get_pixel(x, y).channels();
        for channel in 0..P::CHANNEL_COUNT as usize {
            result[channel] += pixel[channel].float() * weight;
        }
    }
    Ok(result)
}

fn resize_buffer<P: Pixel + 'static>(
    source: ImageBuffer<P, Vec<P::Subpixel>>,
    width: u32,
    height: u32,
    signal: Option<&AttachmentAbort>,
) -> Result<ImageBuffer<P, Vec<P::Subpixel>>, AttachmentError>
where
    P::Subpixel: Sample,
{
    cancelled(signal)?;
    if source.dimensions() == (width, height) {
        return Ok(source);
    }
    let mut output = ImageBuffer::<P, Vec<P::Subpixel>>::new(width, height);
    output
        .set_color_space(source.color_space())
        .map_err(|error| {
            AttachmentError::new("REQUEST_IMAGE_TRANSFORM_FAILED", error.to_string())
        })?;
    if source.width() == 0 || source.height() == 0 || width == 0 || height == 0 {
        return Ok(output);
    }
    let estimate = (width as u64).saturating_mul(
        std::mem::size_of::<Kernel>() as u64
            + ((6.0 * (source.width() as f32 / width as f32).max(1.0)).ceil() as u64 + 2) * 4,
    );
    let columns = if estimate <= AXIS_BUDGET as u64 {
        let mut columns = Vec::with_capacity(width as usize);
        for x in 0..width {
            columns.push(Kernel::new(x, source.width(), width, signal)?);
        }
        Some(columns)
    } else {
        None
    };
    let mut row = if u64::from(source.width()) * 16 <= ROW_BUDGET as u64 {
        Some(vec![[0.0_f32; 4]; source.width() as usize])
    } else {
        None
    };
    for y in 0..height {
        cancelled(signal)?;
        let rows = Kernel::new(y, source.height(), height, signal)?;
        if let Some(row) = row.as_mut() {
            for x in 0..source.width() {
                if x % 128 == 0 {
                    cancelled(signal)?;
                }
                row[x as usize] = vertical(&source, x, &rows, signal)?;
            }
        }
        for x in 0..width {
            if x % 64 == 0 {
                cancelled(signal)?;
            }
            let uncached;
            let column = match &columns {
                Some(columns) => &columns[x as usize],
                None => {
                    uncached = Kernel::new(x, source.width(), width, signal)?;
                    &uncached
                }
            };
            let mut result = [0.0_f32; 4];
            for source_x in column.start..column.end {
                if (source_x - column.start) % 4096 == 0 {
                    cancelled(signal)?;
                }
                let sample = match &row {
                    Some(row) => row[source_x as usize],
                    None => vertical(&source, source_x, &rows, signal)?,
                };
                let weight = column.weight(source_x);
                for channel in 0..P::CHANNEL_COUNT as usize {
                    result[channel] += sample[channel] * weight;
                }
            }
            for (target, value) in output
                .get_pixel_mut(x, y)
                .channels_mut()
                .iter_mut()
                .zip(result)
            {
                *target = P::Subpixel::sampled(value);
            }
        }
    }
    Ok(output)
}

pub(crate) fn resize(
    image: DynamicImage,
    width: u32,
    height: u32,
    signal: Option<&AttachmentAbort>,
) -> Result<DynamicImage, AttachmentError> {
    macro_rules! resize {
        ($buffer:ident, $variant:ident) => {
            resize_buffer($buffer, width, height, signal).map(DynamicImage::$variant)
        };
    }
    match image {
        DynamicImage::ImageLuma8(buffer) => resize!(buffer, ImageLuma8),
        DynamicImage::ImageLumaA8(buffer) => resize!(buffer, ImageLumaA8),
        DynamicImage::ImageRgb8(buffer) => resize!(buffer, ImageRgb8),
        DynamicImage::ImageRgba8(buffer) => resize!(buffer, ImageRgba8),
        DynamicImage::ImageLuma16(buffer) => resize!(buffer, ImageLuma16),
        DynamicImage::ImageLumaA16(buffer) => resize!(buffer, ImageLumaA16),
        DynamicImage::ImageRgb16(buffer) => resize!(buffer, ImageRgb16),
        DynamicImage::ImageRgba16(buffer) => resize!(buffer, ImageRgba16),
        DynamicImage::ImageRgb32F(buffer) => resize!(buffer, ImageRgb32F),
        DynamicImage::ImageRgba32F(buffer) => resize!(buffer, ImageRgba32F),
        other => {
            cancelled(signal)?;
            Ok(other.resize_exact(width, height, image::imageops::FilterType::Lanczos3))
        }
    }
}

#[cfg(test)]
#[path = "request_resize_tests.rs"]
mod tests;
