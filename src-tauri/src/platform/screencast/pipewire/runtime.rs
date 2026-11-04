use std::{
  cell::{Cell, RefCell},
  collections::HashSet,
  io::Cursor,
  num::NonZeroU32,
  os::fd::OwnedFd,
  rc::Rc,
  sync::{Arc, atomic::Ordering},
  time::{Duration, Instant},
};

use ::pipewire as pw;
use pw::{
  properties::properties,
  spa::{
    self,
    param::{
      ParamType,
      format::{FormatProperties, MediaSubtype, MediaType},
      video::{VideoFormat, VideoInfoRaw},
    },
    pod::{Object, Pod, Value, serialize::PodSerializer},
    utils::{Direction, Rectangle, SpaTypes},
  },
};
use tokio::sync::oneshot;

use super::{
  super::*,
  backend, buffer, caused_by,
  diagnostics::FrameStats,
  portal::Target,
  rgba::{self, FrameClock, FrameLimiter},
  session::{Control, SourceControl},
};

fn serialize(object: Object) -> Result<Vec<u8>, CaptureError> {
  PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
    .map(|result| result.0.into_inner())
    .map_err(|error| backend(format!("无法序列化 SPA 参数：{error:?}")))
}

fn format_param(max_fps: Option<NonZeroU32>) -> Result<Vec<u8>, CaptureError> {
  let mut object = spa::pod::object!(
    SpaTypes::ObjectParamFormat,
    ParamType::EnumFormat,
    spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
    spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
    spa::pod::property!(
      FormatProperties::VideoFormat,
      Choice,
      Enum,
      Id,
      VideoFormat::BGRx,
      VideoFormat::BGRx,
      VideoFormat::RGBx,
      VideoFormat::BGRA,
      VideoFormat::RGBA
    ),
    spa::pod::property!(
      FormatProperties::VideoSize,
      Choice,
      Range,
      Rectangle,
      Rectangle {
        width: 1920,
        height: 1080
      },
      Rectangle {
        width: 1,
        height: 1
      },
      Rectangle {
        width: rgba::MAX_DIMENSION,
        height: rgba::MAX_DIMENSION
      }
    ),
    spa::pod::property!(
      FormatProperties::VideoFramerate,
      Choice,
      Range,
      Fraction,
      spa::utils::Fraction {
        num: if max_fps.is_some() { 0 } else { 60 },
        denom: 1
      },
      spa::utils::Fraction { num: 0, denom: 1 },
      spa::utils::Fraction {
        num: max_fps.map_or(1000, NonZeroU32::get),
        denom: 1
      }
    ),
  );
  if let Some(fps) = max_fps {
    object.properties.push(spa::pod::property!(
      FormatProperties::VideoMaxFramerate,
      Choice,
      Range,
      Fraction,
      spa::utils::Fraction {
        num: fps.get(),
        denom: 1
      },
      spa::utils::Fraction { num: 1, denom: 1 },
      spa::utils::Fraction {
        num: fps.get(),
        denom: 1
      }
    ));
  }
  serialize(object)
}

fn buffer_params() -> Result<Vec<Vec<u8>>, CaptureError> {
  // 限定共享内存；MAP_BUFFERS 负责把 MemFd 映射为可读数据。
  let mask = ((1u32 << spa::sys::SPA_DATA_MemPtr) | (1u32 << spa::sys::SPA_DATA_MemFd)) as i32;
  let mut params = vec![serialize(Object {
    type_: SpaTypes::ObjectParamBuffers.as_raw(),
    id: ParamType::Buffers.as_raw(),
    properties: vec![
      spa::pod::Property::new(
        spa::sys::SPA_PARAM_BUFFERS_buffers,
        Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
          spa::utils::ChoiceFlags::empty(),
          spa::utils::ChoiceEnum::Range {
            default: 8,
            min: 2,
            max: 32,
          },
        ))),
      ),
      spa::pod::Property::new(
        spa::sys::SPA_PARAM_BUFFERS_dataType,
        Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
          spa::utils::ChoiceFlags::empty(),
          spa::utils::ChoiceEnum::Flags {
            default: mask,
            flags: vec![mask],
          },
        ))),
      ),
    ],
  })?];
  for (kind, size) in [
    (
      spa::sys::SPA_META_Header,
      std::mem::size_of::<spa::sys::spa_meta_header>(),
    ),
    (
      spa::sys::SPA_META_VideoCrop,
      std::mem::size_of::<spa::sys::spa_meta_region>(),
    ),
    (
      spa::sys::SPA_META_VideoTransform,
      std::mem::size_of::<spa::sys::spa_meta_videotransform>(),
    ),
  ] {
    params.push(serialize(Object {
      type_: SpaTypes::ObjectParamMeta.as_raw(),
      id: ParamType::Meta.as_raw(),
      properties: vec![
        spa::pod::Property::new(
          spa::sys::SPA_PARAM_META_type,
          Value::Id(spa::utils::Id(kind)),
        ),
        spa::pod::Property::new(spa::sys::SPA_PARAM_META_size, Value::Int(size as i32)),
      ],
    })?);
  }
  Ok(params)
}

