use std::{
  num::NonZeroU32,
  sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
  },
  task::{Context, Wake, Waker},
  time::Duration,
};

use futures_util::StreamExt;
use image::RgbaImage;

use super::{
  super::*,
  portal::map_capabilities,
  rgba::{FrameClock, FrameLimiter, Layout, PixelFormat, convert},
  session::{Cleanup, Control, Session},
};

fn layout() -> Layout {
  Layout {
    width: 1,
    height: 2,
    offset: 0,
    size: 8,
    stride: 4,
    format: PixelFormat::Rgba,
    premultiplied: false,
    crop: None,
    transform: 0,
  }
}

fn frame(sequence: u64) -> ArcCaptureFrame {
  frame_from("test", sequence)
}

fn frame_from(source: &str, sequence: u64) -> ArcCaptureFrame {
  Arc::new(CaptureFrame {
    source_id: SourceId(source.into()),
    image: RgbaImage::new(1, 1),
    timestamp: Duration::ZERO,
    sequence,
  })
}

#[test]
fn converts_padded_bgrx_rows_with_chunk_offset() {
  let bytes = [9, 9, 3, 2, 1, 0, 9, 9, 6, 5, 4, 0];
  let image = convert(
    &bytes,
    Layout {
      offset: 2,
      stride: 6,
      size: 10,
      format: PixelFormat::Bgrx,
      ..layout()
    },
  )
  .unwrap();
  assert_eq!(image.into_raw(), [1, 2, 3, 255, 4, 5, 6, 255]);
}

#[test]
fn converts_bottom_up_rows_and_premultiplied_alpha() {
  let image = convert(
    &[0, 0, 0, 0, 64, 32, 0, 128],
    Layout {
      stride: -4,
      premultiplied: true,
      ..layout()
    },
  )
  .unwrap();
  assert_eq!(image.into_raw(), [128, 64, 0, 128, 0, 0, 0, 0]);
}

#[test]
fn preserves_bgra_alpha_and_makes_rgbx_opaque() {
  let image = convert(
    &[1, 2, 3, 4, 5, 6, 7, 8],
    Layout {
      format: PixelFormat::Bgra,
      ..layout()
    },
  )
  .unwrap();
  assert_eq!(image.into_raw(), [3, 2, 1, 4, 7, 6, 5, 8]);
  let image = convert(
    &[1, 2, 3, 4, 5, 6, 7, 8],
    Layout {
      format: PixelFormat::Rgbx,
      ..layout()
    },
  )
  .unwrap();
  assert_eq!(image.into_raw(), [1, 2, 3, 255, 5, 6, 7, 255]);
}

#[test]
fn crops_and_rotates_to_actual_pixel_dimensions() {
  let bytes = [1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255];
  let image = convert(
    &bytes,
    Layout {
      width: 2,
      stride: 8,
      size: 16,
      crop: Some((1, 0, 1, 2)),
      transform: 1,
      ..layout()
    },
  )
  .unwrap();
  assert_eq!(image.dimensions(), (2, 1));
  assert_eq!(image.into_raw(), [2, 0, 0, 255, 4, 0, 0, 255]);
}

#[test]
fn rejects_invalid_buffer_bounds_stride_crop_and_dimensions() {
  for invalid in [
    Layout {
      offset: usize::MAX,
      ..layout()
    },
    Layout {
      size: 7,
      ..layout()
    },
    Layout {
      stride: 3,
      ..layout()
    },
    Layout {
      width: 0,
      ..layout()
    },
    Layout {
      width: u32::MAX,
      ..layout()
    },
    Layout {
      crop: Some((1, 0, 1, 1)),
      ..layout()
    },
    Layout {
      crop: Some((u32::MAX, 0, 1, 1)),
      ..layout()
    },
    Layout {
      transform: 8,
      ..layout()
    },
  ] {
    assert!(convert(&[0; 8], invalid).is_err());
  }
}

#[test]
fn normalizes_native_timestamps_and_never_moves_backwards() {
  let mut clock = FrameClock::default();
  assert_eq!(
    clock.next(Some(1000), Duration::from_nanos(10)),
    (Duration::from_nanos(10), 0)
  );
  assert_eq!(
    clock.next(Some(1020), Duration::from_nanos(99)),
    (Duration::from_nanos(30), 1)
  );
  assert_eq!(
    clock.next(Some(1010), Duration::from_nanos(110)),
    (Duration::from_nanos(30), 2)
  );
  assert_eq!(
    clock.next(None, Duration::from_nanos(120)),
    (Duration::from_nanos(120), 3)
  );
}

