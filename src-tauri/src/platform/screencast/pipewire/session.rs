use std::{
  collections::{HashMap, HashSet},
  pin::Pin,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  task::{Context, Poll},
};

use futures_util::{Stream, task::AtomicWaker};
use tokio::sync::{oneshot, watch};

use super::{super::*, backend, portal};

#[derive(Clone, Debug)]
pub(super) enum Cleanup {
  Running,
  Failed(String),
  Released,
}

#[derive(Default)]
struct Slot {
  frame: Option<ArcCaptureFrame>,
  error: Option<CaptureError>,
  ended: bool,
}

#[derive(Default)]
pub(super) struct Mailbox {
  slot: Mutex<Slot>,
  waker: AtomicWaker,
}

impl Mailbox {
  pub fn publish(&self, frame: ArcCaptureFrame) -> bool {
    // 消费者只在 poll 内短暂持锁，原生线程不等待异步消费者。
    // 竞争时直接丢帧，避免阻塞 PipeWire 回调。
    let mut published = false;
    if let Ok(mut slot) = self.slot.try_lock()
      && !slot.ended
    {
      slot.frame = Some(frame);
      published = true;
    }
    self.waker.wake();
    published
  }

  pub fn finish(&self, error: Option<CaptureError>) {
    let mut slot = self.slot.lock().unwrap();
    if !slot.ended {
      slot.frame = None;
      slot.error = error;
      slot.ended = true;
    }
    drop(slot);
    self.waker.wake();
  }
}

/// 来源的独立停止状态和邮箱；不持有原生对象。
#[derive(Default)]
pub(super) struct SourceControl {
  pub stopped: AtomicBool,
  pub mailbox: Mailbox,
}

impl SourceControl {
  fn finish(&self, error: Option<CaptureError>) {
    self.stopped.store(true, Ordering::Release);
    self.mailbox.finish(error);
  }
}

#[derive(Default)]
struct GroupState {
  sources: HashMap<SourceId, Arc<SourceControl>>,
  ended: bool,
  error: Option<CaptureError>,
}

pub(super) struct Control {
  pub stopped: AtomicBool,
  pub stop: watch::Sender<bool>,
  pub cleanup: watch::Sender<Cleanup>,
  state: Mutex<GroupState>,
}

impl Control {
  pub fn new() -> Arc<Self> {
    Arc::new(Self {
      stopped: AtomicBool::new(false),
      stop: watch::channel(false).0,
      cleanup: watch::channel(Cleanup::Running).0,
      state: Mutex::new(GroupState::default()),
    })
  }

  pub fn request_stop(&self) {
    self.finish(None);
  }

  pub fn fail(&self, error: CaptureError) {
    self.finish(Some(error));
  }

  fn finish(&self, error: Option<CaptureError>) {
    let mut state = self.state.lock().unwrap();
    if !state.ended {
      state.ended = true;
      state.error = error;
      for source in state.sources.values() {
        source.finish(state.error.clone());
      }
    }
    self.stopped.store(true, Ordering::Release);
    drop(state);
    // 即使已停止也通知 supervisor，允许重试 Portal 清理。
    self.stop.send_replace(true);
  }

  /// 在原生捕获开始前登记所有来源；重复登记不会覆盖缓存或终止状态。
  pub fn register_sources(&self, sources: &[CaptureSource]) {
    let mut state = self.state.lock().unwrap();
    let ended = state.ended;
    let error = state.error.clone();
    for source in sources {
      state.sources.entry(source.id.clone()).or_insert_with(|| {
        let source = Arc::new(SourceControl::default());
        if ended {
          source.finish(error.clone());
        }
        source
      });
    }
  }

  pub fn source(&self, id: &SourceId) -> Option<Arc<SourceControl>> {
    self.state.lock().unwrap().sources.get(id).cloned()
  }

  fn request_source_stop(&self, source: &SourceControl) {
    self.finish_source(source, None);
  }

  /// 运行期间的来源异常只结束本路；最后一路结束才触发共享资源清理。
  pub fn fail_source(&self, source: &SourceControl, error: CaptureError) {
    self.finish_source(source, Some(error));
  }