struct StreamData {
  source_id: SourceId,
  source: Arc<SourceControl>,
  format: Option<VideoInfoRaw>,
  control: Arc<Control>,
  readiness: Rc<StartupBarrier>,
  clock: FrameClock,
  start: Instant,
  limiter: FrameLimiter,
  pending: Option<ArcCaptureFrame>,
  stats: FrameStats,
}

impl StreamData {
  fn format_changed(&mut self, param: Option<&Pod>) -> Result<Option<VideoInfoRaw>, CaptureError> {
    // 清空旧尺寸对应的待发布图像，但保留限速状态，重协商不能形成交付突发。
    self.format = None;
    self.pending = None;
    let Some(param) = param else { return Ok(None) };
    let mut format = VideoInfoRaw::default();
    let (media, subtype) = spa::param::format_utils::parse_format(param)
      .map_err(|error| caused_by("无法解析 PipeWire 媒体类型", error))?;
    if media != MediaType::Video || subtype != MediaSubtype::Raw {
      return Err(backend("PipeWire 未协商原始视频格式"));
    }
    format
      .parse(param)
      .map_err(|error| caused_by("无法解析 PipeWire 视频格式", error))?;
    buffer::validate_format(format)?;
    self.stats.negotiated(format);
    Ok(Some(format))
  }

  fn fail(&mut self, error: CaptureError) {
    self.pending = None;
    self
      .readiness
      .fail_source(&self.control, &self.source, error);
  }

  fn deliver_pending(&mut self, elapsed: Duration) {
    if self.source.stopped.load(Ordering::Acquire) {
      self.pending = None;
      return;
    }
    if !self.limiter.ready(elapsed) {
      return;
    }
    let Some(frame) = self.pending.take() else {
      return;
    };
    // 该图像已成功转换。邮箱竞争也计作一次采样，避免竞争时反复整帧转换。
    self.limiter.record_sample(elapsed);
    if self.source.mailbox.publish(frame) {
      self.stats.delivered(elapsed);
    } else {
      self.stats.mailbox_dropped += 1;
    }
  }

  fn stage_frame(
    &mut self,
    elapsed: Duration,
    timestamp: Duration,
    sequence: u64,
    copy: impl FnOnce() -> Result<image::RgbaImage, CaptureError>,
  ) -> bool {
    if self.source.stopped.load(Ordering::Acquire)
      || self.pending.is_some()
      || !self.limiter.should_convert(elapsed)
    {
      return false;
    }
    let started = Instant::now();
    let result = copy();
    self.stats.converted += 1;
    self.stats.conversion(started.elapsed());
    match result {
      Ok(image) => {
        self.pending = Some(Arc::new(CaptureFrame {
          source_id: self.source_id.clone(),
          image,
          timestamp,
          sequence,
        }))
      }
      Err(error) => self.fail(error),
    }
    true
  }

  fn process(&mut self, stream: &pw::stream::Stream) {
    // 固定本次队列快照，并持有所有 guard 到取帧结束，防止归还后被再次取出。
    // 上限控制单次回调工作量；剩余画面交给下一次回调，不无界排空。
    const MAX_BATCH: u32 = 32;
    let count = stream
      .time()
      .map_or(1, |time| time.avail_buffers().min(MAX_BATCH));
    let mut buffers = Vec::new();
    for _ in 0..count {
      let Some(buffer) = stream.dequeue_buffer() else {
        break;
      };
      buffers.push(buffer);
    }
    let mut selected = None;
    for (index, buffer) in buffers.iter_mut().enumerate() {
      self.stats.received += 1;
      if self.source.stopped.load(Ordering::Acquire) {
        self.pending = None;
        return;
      }
      let Some(format) = self.format else {
        self.stats.invalid += 1;
        continue;
      };
      // 即使限速，也检查坏帧、映射和布局错误；此步骤不分配像素图像。
      match buffer::prepare_frame(buffer, format) {
        Ok(Some(prepared)) => {
          self.stats.valid += 1;
          let elapsed = self.start.elapsed();
          let (timestamp, sequence) = self.clock.next(prepared.pts, elapsed);
          self.stats.skipped += 1;
          if self.pending.is_none() && self.limiter.should_convert(elapsed) {
            selected = Some((index, prepared, timestamp, sequence));
          }
        }
        Ok(None) => self.stats.invalid += 1,
        Err(error) => {
          self.fail(error);
          return;
        }
      }
    }
    // 本批只转换最后一个有效候选帧；坏帧不会覆盖较早的有效候选。
    if let Some((index, prepared, timestamp, sequence)) = selected
      && self.stage_frame(self.start.elapsed(), timestamp, sequence, || {
        buffer::copy_frame(&mut buffers[index], prepared)
      })
    {
      self.stats.skipped -= 1;
    }
    // 发布和限速时间在成功转换后读取，复制失败不消耗采样机会。
    self.deliver_pending(self.start.elapsed());
  }
}

