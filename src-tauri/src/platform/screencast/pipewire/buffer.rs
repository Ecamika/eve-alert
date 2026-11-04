use image::RgbaImage;
use pipewire::{
  buffer::Buffer,
  spa::{
    self,
    buffer::{
      DataType,
      meta::{MetaHeader, MetaHeaderFlags, MetaVideoCrop, MetaVideoTransform},
    },
    param::video::{VideoFormat, VideoInfoRaw},
  },
};

use super::{
  super::CaptureError,
  backend,
  rgba::{self, Layout, PixelFormat},
};

pub(super) fn validate_format(format: VideoInfoRaw) -> Result<(), CaptureError> {
  if ![
    VideoFormat::BGRx,
    VideoFormat::RGBx,
    VideoFormat::BGRA,
    VideoFormat::RGBA,
  ]
  .contains(&format.format())
  {
    return Err(backend("只支持 CPU RGBA/BGRA/RGBx/BGRx 格式"));
  }
  // 未声明色彩属性的桌面 SDR 流按 sRGB 解释。显式 HDR、非 sRGB 色域、
  // 有限范围、隔行扫描和 GPU modifier 均拒绝，避免错误标记为 sRGB。
  if ![
    spa::sys::SPA_VIDEO_TRANSFER_UNKNOWN,
    spa::sys::SPA_VIDEO_TRANSFER_SRGB,
  ]
  .contains(&format.transfer_function())
    || ![
      spa::sys::SPA_VIDEO_COLOR_PRIMARIES_UNKNOWN,
      spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT709,
    ]
    .contains(&format.color_primaries())
    || ![
      spa::sys::SPA_VIDEO_COLOR_RANGE_UNKNOWN,
      spa::sys::SPA_VIDEO_COLOR_RANGE_0_255,
    ]
    .contains(&format.color_range())
    || ![
      spa::sys::SPA_VIDEO_COLOR_MATRIX_UNKNOWN,
      spa::sys::SPA_VIDEO_COLOR_MATRIX_RGB,
    ]
    .contains(&format.color_matrix())
    || format.interlace_mode().as_raw() != spa::sys::SPA_VIDEO_INTERLACE_MODE_PROGRESSIVE
    || format.flags().bits() & spa::sys::SPA_VIDEO_FLAG_MODIFIER != 0
  {
    return Err(backend("不支持该色彩空间、HDR、隔行扫描或 GPU modifier"));
  }
  rgba::dimensions(format.size().width, format.size().height)?;
  Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct PreparedFrame {
  layout: Layout,
  pub pts: Option<i64>,
}

pub(super) fn prepare_frame(
  buffer: &mut Buffer<'_>,
  format: VideoInfoRaw,
) -> Result<Option<PreparedFrame>, CaptureError> {
  let header = buffer.find_meta::<MetaHeader>();
  if header.is_some_and(|header| {
    header
      .flags()
      .intersects(MetaHeaderFlags::CORRUPTED | MetaHeaderFlags::GAP)
  }) {
    return Ok(None);
  }
  let pts = header.map(MetaHeader::pts).filter(|pts| *pts != i64::MIN);
  let crop = buffer
    .find_meta::<MetaVideoCrop>()
    .filter(|crop| crop.meta_region().is_valid())
    .map(|crop| {
      let region = crop.meta_region();
      let position = region.position();
      if position.x < 0 || position.y < 0 {
        return Err(backend("SPA 裁剪坐标为负"));
      }
      Ok((
        position.x as u32,
        position.y as u32,
        region.size().width,
        region.size().height,
      ))
    })
    .transpose()?;
  let transform = buffer
    .find_meta::<MetaVideoTransform>()
    .map_or(0, |meta| meta.transform().as_raw());
  let datas = buffer.datas_mut();
  if datas.len() != 1 {
    return Err(backend("只支持单平面 CPU 缓冲区"));
  }
  let data = &mut datas[0];
  if !matches!(data.type_(), DataType::MemPtr | DataType::MemFd) || data.as_raw().chunk.is_null() {
    return Err(backend("PipeWire 未提供可读的 CPU 缓冲区"));
  }
  if data.chunk().size() == 0
    || data.chunk().as_raw().flags as u32 & spa::sys::SPA_CHUNK_FLAG_EMPTY != 0
  {
    return Ok(None);
  }
  if data
    .chunk()
    .flags()
    .contains(spa::buffer::ChunkFlags::CORRUPTED)
  {
    return Ok(None);
  }
  let layout = Layout {
    width: format.size().width,
    height: format.size().height,
    offset: data.chunk().offset() as usize,
    size: data.chunk().size() as usize,
    stride: data.chunk().stride(),
    format: match format.format() {
      VideoFormat::BGRx => PixelFormat::Bgrx,
      VideoFormat::RGBx => PixelFormat::Rgbx,
      VideoFormat::BGRA => PixelFormat::Bgra,
      VideoFormat::RGBA => PixelFormat::Rgba,
      _ => return Err(backend("不支持的像素格式")),
    },
    premultiplied: format.flags().bits() & spa::sys::SPA_VIDEO_FLAG_PREMULTIPLIED_ALPHA != 0,
    crop,
    transform,
  };
  let bytes = data
    .data()
    .ok_or_else(|| backend("PipeWire 未映射共享内存缓冲区"))?;
  rgba::validate_layout(bytes.len(), layout)?;
  Ok(Some(PreparedFrame { layout, pts }))
}

pub(super) fn copy_frame(
  buffer: &mut Buffer<'_>,
  prepared: PreparedFrame,
) -> Result<RgbaImage, CaptureError> {
  let datas = buffer.datas_mut();
  let bytes = datas[0]
    .data()
    .ok_or_else(|| backend("PipeWire 未映射共享内存缓冲区"))?;
  // MAP_BUFFERS 已处理 MemFd 的 mapoffset。必须在 Buffer Drop 归还前完成复制。
  rgba::convert(bytes, prepared.layout)
}
