//! Linux 的 XDG Desktop Portal + PipeWire 后端。
//!
//! 在 Tokio runtime 内调用异步方法。原生 PipeWire 对象只在专用线程创建、
//! 使用和销毁；后端不依赖 Tauri。宿主可注入 Portal 窗口标识。

mod buffer;
mod portal;
mod rgba;
mod runtime;
mod session;

use super::*;

/// 每次启动创建新的单选或多选会话；可请求持久化并尝试恢复先前整组选源。
/// 恢复凭据由调用方保存，无法恢复时允许 Portal 提示用户重新选择。
#[derive(Debug, Default, Clone)]
pub struct PipeWireScreencast {
  parent_window: String,
}

impl PipeWireScreencast {
  /// `parent_window` 为 Portal 标识，例如 `x11:123abc` 或宿主导出的
  /// `wayland:…`；`None` 表示无父窗口。Wayland 导出句柄由宿主保活。
  pub fn new(parent_window: Option<String>) -> Self {
    Self {
      parent_window: parent_window.unwrap_or_default(),
    }
  }
}

impl Screencast for PipeWireScreencast {
  fn capabilities(&self) -> CaptureFuture<'_, CaptureCapabilities> {
    Box::pin(async { portal::capabilities().await })
  }

  fn sources(&self) -> CaptureFuture<'_, Vec<CaptureSource>> {
    Box::pin(async { Err(CaptureError::Unsupported(CaptureFeature::SourceEnumeration)) })
  }

  fn start(&self, options: CaptureOptions) -> CaptureFuture<'_, Box<dyn CaptureSession>> {
    Box::pin(async move {
      if matches!(options.target, CaptureTarget::Source(_)) {
        return Err(CaptureError::Unsupported(
          CaptureFeature::DirectSourceSelection,
        ));
      }
      // 无需连接 D-Bus 即可拒绝损坏或不属于此后端的凭据。
      portal::restore_token(&options.target)?;
      session::start(self.parent_window.clone(), options).await
    })
  }
}

fn backend(message: impl Into<String>) -> CaptureError {
  CaptureError::Backend {
    message: message.into(),
    source: None,
  }
}

fn caused_by(message: &str, error: impl std::error::Error + Send + Sync + 'static) -> CaptureError {
  CaptureError::Backend {
    message: message.into(),
    source: Some(std::sync::Arc::new(error)),
  }
}

#[cfg(test)]
mod tests;