impl Drop for StreamData {
  fn drop(&mut self) {
    self.pending = None;
    self.stats.report(self.start.elapsed(), true);
  }
}

fn handle_core_error(control: &Control, id: u32, code: i32, message: &str) {
  // error 的 id 是发生错误的 proxy，而不是目标 node 的全局 ID。
  // stream proxy 错误由 state_changed 处理；可选 NodeInfo 查询错误忽略。
  // 只有共享 core 的故障才能终止整组，避免单路错误先结束所有邮箱。
  if id != pw::core::PW_ID_CORE {
    return;
  }
  control.fail(if code == -13 || code == -1 {
    CaptureError::PermissionDenied
  } else {
    backend(format!("PipeWire core 错误（{code}）：{message}"))
  });
}

const TITLE_KEYS: [&str; 4] = ["media.title", "node.description", "media.name", "node.nick"];

fn matches_target(target: &Target, node: u32, props: Option<&spa::utils::dict::DictRef>) -> bool {
  if let Some(serial) = target.serial {
    props
      .and_then(|props| props.get("object.serial"))
      .and_then(|serial| serial.parse::<u64>().ok())
      == Some(serial)
  } else {
    node == target.node
  }
}

struct SourceMetadata {
  source: CaptureSource,
  title_priority: usize,
}

impl SourceMetadata {
  fn update(&mut self, props: &spa::utils::dict::DictRef) {
    for (priority, key) in TITLE_KEYS.iter().enumerate() {
      if let Some(title) = props.get(key).filter(|value| !value.trim().is_empty()) {
        // 空值和低优先级描述不能覆盖此前收到的有效名称。
        if priority <= self.title_priority {
          self.source.title = title.to_owned();
          self.title_priority = priority;
        }
        break;
      }
    }
  }
}

// 字段按声明顺序销毁，NodeInfo listener 必须先于对应 node proxy 释放。
struct MetadataNode {
  _listener: pw::node::NodeListener,
  _node: pw::node::Node,
}

/// 所有来源已连接且初始元数据查询完成后交付来源快照；不等待可选名称出现。
struct StartupBarrier {
  pending: RefCell<HashSet<SourceId>>,
  sources: Vec<Rc<RefCell<SourceMetadata>>>,
  metadata_ready: Cell<bool>,
  ready: RefCell<Option<oneshot::Sender<Vec<CaptureSource>>>>,
}

impl StartupBarrier {
  fn new(sources: Vec<CaptureSource>, ready: oneshot::Sender<Vec<CaptureSource>>) -> Self {
    Self {
      pending: RefCell::new(sources.iter().map(|source| source.id.clone()).collect()),
      sources: sources
        .into_iter()
        .map(|source| {
          Rc::new(RefCell::new(SourceMetadata {
            source,
            title_priority: usize::MAX,
          }))
        })
        .collect(),
      metadata_ready: Cell::new(false),
      ready: RefCell::new(Some(ready)),
    }
  }

  fn source_started(&self, source: &SourceId) {
    self.pending.borrow_mut().remove(source);
    self.try_ready();
  }

  fn fail_source(&self, control: &Control, source: &SourceControl, error: CaptureError) {
    if source.stopped.load(Ordering::Acquire) {
      return;
    }
    if self.ready.borrow().is_some() {
      // start 仍要求全部来源成功；不能交付部分成功的会话。
      control.fail(error);
    } else {
      control.fail_source(source, error);
    }
  }

  fn metadata_ready(&self) {
    self.metadata_ready.set(true);
    self.try_ready();
  }

  fn try_ready(&self) {
    if self.pending.borrow().is_empty() && self.metadata_ready.get() {
      if let Some(ready) = self.ready.borrow_mut().take() {
        let sources = self
          .sources
          .iter()
          .map(|source| source.borrow().source.clone())
          .collect();
        let _ = ready.send(sources);
      }
    }
  }
}

