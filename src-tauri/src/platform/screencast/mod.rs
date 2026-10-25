//! 跨平台屏幕捕获契约：后端负责选源和启动，会话负责帧流与资源生命周期。
//!
//! 通用接口仅暴露自有数据与控制句柄，不暴露 PipeWire、Direct3D 或
//! ScreenCaptureKit 原生对象，也不依赖 Tauri。平台映射见本目录 README.md。

mod error;
mod types;

#[cfg(target_os = "linux")]
pub mod pipewire;

pub use error::*;
pub use types::*;

use futures_util::future::BoxFuture;

/// 可跨线程调度、支持动态分派的异步捕获操作。
pub type CaptureFuture<'a, T> = BoxFuture<'a, Result<T, CaptureError>>;

/// 平台捕获后端，可通过 `Arc<dyn Screencast>` 在业务服务之间共享。
///
/// `Send + Sync` 约束的是控制句柄。实现应将不满足这些约束的原生对象
/// 留在所属线程，通过消息传递操作；系统 UI 则调度到平台要求的线程。
/// 宿主窗口、UI 调度器等在平台后端创建时注入，不进入通用选项。
pub trait Screencast: Send + Sync {
  /// 查询当前后端的能力快照；允许进行平台异步查询，但不弹出选源界面。
  ///
  /// 能力不表示已获授权，也不保证稍后的启动一定成功。
  fn capabilities(&self) -> CaptureFuture<'_, CaptureCapabilities>;

  /// 枚举后端能够列出的来源，不打开选源界面；可能触发系统权限请求。
  ///
  /// 只返回 `enumerable_source_kinds` 中的类型。后端完全不支持枚举时返回
  /// `Unsupported(SourceEnumeration)`；支持枚举但无来源时返回空列表。
  /// 权限不足返回 `PermissionDenied`，不能用空列表掩盖该错误。
  fn sources(&self) -> CaptureFuture<'_, Vec<CaptureSource>>;

  /// 完成一次授权与选源，返回所有选中来源均已启动的捕获会话。
  ///
  /// `Picker` 使用系统选择器；`Source` 仅接受同一后端实例提供的 ID。
  /// `RestoreOrPicker` 尝试恢复旧选择，允许恢复失败时打开系统选择器。
  /// 后端必须遵守来源类型与光标选项，不支持时返回 `Unsupported`，
  /// 不能静默修改请求或从直接选源回退到选择器。用户取消返回 `Cancelled`。
  /// 不保证返回时已有首帧；来源尺寸从收到的每一帧读取。
  /// `Single` 必须恰好返回一个来源，`Multiple` 允许一个或多个来源；
  /// 不支持多选时返回 `Unsupported`，不能多次弹出单选界面来模拟一次多选。
  /// 任一来源启动失败时清理整组资源，不能悄悄返回部分成功的来源。
  ///
  /// 启动失败或 future 被丢弃时，必须释放已创建的资源、取消可取消的请求，
  /// 并丢弃不可取消请求的迟到结果，不能留下无人持有的捕获会话。
  /// 恢复凭据可能在启动完成前已被消耗；失败或取消不保证旧凭据仍有效。
  /// 成功后立即取出并保存新凭据；未返回新凭据时不能继续重用旧凭据。
  fn start(&self, options: CaptureOptions) -> CaptureFuture<'_, Box<dyn CaptureSession>>;
}

/// 一次授权下的一组来源的捕获控制句柄，与帧流分离以允许等待帧时停止捕获。
///
/// 会话不可重新启动或更换来源；重新选择需要创建新会话。
/// 停止或共享会话异常时结束整组捕获；来源异常或丢弃单路帧流只停止该来源。
/// 丢弃会话必须停止全部来源；全部来源均已停止时也必须清理整组原生资源。
/// `Drop` 无法等待异步清理，需要确认整组释放时应等待 `stop`。
pub trait CaptureSession: Send + Sync {
  /// 非空且 ID 唯一的实际来源快照；选择器 ID 不一定可用于后续直接选源。
  /// 列表在会话期间固定，不承诺跨会话顺序或 ID 稳定。
  fn sources(&self) -> &[CaptureSource];

  /// 取出本次整组选源获得的恢复凭据，最多返回一次，不影响捕获生命周期。
  ///
  /// `start` 成功时即可取出，不必等待首帧或停止；停止后尚未取出的凭据仍可取出。
  /// 未申请、未获准或平台不支持时返回 `None`。凭据由业务层按请求范围保存。
  /// 此接口不保证启动失败、取消或进程崩溃时新凭据能够交付。
  fn take_restore_token(&mut self) -> Option<CaptureRestoreToken> {
    None
  }

  /// 转移指定来源的唯一帧流，每个有效来源第一次返回 `Some`，之后返回 `None`。
  /// 未知或不属于本会话的 ID 返回 `None`，不影响任何来源。
  ///
  /// 每路仅交付该来源的帧，拥有独立缓存、唤醒和生命周期，可移至其他任务。
  /// 停止后首次取出仍返回 `Some`，其中可保留终止错误，但不保留图像帧。
  /// 取出与否不影响捕获启动时间；缓存与终止行为见 `FrameStream`。
  /// 未取出流的来源仍然捕获，但最多缓存一帧；丢弃已取出的流只停止该来源。
  fn take_frames(&mut self, source: &SourceId) -> Option<FrameStream>;

  /// 停止捕获并等待原生资源释放，无需消费者继续轮询帧流。
  ///
  /// 正常停止丢弃待消费图像并结束帧流，不产生新的终止错误。
  /// 若此前已异常终止，保留尚未消费的终止错误。
  /// 重复或并发调用应幂等；已释放资源的会话返回 `Ok(())`。
  /// 已派发的清理不能因 future 被丢弃而中止；清理失败可返回错误并允许重试。
  fn stop(&self) -> CaptureFuture<'_, ()>;
}

#[cfg(test)]
mod tests;