#[test]
fn maps_only_actual_portal_capabilities() {
  let capabilities = map_capabilities(6, 3, 5);
  assert!(capabilities.enumerable_source_kinds.is_empty());
  assert!(capabilities.direct_source_kinds.is_empty());
  assert_eq!(
    capabilities.picker_kinds,
    [
      SelectionKind::Display,
      SelectionKind::Window,
      SelectionKind::Any
    ]
  );
  assert_eq!(capabilities.cursor_modes, [CursorMode::Hidden]);
  assert_eq!(capabilities.restore_kinds, capabilities.picker_kinds);
  assert_eq!(
    capabilities.multiple_picker_kinds,
    capabilities.picker_kinds
  );
  assert_eq!(
    capabilities.multiple_restore_kinds,
    capabilities.restore_kinds
  );
  assert_eq!(
    capabilities.persistence_modes,
    [
      CapturePersistence::None,
      CapturePersistence::Application,
      CapturePersistence::UntilRevoked,
    ]
  );
  let legacy = map_capabilities(3, 3, 3);
  assert!(legacy.restore_kinds.is_empty());
  assert!(legacy.multiple_restore_kinds.is_empty());
  assert_eq!(legacy.multiple_picker_kinds, legacy.picker_kinds);
  assert_eq!(legacy.persistence_modes, [CapturePersistence::None]);
  assert_eq!(
    map_capabilities(4, 1, 1).restore_kinds,
    [SelectionKind::Display, SelectionKind::Any]
  );
  assert!(
    !map_capabilities(2, 3, 3)
      .picker_kinds
      .contains(&SelectionKind::Any)
  );
  assert_eq!(
    map_capabilities(1, 1, 1).picker_kinds,
    [SelectionKind::Display, SelectionKind::Any]
  );
  assert!(map_capabilities(6, 4, 4).picker_kinds.is_empty());
}

#[tokio::test]
async fn retains_only_latest_pending_frame() {
  let control = Control::new();
  let mut session = make_session(&control);
  let mut frames = session.take_frames(&SourceId("test".into())).unwrap();
  for sequence in 0..100 {
    publish(&control, frame(sequence));
  }
  assert_eq!(frames.next().await.unwrap().unwrap().sequence, 99);
  assert!(futures_util::poll!(frames.next()).is_pending());
}

#[tokio::test]
async fn consumes_each_source_independently_and_retains_its_latest_frame() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  let mut first = session.take_frames(&SourceId("test".into())).unwrap();
  let mut second = session.take_frames(&SourceId("other".into())).unwrap();
  publish(&control, frame_from("other", 0));
  for sequence in 0..100 {
    publish(&control, frame(sequence));
  }
  let second_frame = second.next().await.unwrap().unwrap();
  assert_eq!(second_frame.source_id, SourceId("other".into()));
  assert_eq!(second_frame.sequence, 0);
  assert!(futures_util::poll!(second.next()).is_pending());
  publish(&control, frame_from("other", 1));
  assert_eq!(second.next().await.unwrap().unwrap().sequence, 1);
  let first_frame = first.next().await.unwrap().unwrap();
  assert_eq!(first_frame.source_id, SourceId("test".into()));
  assert_eq!(first_frame.sequence, 99);
  assert!(futures_util::poll!(first.next()).is_pending());
}

#[tokio::test]
async fn delivers_terminal_error_once_and_discards_pending_images() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  let mut frames = session.take_frames(&SourceId("test".into())).unwrap();
  let mut other = session.take_frames(&SourceId("other".into())).unwrap();
  publish(&control, frame(0));
  publish(&control, frame_from("other", 0));
  control.fail(CaptureError::SessionClosed);
  publish(&control, frame(1));
  control.request_stop();
  assert!(matches!(
    frames.next().await,
    Some(Err(CaptureError::SessionClosed))
  ));
  assert!(frames.next().await.is_none());
  assert!(frames.next().await.is_none());
  assert!(matches!(
    other.next().await,
    Some(Err(CaptureError::SessionClosed))
  ));
  assert!(other.next().await.is_none());
  assert!(other.next().await.is_none());
}

