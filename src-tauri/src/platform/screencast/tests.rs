use std::num::NonZeroU32;

use super::*;

#[test]
fn treats_missing_or_zero_fps_as_no_additional_limit() {
  for fps in [None, Some(0)] {
    let options = CaptureOptions::new(
      CaptureTarget::Picker(SelectionKind::Any),
      CursorMode::Hidden,
      fps,
    );
    assert_eq!(options.max_fps, None);
    assert_eq!(options.persistence, CapturePersistence::None);
    assert_eq!(options.multiplicity, CaptureMultiplicity::Single);
  }
}

#[test]
fn allows_multiple_selection_without_changing_other_requests() {
  let options = CaptureOptions::new(
    CaptureTarget::Picker(SelectionKind::Any),
    CursorMode::Embedded,
    Some(30),
  )
  .with_multiplicity(CaptureMultiplicity::Multiple)
  .with_persistence(CapturePersistence::UntilRevoked);
  assert_eq!(options.multiplicity, CaptureMultiplicity::Multiple);
  assert_eq!(options.target, CaptureTarget::Picker(SelectionKind::Any));
  assert_eq!(options.cursor, CursorMode::Embedded);
  assert_eq!(options.max_fps, NonZeroU32::new(30));
  assert_eq!(options.persistence, CapturePersistence::UntilRevoked);
}

#[test]
fn preserves_requested_target_cursor_and_positive_fps() {
  let target = CaptureTarget::Source(SourceId("backend:window:1".into()));
  let options = CaptureOptions::new(target.clone(), CursorMode::Embedded, Some(30));

  assert_eq!(options.target, target);
  assert_eq!(options.cursor, CursorMode::Embedded);
  assert_eq!(options.max_fps, NonZeroU32::new(30));
}

#[test]
fn supports_dynamic_dispatch_and_cross_thread_control_handles() {
  fn assert_send<T: Send>() {}
  fn assert_send_sync<T: Send + Sync>() {}

  // 直接实例化 trait object 类型；新增不兼容动态分派的方法会在此编译失败。
  assert_send_sync::<Box<dyn Screencast>>();
  assert_send_sync::<Box<dyn CaptureSession>>();
  assert_send_sync::<ArcCaptureFrame>();
  assert_send_sync::<CaptureError>();
  assert_send_sync::<CaptureRestoreToken>();
  assert_send::<FrameStream>();
  assert_send::<CaptureFuture<'static, Box<dyn CaptureSession>>>();
}

#[test]
fn serializes_restore_tokens_without_exposing_payload_in_debug() {
  let token = CaptureRestoreToken::new("test-backend", 1, b"private-token".to_vec());
  let encoded = serde_json::to_string(&token).unwrap();
  let restored: CaptureRestoreToken = serde_json::from_str(&encoded).unwrap();
  assert_eq!(restored, token);
  assert_eq!(
    restored.payload_for("test-backend", 1),
    Some(&b"private-token"[..])
  );
  assert_eq!(restored.payload_for("another-backend", 1), None);
  assert_eq!(restored.payload_for("test-backend", 2), None);

  let options = CaptureOptions::new(
    CaptureTarget::RestoreOrPicker {
      token,
      kind: SelectionKind::Window,
    },
    CursorMode::Hidden,
    None,
  )
  .with_persistence(CapturePersistence::UntilRevoked);
  assert_eq!(options.persistence, CapturePersistence::UntilRevoked);
  assert!(!format!("{options:?}").contains("private-token"));
  assert!(!format!("{options:?}").contains("112, 114, 105"));
}

#[test]
fn cloning_backend_errors_preserves_the_original_error_chain() {
  use std::{error::Error, sync::Arc};

  let cause: Arc<dyn Error + Send + Sync> = Arc::new(std::io::Error::other("original failure"));
  let error = CaptureError::Backend {
    message: "capture failed".into(),
    source: Some(cause.clone()),
  };
  let cloned = error.clone();
  assert_eq!(cloned.source().unwrap().to_string(), "original failure");
  let CaptureError::Backend {
    source: Some(shared),
    ..
  } = cloned
  else {
    panic!("克隆后必须保留错误类型与来源");
  };
  assert!(Arc::ptr_eq(&shared, &cause));
}
