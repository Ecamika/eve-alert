use futures_util::stream::BoxStream;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{fmt, num::NonZeroU32, sync::Arc, time::Duration};

use super::CaptureError;

/// 后端实例内的不透明标识，不应解析为窗口句柄或 PipeWire node ID。
///
/// 不承诺跨后端、跨重启或跨授权会话有效，也不是持久授权凭据。
/// 来源消失后 ID 必须失效，不能因原生 ID 复用而捕获另一个来源。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
  Display,
  Window,
}

/// 来源的元数据快照，尺寸可能动态变化，因此由每一帧的图像携带。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSource {
  /// 平台提供的来源显示名称快照，不保证等同于窗口的真实标题。
  /// 平台没有提供可用名称时为空，不用生成标签或原生 ID 代替。
  pub title: String,
  pub id: SourceId,
  pub kind: SourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionKind {
  Display,
  Window,
  /// 允许选择 Display 或 Window；数量由 `CaptureMultiplicity` 决定。
  Any,
}

/// 系统选源或恢复时允许选择的来源数量，不控制图像合成。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureMultiplicity {
  /// 必须恰好选择一个来源。
  #[default]
  Single,
  /// 允许一次选择一个或多个来源，不保证用户一定选择多个。
  Multiple,
}

/// 后端提供的可序列化恢复凭据，不持有原生资源，也不是 `SourceId`。
///
/// 调用方只负责保存并传回，不能解析、修改内容或跨平台使用。
/// 凭据可能一次性有效；克隆及反序列化不会产生新的使用次数。
/// 同一凭据的恢复请求必须串行，启动后立即保存新凭据，不能重用旧副本。
/// `Debug` 隐藏负载，但序列化结果包含凭据，不应写入日志。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRestoreToken {
  backend: String,
  format_version: u32,
  payload: Vec<u8>,
}

impl CaptureRestoreToken {
  pub(crate) fn new(backend: impl Into<String>, format_version: u32, payload: Vec<u8>) -> Self {
    Self {
      backend: backend.into(),
      format_version,
      payload,
    }
  }

  pub(crate) fn payload_for(&self, backend: &str, format_version: u32) -> Option<&[u8]> {
    (self.backend == backend && self.format_version == format_version)
      .then_some(self.payload.as_slice())
  }
}

impl fmt::Debug for CaptureRestoreToken {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("CaptureRestoreToken")
      .field("backend", &self.backend)
      .field("format_version", &self.format_version)
      .field("payload", &"<redacted>")
      .finish()
  }
}