#[tokio::test]
async fn normal_stop_wakes_waiter_and_ends_without_an_error() {
  let control = Control::new();
  let mut session = make_session(&control);
  let mut frames = session.take_frames(&SourceId("test".into())).unwrap();
  assert!(futures_util::poll!(frames.next()).is_pending());
  control.request_stop();
  assert!(frames.next().await.is_none());
}

fn make_session(control: &Arc<Control>) -> Session {
  Session::new(
    vec![CaptureSource {
      title: String::new(),
      id: SourceId("test".into()),
      kind: SourceKind::Display,
    }],
    None,
    control.clone(),
  )
}

fn make_multi_session(control: &Arc<Control>) -> Session {
  Session::new(
    vec![
      CaptureSource {
        title: String::new(),
        id: SourceId("test".into()),
        kind: SourceKind::Display,
      },
      CaptureSource {
        title: String::new(),
        id: SourceId("other".into()),
        kind: SourceKind::Window,
      },
    ],
    None,
    control.clone(),
  )
}

fn publish(control: &Control, frame: ArcCaptureFrame) {
  control
    .source(&frame.source_id)
    .unwrap()
    .mailbox
    .publish(frame);
}

#[test]
fn takes_each_source_once_and_rejects_unknown_ids_without_stopping_capture() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  assert!(session.take_frames(&SourceId("foreign".into())).is_none());
  let first = session.take_frames(&SourceId("test".into())).unwrap();
  assert!(session.take_frames(&SourceId("test".into())).is_none());
  let second = session.take_frames(&SourceId("other".into())).unwrap();
  assert!(session.take_frames(&SourceId("other".into())).is_none());
  assert!(!control.stopped.load(Ordering::Acquire));
  drop((first, second));
}

#[tokio::test]
async fn dropping_one_source_only_stops_it_and_dropping_the_last_stops_the_group() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  let first = session.take_frames(&SourceId("test".into())).unwrap();
  drop(first);
  assert!(
    control
      .source(&SourceId("test".into()))
      .unwrap()
      .stopped
      .load(Ordering::Acquire)
  );
  assert!(
    !control
      .source(&SourceId("other".into()))
      .unwrap()
      .stopped
      .load(Ordering::Acquire)
  );
  assert!(!control.stopped.load(Ordering::Acquire));
  // 未取出的另一路仍然运行，并保留自己的最新帧。
  publish(&control, frame_from("other", 7));
  let mut second = session.take_frames(&SourceId("other".into())).unwrap();
  assert_eq!(second.next().await.unwrap().unwrap().sequence, 7);
  assert!(session.take_frames(&SourceId("test".into())).is_none());
  drop(second);
  assert!(control.stopped.load(Ordering::Acquire));
  assert!(*control.stop.borrow());
}

#[derive(Default)]
struct WakeCounter(AtomicUsize);

#[tokio::test]
async fn a_disappearing_source_ends_only_its_stream_and_other_sources_keep_delivering() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  let first_id = SourceId("test".into());
  let first_source = control.source(&first_id).unwrap();
  let mut first = session.take_frames(&first_id).unwrap();
  let mut second = session.take_frames(&SourceId("other".into())).unwrap();
  publish(&control, frame(0));
  publish(&control, frame_from("other", 0));
  control.fail_source(
    &first_source,
    CaptureError::SourceUnavailable(first_id.clone()),
  );
  // 后续帧与重复通知都不能覆盖本路终止错误，也不能触发整组停止。
  publish(&control, frame(1));
  control.fail_source(&first_source, CaptureError::SessionClosed);
  assert!(!control.stopped.load(Ordering::Acquire));
  assert!(!*control.stop.borrow());
  assert!(matches!(
    first.next().await,
    Some(Err(CaptureError::SourceUnavailable(id))) if id == first_id
  ));
  assert!(first.next().await.is_none());
  drop(first);
  assert_eq!(second.next().await.unwrap().unwrap().sequence, 0);
  publish(&control, frame_from("other", 1));
  assert_eq!(second.next().await.unwrap().unwrap().sequence, 1);
  assert!(futures_util::poll!(second.next()).is_pending());
  drop(second);
  assert!(control.stopped.load(Ordering::Acquire));
  assert!(*control.stop.borrow());
}