  fn finish_source(&self, source: &SourceControl, error: Option<CaptureError>) {
    let all_stopped = {
      let state = self.state.lock().unwrap();
      source.finish(error);
      !state.sources.is_empty()
        && state
          .sources
          .values()
          .all(|source| source.stopped.load(Ordering::Acquire))
    };
    if all_stopped {
      self.request_stop();
    }
  }

  pub fn take_error(&self) -> Option<CaptureError> {
    self.state.lock().unwrap().error.take()
  }

  pub fn frames(self: &Arc<Self>, id: &SourceId) -> Option<FrameStream> {
    self.source(id).map(|source| {
      Box::pin(Frames {
        control: self.clone(),
        source,
      }) as FrameStream
    })
  }
}

struct Frames {
  control: Arc<Control>,
  source: Arc<SourceControl>,
}

impl Stream for Frames {
  type Item = Result<ArcCaptureFrame, CaptureError>;

  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    self.source.mailbox.waker.register(cx.waker());
    let mut slot = self.source.mailbox.slot.lock().unwrap();
    if let Some(error) = slot.error.take() {
      return Poll::Ready(Some(Err(error)));
    }
    if slot.ended {
      return Poll::Ready(None);
    }
    if let Some(frame) = slot.frame.take() {
      return Poll::Ready(Some(Ok(frame)));
    }
    Poll::Pending
  }
}

impl Drop for Frames {
  fn drop(&mut self) {
    self.control.request_source_stop(&self.source);
  }
}

pub(super) struct Session {
  pub sources: Vec<CaptureSource>,
  pub restore_token: Option<CaptureRestoreToken>,
  pub control: Arc<Control>,
  available_frames: HashSet<SourceId>,
}

impl Session {
  pub fn new(
    sources: Vec<CaptureSource>,
    restore_token: Option<CaptureRestoreToken>,
    control: Arc<Control>,
  ) -> Self {
    control.register_sources(&sources);
    Self {
      available_frames: sources.iter().map(|source| source.id.clone()).collect(),
      sources,
      restore_token,
      control,
    }
  }
}

impl CaptureSession for Session {
  fn sources(&self) -> &[CaptureSource] {
    &self.sources
  }

  fn take_restore_token(&mut self) -> Option<CaptureRestoreToken> {
    self.restore_token.take()
  }

  fn take_frames(&mut self, source: &SourceId) -> Option<FrameStream> {
    if self.available_frames.remove(source) {
      self.control.frames(source)
    } else {
      None
    }
  }

  fn stop(&self) -> CaptureFuture<'_, ()> {
    // 在派发时请求清理，不依赖返回的 future 被轮询或继续持有。
    let mut cleanup = self.control.cleanup.subscribe();
    self.control.cleanup.send_if_modified(|state| {
      if matches!(state, Cleanup::Failed(_)) {
        *state = Cleanup::Running;
        true
      } else {
        false
      }
    });
    self.control.request_stop();
    Box::pin(async move {
      loop {
        let state = cleanup.borrow_and_update().clone();
        match state {
          Cleanup::Released => return Ok(()),
          Cleanup::Failed(message) => return Err(backend(message)),
          Cleanup::Running => {}
        }
        cleanup
          .changed()
          .await
          .map_err(|_| backend("捕获清理任务意外结束"))?;
      }
    })
  }
}

impl Drop for Session {
  fn drop(&mut self) {
    self.control.request_stop();
  }
}

struct StartGuard(Option<Arc<Control>>);

impl Drop for StartGuard {
  fn drop(&mut self) {
    if let Some(control) = &self.0 {
      control.request_stop();
    }
  }
}

pub(super) async fn start(
  parent: String,
  options: CaptureOptions,
) -> Result<Box<dyn CaptureSession>, CaptureError> {
  let control = Control::new();
  let mut guard = StartGuard(Some(control.clone()));
  let (ready, result) = oneshot::channel();
  tokio::spawn(portal::supervise(parent, options, control, ready));
  let session = result
    .await
    .map_err(|_| backend("捕获启动任务意外结束"))??;
  // supervisor 已移交控制权；取消发生在移交前或之后均由 Drop 触发清理。
  guard.0 = None;
  Ok(session)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn cancelling_start_guard_requests_cleanup() {
    let control = Control::new();
    drop(StartGuard(Some(control.clone())));
    assert!(control.stopped.load(Ordering::Acquire));
    assert!(*control.stop.borrow());
  }
}