pub(super) fn run(
  fd: OwnedFd,
  targets: Vec<(CaptureSource, Target)>,
  options: CaptureOptions,
  control: Arc<Control>,
  ready: oneshot::Sender<Vec<CaptureSource>>,
) -> Result<(), CaptureError> {
  if control.stopped.load(Ordering::Acquire) {
    return Ok(());
  }
  let main_loop = pw::main_loop::MainLoopRc::new(None)
    .map_err(|error| caused_by("无法创建 PipeWire 主循环", error))?;
  let context = pw::context::ContextRc::new(&main_loop, None)
    .map_err(|error| caused_by("无法创建 PipeWire 上下文", error))?;
  let core = context
    .connect_fd_rc(fd, None)
    .map_err(|error| caused_by("无法连接 Portal PipeWire remote", error))?;
  let readiness = Rc::new(StartupBarrier::new(
    targets.iter().map(|(source, _)| source.clone()).collect(),
    ready,
  ));
  let sync_sequence = Rc::new(Cell::new(None));
  let sync_for_done = sync_sequence.clone();
  let second_roundtrip = Rc::new(Cell::new(false));
  let roundtrip_for_done = second_roundtrip.clone();
  let ready_for_done = readiness.clone();
  let core_for_done = core.downgrade();
  let error_control = control.clone();
  let _core_listener = core
    .add_listener_local()
    .done(move |id, seq| {
      if id != pw::core::PW_ID_CORE || sync_for_done.get() != Some(seq) {
        return;
      }
      if !roundtrip_for_done.replace(true) {
        // 第一轮保证收到 registry 的初始 globals；回调中创建的 NodeInfo
        // 查询在第二轮 sync 之前派发，第二轮处理完即可生成完整来源快照。
        if let Some(core) = core_for_done.upgrade() {
          if let Ok(seq) = core.sync(0) {
            sync_for_done.set(Some(seq));
            return;
          }
        }
      }
      sync_for_done.set(None);
      ready_for_done.metadata_ready();
    })
    .error(move |id, _, code, message| {
      handle_core_error(&error_control, id, code, message);
    })
    .register();
  let registry = core
    .get_registry_rc()
    .map_err(|error| caused_by("无法读取 PipeWire registry", error))?;
  let session_start = Instant::now();
  // listeners 比 streams 后声明，保证所有退出路径均先销毁 listeners。
  let mut streams = Vec::new();
  let mut registry_listeners = Vec::new();
  let mut stream_listeners = Vec::new();
  let mut metadata_nodes = Vec::new();
  let mut stream_data = Vec::new();
  for (index, (source, target)) in targets.into_iter().enumerate() {
    let source_control = control
      .source(&source.id)
      .ok_or_else(|| CaptureError::SourceUnavailable(source.id.clone()))?;
    let selected_node = Rc::new(Cell::new(None));
    let node_for_global = selected_node.clone();
    let node_for_remove = selected_node.clone();
    let removed_control = control.clone();
    let removed_source = source_control.clone();
    let ready_for_remove = readiness.clone();
    let source_id = source.id.clone();
    let source_metadata = readiness.sources[index].clone();
    let registry_for_global = registry.downgrade();
    let node_metadata = Rc::new(RefCell::new(None));
    let node_for_metadata = node_metadata.clone();
    let source_for_global = source_control.clone();
    let registry_listener = registry
      .add_listener_local()
      .global(move |global| {
        if global.type_ != pw::types::ObjectType::Node
          || source_for_global.stopped.load(Ordering::Acquire)
        {
          return;
        }
        let matches = matches_target(&target, global.id, global.props);
        if matches {
          node_for_global.set(Some(global.id));
          if let Some(props) = global.props {
            source_metadata.borrow_mut().update(props);
          }
          // Registry 的公开属性可能不含 media.title/media.name，进一步读取
          // 同一个授权来源的 NodeInfo；绝不查询本应用创建的消费 stream。
          if node_for_metadata.borrow().is_none()
            && global
              .permissions
              .contains(pw::permissions::PermissionFlags::R)
          {
            if let Some(registry) = registry_for_global.upgrade() {
              if let Ok(node) = registry.bind::<pw::node::Node, _>(global) {
                let source = source_metadata.clone();
                let listener = node
                  .add_listener_local()
                  .info(move |info| {
                    if let Some(props) = info.props() {
                      source.borrow_mut().update(props);
                    }
                  })
                  .register();
                *node_for_metadata.borrow_mut() = Some(MetadataNode {
                  _listener: listener,
                  _node: node,
                });
              }
            }
          }
        }
      })
      .global_remove(move |node| {
        if !removed_source.stopped.load(Ordering::Acquire) && node_for_remove.get() == Some(node) {
          ready_for_remove.fail_source(
            &removed_control,
            &removed_source,
            CaptureError::SourceUnavailable(source_id.clone()),
          );
        }
      })
      .register();
    registry_listeners.push(Some(registry_listener));
    metadata_nodes.push(node_metadata);
    let mut props = properties! {
      *pw::keys::MEDIA_TYPE => "Video",
      *pw::keys::MEDIA_CATEGORY => "Capture",
      *pw::keys::MEDIA_ROLE => "Screen",
      "node.dont-reconnect" => "true",
    };
    if let Some(serial) = target.serial {
      props.insert(*pw::keys::TARGET_OBJECT, serial.to_string());
    }
    let stream = pw::stream::StreamBox::new(&core, "eve-alert-screencast", props)
      .map_err(|error| caused_by("无法创建 PipeWire stream", error))?;
    let started = Rc::new(Cell::new(false));
    let started_state = started.clone();
    let state_source = source.id.clone();
    let ready_state = readiness.clone();
    let data = Rc::new(RefCell::new(StreamData {
      source_id: source.id,
      source: source_control.clone(),
      format: None,
      control: control.clone(),
      readiness: readiness.clone(),
      clock: FrameClock::default(),
      start: session_start,
      limiter: FrameLimiter::new(options.max_fps),
      pending: None,
      stats: FrameStats::new(options.max_fps, session_start.elapsed()),
    }));
    stream_data.push(data.clone());
    let stream_listener = stream
      .add_local_listener_with_user_data(data)
      .state_changed(move |_, data, _, state| {
        let mut data = data.borrow_mut();
        if data.source.stopped.load(Ordering::Acquire) {
          return;
        }
        match state {
          pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming => {
            started_state.set(true);
            ready_state.source_started(&state_source);
          }
          pw::stream::StreamState::Error(message) => {
            data.fail(backend(format!("PipeWire stream 错误：{message}")))
          }
          pw::stream::StreamState::Unconnected if started_state.get() => {
            data.fail(CaptureError::SourceUnavailable(state_source.clone()))
          }
          _ => {}
        }
      })
      .param_changed(|stream, data, id, param| {
        let mut data = data.borrow_mut();
        if id != ParamType::Format.as_raw() || data.source.stopped.load(Ordering::Acquire) {
          return;
        }
        let result = (|| {
          let Some(format) = data.format_changed(param)? else {
            return Ok(());
          };
          let bytes = buffer_params()?;
          let mut params = bytes
            .iter()
            .map(|bytes| Pod::from_bytes(bytes).ok_or_else(|| backend("SPA 缓冲区参数无效")))
            .collect::<Result<Vec<_>, _>>()?;
          stream
            .update_params(&mut params)
            .map_err(|error| caused_by("无法协商 CPU 缓冲区及帧元数据", error))?;
          data.format = Some(format);
          Ok::<_, CaptureError>(())
        })();
        if let Err(error) = result {
          data.fail(error);
        }
      })
      .process(|stream, data| {
        data.borrow_mut().process(stream);
      })
      .register()
      .map_err(|error| caused_by("无法注册 PipeWire stream listener", error))?;
    let bytes = format_param(options.max_fps)?;
    let mut params = [Pod::from_bytes(&bytes).ok_or_else(|| backend("SPA 格式参数无效"))?];
    stream
      .connect(
        Direction::Input,
        if target.serial.is_some() {
          None
        } else {
          Some(target.node)
        },
        pw::stream::StreamFlags::AUTOCONNECT
          | pw::stream::StreamFlags::MAP_BUFFERS
          | pw::stream::StreamFlags::DONT_RECONNECT,
        &mut params,
      )
      .map_err(|error| caused_by("无法连接选中的 PipeWire 来源", error))?;
    streams.push(Some((source_control, stream)));
    stream_listeners.push(Some(stream_listener));
  }
  match core.sync(0) {
    Ok(seq) => sync_sequence.set(Some(seq)),
    Err(_) => readiness.metadata_ready(),
  }
  while !control.stopped.load(Ordering::Acquire) {
    let mut timeout = Duration::from_millis(50);
    for index in 0..streams.len() {
      if streams[index].is_none() {
        continue;
      }
      let mut data = stream_data[index].borrow_mut();
      let elapsed = session_start.elapsed();
      data.deliver_pending(elapsed);
      data.stats.report(elapsed, false);
      if data.pending.is_some() {
        timeout = timeout.min(data.limiter.remaining(elapsed));
      }
      if streams[index]
        .as_ref()
        .is_some_and(|(source, _)| source.stopped.load(Ordering::Acquire))
      {
        data.pending = None;
        data.stats.report(elapsed, true);
        // 单路停止或异常：先撤销监听，再销毁 stream；不关闭共享 remote。
        // 销毁产生的状态/registry 通知不能被误判为整组来源异常。
        stream_listeners[index].take();
        registry_listeners[index].take();
        metadata_nodes[index].borrow_mut().take();
        streams[index].take();
      }
    }
    let result = main_loop
      .loop_()
      .iterate(pw::loop_::Timeout::Finite(timeout));
    if result < 0 {
      return Err(backend(format!("PipeWire 主循环失败（{result}）")));
    }
  }
  // 退出函数时按声明的逆序销毁 listeners、streams、registry、core、context。
  // supervisor join 此线程后才报告资源已释放。
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn data() -> StreamData {
    let source = source("test");
    let control = Control::new();
    control.register_sources(std::slice::from_ref(&source));
    let (ready, _) = oneshot::channel();
    StreamData {
      source_id: source.id.clone(),
      source: control.source(&source.id).unwrap(),
      format: None,
      control,
      readiness: Rc::new(StartupBarrier::new(vec![source], ready)),
      clock: FrameClock::default(),
      start: Instant::now(),
      limiter: FrameLimiter::new(NonZeroU32::new(2)),
      pending: None,
      stats: FrameStats::new(NonZeroU32::new(2), Duration::ZERO),
    }
  }

  fn sample(data: &mut StreamData, ms: u64, sequence: u64) -> bool {
    let now = Duration::from_millis(ms);
    data.stage_frame(now, now, sequence, || Ok(image::RgbaImage::new(1, 1)))
  }

  #[test]
  fn jittered_upstream_keeps_sampling_without_exceeding_delivery_limit() {
    let mut data = data();
    assert!(sample(&mut data, 0, 0));
    data.deliver_pending(Duration::ZERO);
    for (index, arrival) in [499, 1001, 1499, 2000, 2498, 3001].into_iter().enumerate() {
      assert!(sample(&mut data, arrival, index as u64 + 1));
      // 早到帧只保留在独立的自有图像槽中，不提前发布。
      let wait = data.limiter.remaining(Duration::from_millis(arrival));
      data.deliver_pending(Duration::from_millis(arrival) + wait);
      assert!(data.pending.is_none());
      assert_eq!(
        data
          .limiter
          .remaining(Duration::from_millis(arrival) + wait),
        Duration::from_millis(500)
      );
    }
    assert_eq!(data.stats.converted, 7);
  }

  #[test]
  fn surplus_frames_skip_conversion_and_pending_is_bounded_and_cleared_on_stop() {
    let mut first = data();
    let mut second = data();
    assert!(sample(&mut first, 0, 0));
    first.deliver_pending(Duration::ZERO);
    for ms in 1..450 {
      assert!(
        !first.stage_frame(Duration::from_millis(ms), Duration::ZERO, ms, || panic!(
          "限速帧不应执行像素转换"
        ))
      );
    }
    assert!(sample(&mut first, 499, 1));
    assert!(
      !first.stage_frame(Duration::from_millis(499), Duration::ZERO, 2, || panic!(
        "待发布槽最多一个图像"
      ))
    );
    assert!(sample(&mut second, 1, 0));
    second.deliver_pending(Duration::from_millis(1));
    first.control.request_stop();
    first.deliver_pending(Duration::from_millis(500));
    assert!(first.pending.is_none());
    assert!(!sample(&mut first, 1000, 3));
    assert!(!second.source.stopped.load(Ordering::Acquire));
  }

  #[test]
  fn unsuccessful_sampling_does_not_commit_limiter() {
    let mut data = data();
    assert!(data.stage_frame(Duration::ZERO, Duration::ZERO, 0, || Err(backend("坏帧"))));
    assert!(data.pending.is_none());
    assert!(data.limiter.ready(Duration::from_millis(1)));
    assert!(data.source.stopped.load(Ordering::Acquire));
  }

  #[test]
  fn resizing_discards_old_pending_image_without_resetting_delivery_limit() {
    let mut data = data();
    assert!(sample(&mut data, 0, 0));
    data.deliver_pending(Duration::ZERO);
    assert!(sample(&mut data, 499, 1));
    for width in [640, 1280] {
      let bytes = serialize(spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::Format,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(FormatProperties::VideoFormat, Id, VideoFormat::BGRA),
        spa::pod::property!(
          FormatProperties::VideoSize,
          Rectangle,
          Rectangle { width, height: 480 }
        ),
        spa::pod::property!(
          FormatProperties::VideoFramerate,
          Fraction,
          spa::utils::Fraction { num: 0, denom: 1 }
        ),
        spa::pod::property!(
          FormatProperties::VideoMaxFramerate,
          Fraction,
          spa::utils::Fraction { num: 2, denom: 1 }
        )
      ))
      .unwrap();
      let format = data
        .format_changed(Pod::from_bytes(&bytes))
        .unwrap()
        .unwrap();
      assert_eq!(format.size().width, width);
      assert!(super::super::diagnostics::upstream_is_limited(
        format,
        NonZeroU32::new(2).unwrap()
      ));
      assert!(data.pending.is_none());
      assert!(!data.limiter.ready(Duration::from_millis(499)));
      assert!(data.limiter.ready(Duration::from_millis(500)));
    }
  }

  #[test]
  fn format_choices_bound_both_variable_and_fixed_rates_without_limiting_none() {
    use spa::{
      pod::{ChoiceValue, deserialize::PodDeserializer},
      utils::{Choice, ChoiceEnum, Fraction},
    };
    for requested in [None, NonZeroU32::new(2), NonZeroU32::new(30)] {
      let bytes = format_param(requested).unwrap();
      let (_, Value::Object(object)) = PodDeserializer::deserialize_any_from(&bytes).unwrap()
      else {
        panic!()
      };
      let property =
        |key: FormatProperties| object.properties.iter().find(|p| p.key == key.as_raw());
      let framerate = &property(FormatProperties::VideoFramerate).unwrap().value;
      let maxrate = property(FormatProperties::VideoMaxFramerate);
      if let Some(fps) = requested {
        assert_eq!(
          *framerate,
          Value::Choice(ChoiceValue::Fraction(Choice(
            spa::utils::ChoiceFlags::empty(),
            ChoiceEnum::Range {
              default: Fraction { num: 0, denom: 1 },
              min: Fraction { num: 0, denom: 1 },
              max: Fraction {
                num: fps.get(),
                denom: 1
              },
            }
          )))
        );
        assert_eq!(
          maxrate.unwrap().value,
          Value::Choice(ChoiceValue::Fraction(Choice(
            spa::utils::ChoiceFlags::empty(),
            ChoiceEnum::Range {
              default: Fraction {
                num: fps.get(),
                denom: 1
              },
              min: Fraction { num: 1, denom: 1 },
              max: Fraction {
                num: fps.get(),
                denom: 1
              },
            }
          )))
        );
      } else {
        assert!(maxrate.is_none());
      }
    }
  }

  fn source(id: &str) -> CaptureSource {
    CaptureSource {
      title: String::new(),
      id: SourceId(id.into()),
      kind: SourceKind::Window,
    }
  }

  #[test]
  fn a_source_failure_during_startup_fails_the_entire_group() {
    let sources = vec![source("first"), source("second")];
    let control = Control::new();
    control.register_sources(&sources);
    let (ready, mut result) = oneshot::channel();
    let barrier = StartupBarrier::new(sources, ready);
    let first = SourceId("first".into());
    barrier.source_started(&first);
    barrier.fail_source(
      &control,
      &control.source(&first).unwrap(),
      CaptureError::SourceUnavailable(first),
    );
    assert!(control.stopped.load(Ordering::Acquire));
    assert!(matches!(
      control.take_error(),
      Some(CaptureError::SourceUnavailable(_))
    ));
    assert!(
      control
        .source(&SourceId("second".into()))
        .unwrap()
        .stopped
        .load(Ordering::Acquire)
    );
    assert!(matches!(
      result.try_recv(),
      Err(oneshot::error::TryRecvError::Empty)
    ));
  }

  #[test]
  fn a_source_failure_after_startup_keeps_other_sources_running() {
    let sources = vec![source("first"), source("second")];
    let control = Control::new();
    control.register_sources(&sources);
    let (ready, mut result) = oneshot::channel();
    let barrier = StartupBarrier::new(sources, ready);
    let first = SourceId("first".into());
    let second = SourceId("second".into());
    barrier.metadata_ready();
    barrier.source_started(&first);
    barrier.source_started(&second);
    assert_eq!(result.try_recv().unwrap().len(), 2);
    barrier.fail_source(
      &control,
      &control.source(&first).unwrap(),
      CaptureError::SourceUnavailable(first.clone()),
    );
    assert!(
      control
        .source(&first)
        .unwrap()
        .stopped
        .load(Ordering::Acquire)
    );
    assert!(
      !control
        .source(&second)
        .unwrap()
        .stopped
        .load(Ordering::Acquire)
    );
    assert!(!control.stopped.load(Ordering::Acquire));
    assert!(control.take_error().is_none());
  }

  #[test]
  fn proxy_errors_do_not_stop_the_group_but_shared_core_errors_do() {
    let control = Control::new();
    control.register_sources(&[source("first"), source("second")]);
    handle_core_error(&control, 10, -13, "stream permission denied");
    assert!(!control.stopped.load(Ordering::Acquire));
    assert!(control.take_error().is_none());
    handle_core_error(&control, pw::core::PW_ID_CORE, -32, "connection broken");
    assert!(control.stopped.load(Ordering::Acquire));
    assert!(matches!(
      control.take_error(),
      Some(CaptureError::Backend { .. })
    ));
    let control = Control::new();
    handle_core_error(&control, pw::core::PW_ID_CORE, -13, "permission denied");
    assert!(matches!(
      control.take_error(),
      Some(CaptureError::PermissionDenied)
    ));
  }

  #[test]
  fn waits_for_every_source_and_ignores_repeated_start_notifications() {
    let first = SourceId("first".into());
    let second = SourceId("second".into());
    let (ready, mut result) = oneshot::channel();
    let barrier = StartupBarrier::new(vec![source("first"), source("second")], ready);
    barrier.metadata_ready();
    barrier.source_started(&first);
    barrier.source_started(&first);
    barrier.source_started(&SourceId("foreign".into()));
    assert!(matches!(
      result.try_recv(),
      Err(oneshot::error::TryRecvError::Empty)
    ));
    barrier.source_started(&second);
    let sources = result.try_recv().unwrap();
    assert_eq!(
      sources.iter().map(|source| &source.id).collect::<Vec<_>>(),
      [&first, &second]
    );
    barrier.source_started(&second);
  }

  #[test]
  fn returns_names_queried_before_startup_without_reusing_the_original_empty_snapshot() {
    let (ready, mut result) = oneshot::channel();
    let mut display = source("display");
    display.kind = SourceKind::Display;
    let barrier = StartupBarrier::new(vec![source("window"), display], ready);
    barrier.source_started(&SourceId("window".into()));
    barrier.source_started(&SourceId("display".into()));
    assert!(matches!(
      result.try_recv(),
      Err(oneshot::error::TryRecvError::Empty)
    ));
    let window_props = properties! { "media.title" => "EVE — 测试窗口" };
    let display_props = properties! { "node.description" => "Dell U2723QE" };
    barrier.sources[0].borrow_mut().update(window_props.dict());
    barrier.sources[1].borrow_mut().update(display_props.dict());
    barrier.metadata_ready();
    let sources = result.try_recv().unwrap();
    assert_eq!(sources[0].title, "EVE — 测试窗口");
    assert_eq!(sources[0].id, SourceId("window".into()));
    assert_eq!(sources[1].title, "Dell U2723QE");
    assert_eq!(sources[1].kind, SourceKind::Display);
  }

  #[test]
  fn prefers_platform_titles_and_does_not_erase_them_with_empty_or_lower_priority_values() {
    let mut metadata = SourceMetadata {
      source: source("window"),
      title_priority: usize::MAX,
    };
    let registry_props = properties! { "node.description" => "Desktop capture" };
    metadata.update(registry_props.dict());
    assert_eq!(metadata.source.title, "Desktop capture");
    let info_props = properties! {
      "media.title" => "真实标题",
      "node.description" => "Lower priority",
      "media.name" => "Another name",
    };
    metadata.update(info_props.dict());
    assert_eq!(metadata.source.title, "真实标题");
    let missing_props = properties! { "media.title" => "  ", "media.name" => "Lower priority" };
    metadata.update(missing_props.dict());
    assert_eq!(metadata.source.title, "真实标题");
  }

  #[test]
  fn uses_only_available_platform_labels_and_keeps_missing_titles_empty() {
    for (key, title) in [
      ("media.title", "Window"),
      ("node.description", "Monitor"),
      ("media.name", "Stream name"),
      ("node.nick", "Nickname"),
    ] {
      let mut props = pw::properties::PropertiesBox::new();
      props.insert(key, title);
      let mut metadata = SourceMetadata {
        source: source("test"),
        title_priority: usize::MAX,
      };
      metadata.update(props.dict());
      assert_eq!(metadata.source.title, title);
    }
    let props = properties! {
      "media.title" => "",
      "node.description" => "  ",
      "node.name" => "eve-alert-screencast",
      "application.name" => "EVE Alert",
    };
    let mut metadata = SourceMetadata {
      source: source("test"),
      title_priority: usize::MAX,
    };
    metadata.update(props.dict());
    assert!(metadata.source.title.is_empty());
    // 即使没有名称，也必须按时完成启动，不能等待一个可选标题。
    let (ready, mut result) = oneshot::channel();
    let barrier = StartupBarrier::new(vec![source("test")], ready);
    barrier.metadata_ready();
    barrier.source_started(&SourceId("test".into()));
    assert!(result.try_recv().unwrap()[0].title.is_empty());
  }

  #[test]
  fn matches_only_the_authorized_target_and_does_not_fall_back_from_serial_to_a_reused_node_id() {
    let target = Target {
      node: 10,
      serial: Some(100),
    };
    let correct = properties! { "object.serial" => "100" };
    let reused = properties! { "object.serial" => "101" };
    assert!(matches_target(&target, 10, Some(correct.dict())));
    assert!(!matches_target(&target, 10, Some(reused.dict())));
    assert!(!matches_target(&target, 10, None));
    let legacy = Target {
      node: 10,
      serial: None,
    };
    assert!(matches_target(&legacy, 10, None));
    assert!(!matches_target(&legacy, 11, None));
  }
}