#[tokio::test]
async fn the_last_source_failure_requests_cleanup_and_preserves_each_sources_error() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  for id in ["test", "other"] {
    let source_id = SourceId(id.into());
    control.fail_source(
      &control.source(&source_id).unwrap(),
      CaptureError::SourceUnavailable(source_id),
    );
  }
  assert!(control.stopped.load(Ordering::Acquire));
  assert!(*control.stop.borrow());
  control.cleanup.send_replace(Cleanup::Released);
  session.stop().await.unwrap();
  // 即使来源异常时尚未取流，错误仍分别保留，整组正常清理不能覆盖它们。
  for id in ["test", "other"] {
    let source_id = SourceId(id.into());
    let mut frames = session.take_frames(&source_id).unwrap();
    assert!(matches!(
      frames.next().await,
      Some(Err(CaptureError::SourceUnavailable(id))) if id == source_id
    ));
    assert!(frames.next().await.is_none());
  }
}

#[tokio::test]
async fn shared_session_failure_ends_remaining_sources_without_overwriting_a_local_error() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  let first_id = SourceId("test".into());
  control.fail_source(
    &control.source(&first_id).unwrap(),
    CaptureError::SourceUnavailable(first_id.clone()),
  );
  control.fail(CaptureError::PermissionDenied);
  let mut first = session.take_frames(&first_id).unwrap();
  let mut second = session.take_frames(&SourceId("other".into())).unwrap();
  assert!(matches!(
    first.next().await,
    Some(Err(CaptureError::SourceUnavailable(id))) if id == first_id
  ));
  assert!(matches!(
    second.next().await,
    Some(Err(CaptureError::PermissionDenied))
  ));
  assert!(first.next().await.is_none());
  assert!(second.next().await.is_none());
}

#[test]
fn failing_a_source_wakes_only_its_waiter() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  let first_id = SourceId("test".into());
  let mut first = session.take_frames(&first_id).unwrap();
  let mut second = session.take_frames(&SourceId("other".into())).unwrap();
  let first_wakes = Arc::new(WakeCounter::default());
  let second_wakes = Arc::new(WakeCounter::default());
  let first_waker = Waker::from(first_wakes.clone());
  let second_waker = Waker::from(second_wakes.clone());
  assert!(
    first
      .as_mut()
      .poll_next(&mut Context::from_waker(&first_waker))
      .is_pending()
  );
  assert!(
    second
      .as_mut()
      .poll_next(&mut Context::from_waker(&second_waker))
      .is_pending()
  );
  control.fail_source(
    &control.source(&first_id).unwrap(),
    CaptureError::SourceUnavailable(first_id),
  );
  assert_eq!(first_wakes.0.load(Ordering::Relaxed), 1);
  assert_eq!(second_wakes.0.load(Ordering::Relaxed), 0);
}

impl Wake for WakeCounter {
  fn wake(self: Arc<Self>) {
    self.wake_by_ref();
  }

  fn wake_by_ref(self: &Arc<Self>) {
    self.0.fetch_add(1, Ordering::Relaxed);
  }
}

#[test]
fn publishing_a_frame_only_wakes_its_source_consumer() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  let mut first = session.take_frames(&SourceId("test".into())).unwrap();
  let mut second = session.take_frames(&SourceId("other".into())).unwrap();
  let first_wakes = Arc::new(WakeCounter::default());
  let second_wakes = Arc::new(WakeCounter::default());
  let first_waker = Waker::from(first_wakes.clone());
  let second_waker = Waker::from(second_wakes.clone());
  assert!(
    first
      .as_mut()
      .poll_next(&mut Context::from_waker(&first_waker))
      .is_pending()
  );
  assert!(
    second
      .as_mut()
      .poll_next(&mut Context::from_waker(&second_waker))
      .is_pending()
  );
  publish(&control, frame(0));
  assert_eq!(first_wakes.0.load(Ordering::Relaxed), 1);
  assert_eq!(second_wakes.0.load(Ordering::Relaxed), 0);
  publish(&control, frame_from("other", 0));
  assert_eq!(first_wakes.0.load(Ordering::Relaxed), 1);
  assert_eq!(second_wakes.0.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn registering_sources_after_a_startup_failure_preserves_the_terminal_error() {
  let control = Control::new();
  control.fail(CaptureError::PermissionDenied);
  let mut session = make_session(&control);
  let mut frames = session.take_frames(&SourceId("test".into())).unwrap();
  assert!(matches!(
    frames.next().await,
    Some(Err(CaptureError::PermissionDenied))
  ));
  assert!(frames.next().await.is_none());
}

#[tokio::test]
async fn stopping_a_multi_source_session_ends_all_pending_sources() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  assert_eq!(session.sources().len(), 2);
  let mut frames = session.take_frames(&SourceId("test".into())).unwrap();
  for source in session.sources() {
    publish(&control, frame_from(&source.id.0, 0));
  }
  control.cleanup.send_replace(Cleanup::Released);
  session.stop().await.unwrap();
  assert!(frames.next().await.is_none());
  let mut other = session.take_frames(&SourceId("other".into())).unwrap();
  assert!(other.next().await.is_none());
  assert_eq!(session.sources().len(), 2);
}

