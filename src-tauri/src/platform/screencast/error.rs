use std::{error::Error, sync::Arc};

use super::{CapturePersistence, CursorMode, SelectionKind, SourceId, SourceKind};

/// 可由调用方识别并调整请求的不支持项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFeature {
  SourceEnumeration,
  DirectSourceSelection,
  SourceKind(SourceKind),
  Picker(SelectionKind),
  MultipleSelection(SelectionKind),
  Cursor(CursorMode),
  Restore(SelectionKind),
  Persistence(CapturePersistence),
}

/// 启动、帧流及停止操作共用的错误类型。
#[derive(Debug, Clone, thiserror::Error)]
pub enum CaptureError {
  #[error("不支持捕获功能：{0:?}")]
  Unsupported(CaptureFeature),
  /// 恢复凭据的后端、格式或负载不匹配；不包含敏感内容。
  #[error("恢复凭据无效或不适用于当前后端")]
  InvalidRestoreToken,
  /// 用户主动取消授权或选源，区别于明确拒绝权限。
  #[error("用户取消了捕获请求")]
  Cancelled,
  /// 权限未获批准或在捕获过程中被撤销。
  #[error("屏幕捕获权限被拒绝或撤销")]
  PermissionDenied,
  /// ID 无效、已过期、不属于此后端，或捕获中的来源已消失。
  #[error("捕获来源不可用：{0:?}")]
  SourceUnavailable(SourceId),
  /// 原生会话被外部关闭，且平台无法给出更具体的原因。
  #[error("捕获会话已被外部关闭")]
  SessionClosed,
  /// 其他平台错误或数据转换错误，保留原始错误链供日志诊断。
  #[error("捕获后端错误：{message}")]
  Backend {
    message: String,
    #[source]
    /// 使用共享错误链，整组异常可保留原始原因并交付到每个来源的帧流。
    source: Option<Arc<dyn Error + Send + Sync + 'static>>,
  },
}