/// 请求的授权持久化范围；支持请求不表示系统或用户一定批准。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapturePersistence {
  #[default]
  None,
  /// 仅在应用运行期间恢复；凭据不应跨应用重启保存。
  Application,
  /// 允许跨应用重启恢复，直到授权撤销或凭据失效。
  UntilRevoked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureTarget {
  /// 要求使用系统选择器；不支持时不能用自制选择器替代。
  Picker(SelectionKind),
  /// 使用同一后端实例提供且仍然有效的 ID。
  Source(SourceId),
  /// 尝试恢复先前选择，无法恢复时允许系统提示用户重新选择。
  ///
  /// 两种路径均须满足 `kind`；不保证免交互，也不复用原生捕获会话。
  /// 后端或格式不匹配、内容损坏返回 `InvalidRestoreToken`。
  RestoreOrPicker {
    token: CaptureRestoreToken,
    kind: SelectionKind,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CursorMode {
  Hidden,
  Embedded,
}

/// 当前后端的能力快照，每个列表为空表示不支持对应操作。
///
/// 列表应无重复项；枚举到的来源类型必须同时支持直接选源。
/// `Any` 必须显式出现在 `picker_kinds` 中，不能由其他选项隐式推断。
/// 例如只支持不带类型过滤的选择器可仅声明 `Any`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureCapabilities {
  pub enumerable_source_kinds: Vec<SourceKind>,
  pub direct_source_kinds: Vec<SourceKind>,
  pub picker_kinds: Vec<SelectionKind>,
  /// 支持一次多选的系统选择器类型过滤，是 `picker_kinds` 的子集。
  pub multiple_picker_kinds: Vec<SelectionKind>,
  /// 后端能够保证的最终图像光标行为，含必要的平台处理。
  pub cursor_modes: Vec<CursorMode>,
  /// 允许恢复或重新选择的类型过滤；为空表示不支持恢复凭据。
  pub restore_kinds: Vec<SelectionKind>,
  /// 支持恢复多来源选择的类型过滤，是 `restore_kinds` 的子集。
  pub multiple_restore_kinds: Vec<SelectionKind>,
  /// 支持请求的持久化范围，含支持的 `None`；不代表授权已获批准。
  pub persistence_modes: Vec<CapturePersistence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureOptions {
  pub target: CaptureTarget,
  pub cursor: CursorMode,
  /// 每个来源交付帧率的上限，`None` 表示不额外限速；不是固定帧率保证。
  ///
  /// 后端可通过原生配置或丢帧限速，无需生成重复帧；来源静止时允许无新帧。
  pub max_fps: Option<NonZeroU32>,
  /// 本次会话请求的持久化范围，实际凭据由会话返回。
  pub persistence: CapturePersistence,
  /// 系统选择器及恢复请求允许的来源数量；直接 `Source` 仍指定一个来源。
  pub multiplicity: CaptureMultiplicity,
}

impl CaptureOptions {
  /// 保留现有构造形式；`None` 与 `Some(0)` 均表示不额外限速。
  pub fn new(target: CaptureTarget, cursor: CursorMode, max_fps: Option<u32>) -> Self {
    Self {
      target,
      cursor,
      max_fps: max_fps.and_then(NonZeroU32::new),
      persistence: CapturePersistence::None,
      multiplicity: CaptureMultiplicity::Single,
    }
  }

  /// 设置持久化请求；是否获得恢复凭据需检查会话实际返回值。
  pub fn with_persistence(mut self, persistence: CapturePersistence) -> Self {
    self.persistence = persistence;
    self
  }

  /// 设置一次选源允许的来源数量，默认单选。
  pub fn with_multiplicity(mut self, multiplicity: CaptureMultiplicity) -> Self {
    self.multiplicity = multiplicity;
    self
  }
}

/// 脱离原生缓冲区生命周期的 CPU 图像帧。
#[derive(Debug)]
pub struct CaptureFrame {
  /// 本帧所属来源，必定出现在当前会话的 `sources()` 中。
  pub source_id: SourceId,
  /// 紧密排列、从左上角逐行存储的 RGBA8 图像，使用 sRGB 与非预乘 alpha。
  ///
  /// 后端负责格式、方向、行跨度、有效内容区域与色彩转换；HDR 输入需映射到
  /// SDR，无法满足时返回错误。图像尺寸为实际像素尺寸，可随帧变化。
  /// 返回前必须取得自有像素数据，不能继续借用已归还的原生缓冲区。
  pub image: RgbaImage,
  /// 相对本会话开始捕获时刻的单调时间，不是 UNIX 时间或跨会话时钟。
  ///
  /// 优先归一化原生帧时间；不可用时采用收到帧时的单调时钟。
  /// 同一来源内不倒退，首帧不要求为零；多来源不保证同步或交付时间顺序。
  pub timestamp: Duration,
  /// 每个来源分别从零开始，在统一帧生成时递增；限速或覆盖缓存可能使序号出现空洞。
  pub sequence: u64,
}

pub type ArcCaptureFrame = Arc<CaptureFrame>;

/// 单个来源唯一消费者的异步帧流，使用 `Arc` 共享像素而无需复制图像。
///
/// 每路流最多保留一个待消费图像，新帧覆盖旧帧；来源之间不共享队列、缓存或唤醒器。
/// 消费者缓慢或尚未取出流时不能无限积压，也不能阻塞原生捕获回调。
/// 异常终止先丢弃待消费图像，再交付一次 `Err`，随后永久结束；终止错误
/// 不能被图像覆盖。来源异常只终止本路捕获；正常停止直接结束。
/// 不对静止、遮挡或最小化来源保证帧间隔。
/// 会话异常在每路流各交付一次终止错误；轮询某一路不能取走其他路的错误。
/// 已结束的流重复轮询应继续返回 `None`。丢弃流只请求停止该来源，
/// 会话仍存在且其他来源仍活动时继续捕获；全部来源停止后清理整组。
pub type FrameStream = BoxStream<'static, Result<ArcCaptureFrame, CaptureError>>;