#[tokio::test]
async fn takes_restore_token_once_without_affecting_frames_or_stop() {
  let control = Control::new();
  let mut session = make_session(&control);
  let token = CaptureRestoreToken::new("test", 1, b"new-token".to_vec());
  session.restore_token = Some(token.clone());
  assert_eq!(session.take_restore_token(), Some(token));
  assert!(session.take_restore_token().is_none());
  assert!(!control.stopped.load(Ordering::Acquire));
  let mut frames = session.take_frames(&SourceId("test".into())).unwrap();
  publish(&control, frame(1));
  assert_eq!(frames.next().await.unwrap().unwrap().sequence, 1);
  control.cleanup.send_replace(Cleanup::Released);
  session.stop().await.unwrap();
  assert!(frames.next().await.is_none());

  let control = Control::new();
  let mut session = make_session(&control);
  let token = CaptureRestoreToken::new("test", 1, b"unclaimed-token".to_vec());
  session.restore_token = Some(token.clone());
  control.cleanup.send_replace(Cleanup::Released);
  session.stop().await.unwrap();
  assert_eq!(session.take_restore_token(), Some(token));
}

#[tokio::test]
async fn stop_waits_for_release_and_supports_concurrent_calls() {
  let control = Control::new();
  let session = make_session(&control);
  let first = session.stop();
  let second = session.stop();
  let waiter = async { tokio::join!(first, second) };
  tokio::pin!(waiter);
  assert!(futures_util::poll!(&mut waiter).is_pending());
  assert!(control.stopped.load(Ordering::Acquire));
  control.cleanup.send_replace(Cleanup::Released);
  let (first, second) = waiter.await;
  assert!(first.is_ok() && second.is_ok());
  assert!(session.stop().await.is_ok());
}

#[tokio::test]
async fn dropping_either_handle_stops_capture_and_take_frames_is_unique() {
  let control = Control::new();
  let mut session = make_session(&control);
  let frames = session.take_frames(&SourceId("test".into())).unwrap();
  assert!(session.take_frames(&SourceId("test".into())).is_none());
  drop(frames);
  assert!(control.stopped.load(Ordering::Acquire));
  let control = Control::new();
  let mut session = make_session(&control);
  let mut frames = session.take_frames(&SourceId("test".into())).unwrap();
  drop(session);
  assert!(control.stopped.load(Ordering::Acquire));
  assert!(frames.next().await.is_none());
}

#[tokio::test]
async fn taking_frames_after_stop_keeps_only_the_terminal_error() {
  let control = Control::new();
  let mut session = make_multi_session(&control);
  control.fail(CaptureError::PermissionDenied);
  let mut frames = session.take_frames(&SourceId("test".into())).unwrap();
  assert!(matches!(
    frames.next().await,
    Some(Err(CaptureError::PermissionDenied))
  ));
  assert!(frames.next().await.is_none());
  let mut other = session.take_frames(&SourceId("other".into())).unwrap();
  assert!(matches!(
    other.next().await,
    Some(Err(CaptureError::PermissionDenied))
  ));
  assert!(other.next().await.is_none());
}

#[tokio::test]
async fn rejects_enumeration_and_direct_selection_without_opening_a_picker() {
  let backend = super::PipeWireScreencast::default();
  assert!(matches!(
    backend.sources().await,
    Err(CaptureError::Unsupported(CaptureFeature::SourceEnumeration))
  ));
  assert!(matches!(
    backend
      .start(CaptureOptions::new(
        CaptureTarget::Source(SourceId("foreign".into())),
        CursorMode::Hidden,
        None
      ))
      .await,
    Err(CaptureError::Unsupported(
      CaptureFeature::DirectSourceSelection
    ))
  ));
}

