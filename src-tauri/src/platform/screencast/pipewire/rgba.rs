use std::{num::NonZeroU32, time::Duration};

use image::{imageops, RgbaImage};

use super::{super::CaptureError, backend};

pub(super) const MAX_DIMENSION: u32 = 16384;
const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PixelFormat {
  Bgrx,
  Rgbx,
  Bgra,
  Rgba,
}

#[derive(Clone, Copy)]
pub(super) struct Layout {
  pub width: u32,
  pub height: u32,
  pub offset: usize,
  pub size: usize,
  pub stride: i32,
  pub format: PixelFormat,
  pub premultiplied: bool,
  pub crop: Option<(u32, u32, u32, u32)>,
  pub transform: u32,
}

pub(super) fn dimensions(width: u32, height: u32) -> Result<usize, CaptureError> {
  if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
    return Err(backend("捕获图像尺寸无效或超出限制"));
  }
  let bytes = (width as usize)
    .checked_mul(height as usize)
    .and_then(|pixels| pixels.checked_mul(4))
    .filter(|bytes| *bytes <= MAX_FRAME_BYTES)
    .ok_or_else(|| backend("捕获图像过大"))?;
  Ok(bytes)
}

pub(super) fn convert(bytes: &[u8], layout: Layout) -> Result<RgbaImage, CaptureError> {
  dimensions(layout.width, layout.height)?;
  let row_bytes = layout.width as usize * 4;
  let stride = layout.stride.unsigned_abs() as usize;
  if stride < row_bytes {
    return Err(backend("SPA 行跨度小于图像行宽"));
  }
  let needed = stride
    .checked_mul(layout.height as usize - 1)
    .and_then(|value| value.checked_add(row_bytes))
    .ok_or_else(|| backend("SPA 缓冲区大小溢出"))?;
  let end = layout
    .offset
    .checked_add(layout.size)
    .filter(|end| *end <= bytes.len())
    .ok_or_else(|| backend("SPA chunk 超出缓冲区边界"))?;
  if needed > layout.size {
    return Err(backend("SPA chunk 不足以容纳图像"));
  }
  let (x, y, width, height) = layout.crop.unwrap_or((0, 0, layout.width, layout.height));
  if width == 0
    || height == 0
    || x.checked_add(width).is_none_or(|end| end > layout.width)
    || y.checked_add(height).is_none_or(|end| end > layout.height)
  {
    return Err(backend("SPA 裁剪区域超出图像边界"));
  }
  let mut pixels = Vec::with_capacity(dimensions(width, height)?);
  let bgr = matches!(layout.format, PixelFormat::Bgra | PixelFormat::Bgrx);
  let opaque = matches!(layout.format, PixelFormat::Bgrx | PixelFormat::Rgbx);
  for row in y..y + height {
    // chunk.offset 是有效数据区域起点；负 stride 表示自底向上存储。
    let row = if layout.stride < 0 {
      layout.height - 1 - row
    } else {
      row
    };
    let start = layout.offset + row as usize * stride + x as usize * 4;
    let row = bytes
      .get(start..start + width as usize * 4)
      .filter(|_| start + width as usize * 4 <= end)
      .ok_or_else(|| backend("SPA 图像行超出有效数据区域"))?;
    for pixel in row.chunks_exact(4) {
      let mut rgba = if bgr {
        [pixel[2], pixel[1], pixel[0], pixel[3]]
      } else {
        [pixel[0], pixel[1], pixel[2], pixel[3]]
      };
      if opaque {
        rgba[3] = 255;
      }
      if layout.premultiplied && !opaque {
        let alpha = rgba[3] as u32;
        for channel in &mut rgba[..3] {
          *channel = (*channel as u32 * 255 + alpha / 2)
            .checked_div(alpha)
            .unwrap_or(0)
            .min(255) as u8;
        }
      }
      pixels.extend_from_slice(&rgba);
    }
  }
  let image =
    RgbaImage::from_raw(width, height, pixels).ok_or_else(|| backend("RGBA 图像大小无效"))?;
  // SPA 的旋转为逆时针；imageops 的 rotate90 为顺时针。
  let image = match layout.transform {
    0 => image,
    1 => imageops::rotate270(&image),
    2 => imageops::rotate180(&image),
    3 => imageops::rotate90(&image),
    4 => imageops::flip_horizontal(&image),
    5 => imageops::rotate270(&imageops::flip_horizontal(&image)),
    6 => imageops::rotate180(&imageops::flip_horizontal(&image)),
    7 => imageops::rotate90(&imageops::flip_horizontal(&image)),
    _ => return Err(backend("未知 SPA 图像方向")),
  };
  Ok(image)
}

#[derive(Default)]
pub(super) struct FrameClock {
  anchor: Option<(i64, Duration)>,
  last: Duration,
  next_sequence: u64,
}

impl FrameClock {
  pub fn next(&mut self, pts: Option<i64>, elapsed: Duration) -> (Duration, u64) {
    let timestamp = if let Some(pts) = pts {
      let (origin, offset) = *self.anchor.get_or_insert((pts, elapsed));
      pts
        .checked_sub(origin)
        .filter(|delta| *delta >= 0)
        .map(|delta| offset.saturating_add(Duration::from_nanos(delta as u64)))
        .unwrap_or(elapsed)
    } else {
      elapsed
    };
    self.last = self.last.max(timestamp);
    let sequence = self.next_sequence;
    self.next_sequence = self.next_sequence.saturating_add(1);
    (self.last, sequence)
  }
}

pub(super) struct FrameLimiter {
  interval: Option<Duration>,
  delivered: Option<Duration>,
}

impl FrameLimiter {
  pub fn new(fps: Option<NonZeroU32>) -> Self {
    Self {
      // 向上取整，确保纳秒粒度下不会超过所请求的上限。
      interval: fps.map(|fps| Duration::from_nanos(1_000_000_000u64.div_ceil(fps.get() as u64))),
      delivered: None,
    }
  }

  pub fn accepts(&mut self, elapsed: Duration) -> bool {
    if self
      .interval
      .zip(self.delivered)
      .is_some_and(|(interval, last)| elapsed.saturating_sub(last) < interval)
    {
      return false;
    }
    self.delivered = Some(elapsed);
    true
  }
}