#[tokio::test]
async fn rejects_foreign_restore_tokens_before_connecting_to_portal() {
  let backend = super::PipeWireScreencast::default();
  let error = backend
    .start(CaptureOptions::new(
      CaptureTarget::RestoreOrPicker {
        token: CaptureRestoreToken::new("foreign-backend", 1, b"token".to_vec()),
        kind: SelectionKind::Display,
      },
      CursorMode::Hidden,
      None,
    ))
    .await;
  assert!(matches!(error, Err(CaptureError::InvalidRestoreToken)));
}
#[test]
fn dropping_stop_future_does_not_cancel_the_stop_request() {
  let control = Control::new();
  let session = make_session(&control);
  drop(session.stop());
  assert!(control.stopped.load(Ordering::Acquire));
}

#[test]
fn limits_delivery_without_rounding_above_requested_fps() {
  let mut limiter = FrameLimiter::new(std::num::NonZeroU32::new(30));
  assert!(limiter.ready(Duration::ZERO));
  limiter.record_sample(Duration::ZERO);
  assert!(!limiter.ready(Duration::from_nanos(33_333_333)));
  assert!(limiter.ready(Duration::from_nanos(33_333_334)));
  limiter.record_sample(Duration::from_nanos(33_333_334));
  assert!(!limiter.ready(Duration::from_millis(40)));
  assert!(limiter.ready(Duration::from_millis(80)));
  let unlimited = FrameLimiter::new(None);
  for _ in 0..10 {
    assert!(unlimited.ready(Duration::ZERO));
  }
}

#[tokio::test]
async fn retrying_stop_waits_for_the_new_cleanup_attempt() {
  let control = Control::new();
  let session = make_session(&control);
  let first = session.stop();
  control
    .cleanup
    .send_replace(Cleanup::Failed("temporary failure".into()));
  assert!(first.await.is_err());
  let retry = session.stop();
  tokio::pin!(retry);
  assert!(futures_util::poll!(&mut retry).is_pending());
  control.cleanup.send_replace(Cleanup::Released);
  assert!(retry.await.is_ok());
}

#[tokio::test]
#[ignore]
async fn visual_testing() -> anyhow::Result<()> {
  let backend = super::PipeWireScreencast::new(None);
  let mut session = backend
    .start(CaptureOptions {
      target: CaptureTarget::Picker(SelectionKind::Window),
      cursor: CursorMode::Embedded,
      max_fps: None,
      persistence: CapturePersistence::None,
      multiplicity: CaptureMultiplicity::Single,
    })
    .await?;
  let id = session.sources()[0].id.clone();
  let mut frames = session.take_frames(&id).unwrap();
  let first_frame = frames.next().await.unwrap()?;
  first_frame.image.save("./capture_testing.png").unwrap();
  Ok(())
}

#[tokio::test]
#[ignore]
async fn multiple_picker_testing() -> anyhow::Result<()> {
  let backend = super::PipeWireScreencast::new(None);
  let mut session = backend
    .start(CaptureOptions {
      target: CaptureTarget::Picker(SelectionKind::Any),
      cursor: CursorMode::Embedded,
      max_fps: NonZeroU32::new(10),
      persistence: CapturePersistence::None,
      multiplicity: CaptureMultiplicity::Multiple,
    })
    .await?;
  let ids: Vec<_> = session
    .sources()
    .iter()
    .map(|source| source.id.clone())
    .collect();
  let mut frames = Vec::new();
  for id in ids {
    frames.push((id.clone(), session.take_frames(&id).unwrap()));
  }
  let mut tasks = Vec::new();
  println!("Session Sources: {:#?}", session.sources());
  for (id, mut frame) in frames {
    let task = tokio::spawn(async move {
      loop {
        let capture = frame.next().await;
        match capture {
          Some(Ok(capture_frame)) => {
            let image = &capture_frame.image;
            println!("received image: {} x {}", image.width(), image.height());
            println!("source id: {:?}", capture_frame.source_id);
            println!("\n");
          }
          Some(Err(capture_error)) => {
            println!("capture error for {id:?}: {capture_error}");
            return Err(capture_error);
          }
          None => {
            println!("capture ended for {id:?}");
            break;
          }
        }
      }
      Ok::<_, CaptureError>(())
    });
    tasks.push(task);
  }
  let results = futures_util::future::join_all(tasks).await;
  // 等待每路消费者结束并清理资源后，再报告原始捕获错误或任务错误。
  session.stop().await?;
  for result in results {
    result??;
  }
  Ok(())
}
