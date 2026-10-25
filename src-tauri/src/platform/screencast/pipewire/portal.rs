use std::{
  collections::{HashMap, HashSet},
  os::fd::OwnedFd,
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
  time::Duration,
};

use futures_util::StreamExt;
use tokio::sync::{oneshot, watch};
use zbus::{
  Connection, Proxy,
  zvariant::{OwnedObjectPath, OwnedValue, Value},
};

use super::{
  super::*,
  backend, caused_by, runtime,
  session::{Cleanup, Control, Session},
};

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.ScreenCast";
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const RESTORE_BACKEND: &str = "linux.xdg-desktop-portal.screencast";
const RESTORE_FORMAT_VERSION: u32 = 1;
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

type Properties = HashMap<String, OwnedValue>;

#[derive(Clone, Copy)]
pub(super) struct Target {
  pub node: u32,
  pub serial: Option<u64>,
}

struct Prepared {
  streams: Vec<(CaptureSource, Target)>,
  fd: OwnedFd,
  restore_token: Option<CaptureRestoreToken>,
}

fn token() -> String {
  format!(
    "toolbox_{}_{}",
    std::process::id(),
    NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
  )
}

fn dbus_error(error: zbus::Error) -> CaptureError {
  if let zbus::Error::MethodError(name, _, _) = &error {
    match name.as_str() {
      "org.freedesktop.portal.Error.NotAllowed" | "org.freedesktop.DBus.Error.AccessDenied" => {
        return CaptureError::PermissionDenied;
      }
      "org.freedesktop.portal.Error.Cancelled" => return CaptureError::Cancelled,
      _ => {}
    }
  }
  caused_by("Portal D-Bus 调用失败", error)
}

async fn connection() -> Result<Connection, CaptureError> {
  let builder = zbus::connection::Builder::session()
    .map_err(dbus_error)?
    .method_timeout(CALL_TIMEOUT);
  tokio::time::timeout(CALL_TIMEOUT, builder.build())
    .await
    .map_err(|_| backend("连接会话 D-Bus 超时"))?
    .map_err(dbus_error)
}

async fn proxy(connection: &Connection) -> Result<Proxy<'_>, CaptureError> {
  Proxy::new(connection, DESTINATION, PATH, INTERFACE)
    .await
    .map_err(dbus_error)
}

pub(super) fn map_capabilities(version: u32, types: u32, cursors: u32) -> CaptureCapabilities {
  let mut picker_kinds = Vec::new();
  if types & 1 != 0 {
    picker_kinds.push(SelectionKind::Display);
  }
  if types & 2 != 0 {
    picker_kinds.push(SelectionKind::Window);
  }
  // v3 以前 Any 的混合选择无法确认实际类型，不能伪造 CaptureSource.kind。
  if types & 3 != 0 && (version >= 3 || types & 3 != 3) {
    picker_kinds.push(SelectionKind::Any);
  }
  let mut cursor_modes = Vec::new();
  if cursors & 1 != 0 {
    cursor_modes.push(CursorMode::Hidden);
  }
  if cursors & 2 != 0 {
    cursor_modes.push(CursorMode::Embedded);
  }
  let restore_kinds = if version >= 4 {
    picker_kinds.clone()
  } else {
    vec![]
  };
  let mut persistence_modes = vec![CapturePersistence::None];
  if version >= 4 {
    persistence_modes.extend([
      CapturePersistence::Application,
      CapturePersistence::UntilRevoked,
    ]);
  }
  CaptureCapabilities {
    enumerable_source_kinds: vec![],
    direct_source_kinds: vec![],
    multiple_picker_kinds: picker_kinds.clone(),
    picker_kinds,
    cursor_modes,
    multiple_restore_kinds: restore_kinds.clone(),
    restore_kinds,
    persistence_modes,
  }
}

/// 本地校验，不消费凭据；格式错误不能被当作普通恢复失败而弹出选择器。
pub(super) fn restore_token(target: &CaptureTarget) -> Result<Option<&str>, CaptureError> {
  let CaptureTarget::RestoreOrPicker { token, .. } = target else {
    return Ok(None);
  };
  let payload = token
    .payload_for(RESTORE_BACKEND, RESTORE_FORMAT_VERSION)
    .ok_or(CaptureError::InvalidRestoreToken)?;
  let token = std::str::from_utf8(payload).map_err(|_| CaptureError::InvalidRestoreToken)?;
  if token.is_empty() || token.contains('\0') {
    return Err(CaptureError::InvalidRestoreToken);
  }
  Ok(Some(token))
}

fn selection_options<'a>(
  version: u32,
  types: u32,
  cursors: u32,
  options: &'a CaptureOptions,
  handle_token: &'a str,
) -> Result<(u32, HashMap<&'static str, Value<'a>>), CaptureError> {
  let capabilities = map_capabilities(version, types, cursors);
  let kind = match &options.target {
    CaptureTarget::Picker(kind) => {
      if !capabilities.picker_kinds.contains(kind) {
        return Err(CaptureError::Unsupported(CaptureFeature::Picker(*kind)));
      }
      if options.multiplicity == CaptureMultiplicity::Multiple
        && !capabilities.multiple_picker_kinds.contains(kind)
      {
        return Err(CaptureError::Unsupported(
          CaptureFeature::MultipleSelection(*kind),
        ));
      }
      *kind
    }
    CaptureTarget::RestoreOrPicker { kind, .. } => {
      if !capabilities.restore_kinds.contains(kind) {
        return Err(CaptureError::Unsupported(CaptureFeature::Restore(*kind)));
      }
      if options.multiplicity == CaptureMultiplicity::Multiple
        && !capabilities.multiple_restore_kinds.contains(kind)
      {
        return Err(CaptureError::Unsupported(
          CaptureFeature::MultipleSelection(*kind),
        ));
      }
      *kind
    }
    CaptureTarget::Source(_) => {
      return Err(CaptureError::Unsupported(
        CaptureFeature::DirectSourceSelection,
      ));
    }
  };
  if !capabilities.cursor_modes.contains(&options.cursor) {
    return Err(CaptureError::Unsupported(CaptureFeature::Cursor(
      options.cursor,
    )));
  }
  if !capabilities
    .persistence_modes
    .contains(&options.persistence)
  {
    return Err(CaptureError::Unsupported(CaptureFeature::Persistence(
      options.persistence,
    )));
  }
  let selected_types = match kind {
    SelectionKind::Display => 1,
    SelectionKind::Window => 2,
    SelectionKind::Any => types & 3,
  };
  let mut values = HashMap::from([
    ("handle_token", Value::from(handle_token)),
    ("types", Value::from(selected_types)),
    (
      "multiple",
      Value::from(options.multiplicity == CaptureMultiplicity::Multiple),
    ),
  ]);
  if version >= 2 {
    values.insert(
      "cursor_mode",
      Value::from(match options.cursor {
        CursorMode::Hidden => 1u32,
        CursorMode::Embedded => 2u32,
      }),
    );
  }
  if version >= 4 {
    values.insert(
      "persist_mode",
      Value::from(match options.persistence {
        CapturePersistence::None => 0u32,
        CapturePersistence::Application => 1u32,
        CapturePersistence::UntilRevoked => 2u32,
      }),
    );
  }
  if let Some(token) = restore_token(&options.target)? {
    values.insert("restore_token", Value::from(token));
  }
  Ok((selected_types, values))
}

fn returned_restore_token(
  result: &Properties,
  persistence: CapturePersistence,
) -> Result<Option<CaptureRestoreToken>, CaptureError> {
  // 不申请持久化时不交付凭据；申请但未获批准不会使捕获失败。
  if persistence == CapturePersistence::None {
    return Ok(None);
  }
  let Some(value) = result.get("restore_token") else {
    return Ok(None);
  };
  let token = <&str>::try_from(value).map_err(|_| backend("Portal 返回的恢复凭据类型无效"))?;
  if token.is_empty() || token.contains('\0') {
    return Err(backend("Portal 返回的恢复凭据内容无效"));
  }
  Ok(Some(CaptureRestoreToken::new(
    RESTORE_BACKEND,
    RESTORE_FORMAT_VERSION,
    token.as_bytes().to_vec(),
  )))
}

fn selected_streams(
  result: &Properties,
  version: u32,
  selected_types: u32,
  multiplicity: CaptureMultiplicity,
  session_token: &str,
) -> Result<Vec<(CaptureSource, Target)>, CaptureError> {
  let streams: Vec<(u32, Properties)> = result
    .get("streams")
    .ok_or_else(|| backend("Portal 未返回来源"))?
    .try_clone()
    .map_err(|error| caused_by("无法复制 Portal 来源元数据", error))?
    .try_into()
    .map_err(|error| caused_by("Portal 来源元数据无效", error))?;
  if streams.is_empty() {
    return Err(backend("Portal 必须返回至少一个来源"));
  }
  if multiplicity == CaptureMultiplicity::Single && streams.len() != 1 {
    return Err(backend("单选请求必须返回恰好一个来源"));
  }
  let mut nodes = HashSet::new();
  let mut serials = HashSet::new();
  streams
    .into_iter()
    .enumerate()
    .map(|(index, (node, properties))| {
      let source_type = properties
        .get("source_type")
        .map(u32::try_from)
        .transpose()
        .map_err(|error| caused_by("Portal 来源类型无效", error))?
        .unwrap_or(selected_types);
      let kind = match source_type {
        1 => SourceKind::Display,
        2 => SourceKind::Window,
        _ => return Err(backend("Portal 未给出可确认的来源类型")),
      };
      if selected_types & source_type == 0 {
        return Err(backend("Portal 选择的来源不满足类型过滤"));
      }
      let serial = properties
        .get("pipewire-serial")
        .map(u64::try_from)
        .transpose()
        .map_err(|error| caused_by("PipeWire serial 无效", error))?;
      if version >= 6 && serial.is_none() {
        return Err(backend("Portal v6 未返回 PipeWire serial"));
      }
      if !nodes.insert(node) || serial.is_some_and(|serial| !serials.insert(serial)) {
        return Err(backend("Portal 返回了重复的捕获来源"));
      }
      let source = CaptureSource {
        title: String::new(),
        id: SourceId(format!("pipewire:{session_token}:{index}")),
        kind,
      };
      Ok((source, Target { node, serial }))
    })
    .collect()
}

async fn properties(proxy: &Proxy<'_>) -> Result<(u32, u32, u32), CaptureError> {
  let version: u32 = proxy.get_property("version").await.map_err(dbus_error)?;
  let types = proxy
    .get_property("AvailableSourceTypes")
    .await
    .map_err(dbus_error)?;
  let cursors = if version >= 2 {
    proxy
      .get_property("AvailableCursorModes")
      .await
      .map_err(dbus_error)?
  } else {
    1
  };
  Ok((version, types, cursors))
}

pub(super) async fn capabilities() -> Result<CaptureCapabilities, CaptureError> {
  let connection = connection().await?;
  let proxy = proxy(&connection).await?;
  let (version, types, cursors) = properties(&proxy).await?;
  Ok(map_capabilities(version, types, cursors))
}

struct Portal {
  connection: Connection,
  session: Option<OwnedObjectPath>,
  request: Option<OwnedObjectPath>,
  closed: Option<zbus::proxy::SignalStream<'static>>,
  sender: String,
}

impl Portal {
  async fn new() -> Result<Self, CaptureError> {
    let connection = connection().await?;
    let sender = connection
      .unique_name()
      .ok_or_else(|| backend("D-Bus 未分配唯一名称"))?
      .as_str()
      .trim_start_matches(':')
      .replace('.', "_");
    Ok(Self {
      connection,
      session: None,
      request: None,
      closed: None,
      sender,
    })
  }

  fn handle(&self, kind: &str, token: &str) -> Result<OwnedObjectPath, CaptureError> {
    OwnedObjectPath::try_from(format!("{PATH}/{kind}/{}/{token}", self.sender))
      .map_err(|error| caused_by("Portal 对象路径无效", error))
  }

  async fn request<B>(
    &mut self,
    method: &str,
    token: &str,
    body: &B,
    stop: &mut watch::Receiver<bool>,
  ) -> Result<Properties, CaptureError>
  where
    B: serde::Serialize + zbus::zvariant::DynamicType,
  {
    if *stop.borrow() {
      return Err(CaptureError::Cancelled);
    }
    let path = self.handle("request", token)?;
    self.request = Some(path.clone());
    let request = Proxy::new(
      &self.connection,
      DESTINATION,
      path.clone(),
      "org.freedesktop.portal.Request",
    )
    .await
    .map_err(dbus_error)?;
    // 先订阅，避免响应快于方法返回时漏掉信号。
    let mut responses = request
      .receive_signal("Response")
      .await
      .map_err(dbus_error)?;
    let screen = proxy(&self.connection).await?;
    // 派发中的方法不能因 start future 被丢弃而中止。等待返回的句柄，
    // 然后关闭请求及预先记录的 session；方法本身由连接限制为 10 秒。
    let actual: OwnedObjectPath = screen.call(method, body).await.map_err(dbus_error)?;
    if actual != path {
      self.request = Some(actual);
      return Err(backend("Portal 返回了非约定的请求路径"));
    }
    if *stop.borrow() {
      return Err(CaptureError::Cancelled);
    }
    let message = tokio::select! {
      biased;
      _ = stop.changed() => return Err(CaptureError::Cancelled),
      _ = async { self.closed.as_mut().unwrap().next().await }, if self.closed.is_some() => return Err(CaptureError::SessionClosed),
      response = responses.next() => response.ok_or_else(|| backend("Portal 请求未返回响应"))?,
    };
    let (status, results): (u32, Properties) = message.body().deserialize().map_err(dbus_error)?;
    self.request = None;
    match status {
      0 => Ok(results),
      1 => Err(CaptureError::Cancelled),
      // Response 2 也可能是后端失败，不能一律解释为拒绝权限。
      _ => Err(backend(format!(
        "Portal 请求 {method} 失败（响应 {status}）"
      ))),
    }
  }

  async fn prepare(
    &mut self,
    parent: &str,
    options: &CaptureOptions,
    stop: &mut watch::Receiver<bool>,
  ) -> Result<Prepared, CaptureError> {
    let (version, types, cursors) = properties(&proxy(&self.connection).await?).await?;
    let select_handle_token = token();
    // 全部选项在创建授权会话前校验，旧版 Portal 不接收 v4 选项。
    let (selected_types, values) =
      selection_options(version, types, cursors, options, &select_handle_token)?;
    let session_token = token();
    let session = self.handle("session", &session_token)?;
    // 在创建请求前记录路径，取消时也能关闭迟到创建的会话。
    self.session = Some(session.clone());
    let session_proxy = Proxy::new(
      &self.connection,
      DESTINATION,
      session.clone(),
      "org.freedesktop.portal.Session",
    )
    .await
    .map_err(dbus_error)?;
    self.closed = Some(
      session_proxy
        .receive_signal("Closed")
        .await
        .map_err(dbus_error)?,
    );
    let handle_token = token();
    let result = self
      .request(
        "CreateSession",
        &handle_token,
        &(HashMap::from([
          ("handle_token", Value::from(handle_token.as_str())),
          ("session_handle_token", Value::from(session_token.as_str())),
        ]),),
        stop,
      )
      .await?;
    let actual = result
      .get("session_handle")
      .and_then(|value| <&str>::try_from(value).ok());
    if actual != Some(session.as_str()) {
      if let Some(actual) = actual {
        self.session = Some(
          OwnedObjectPath::try_from(actual.to_owned())
            .map_err(|error| caused_by("Portal 会话路径无效", error))?,
        );
      }
      return Err(backend("Portal 返回了非约定的会话路径"));
    }
    self
      .request(
        "SelectSources",
        &select_handle_token,
        &(&session, values),
        stop,
      )
      .await?;
    let handle_token = token();
    let result = self
      .request(
        "Start",
        &handle_token,
        &(
          &session,
          parent,
          HashMap::from([("handle_token", Value::from(handle_token.as_str()))]),
        ),
        stop,
      )
      .await?;
    let restore_token = returned_restore_token(&result, options.persistence)?;
    let streams = selected_streams(
      &result,
      version,
      selected_types,
      options.multiplicity,
      &session_token,
    )?;
    if *stop.borrow() {
      return Err(CaptureError::Cancelled);
    }
    let fd: zbus::zvariant::OwnedFd = proxy(&self.connection)
      .await?
      .call(
        "OpenPipeWireRemote",
        &(&session, HashMap::<&str, Value<'_>>::new()),
      )
      .await
      .map_err(dbus_error)?;
    Ok(Prepared {
      streams,
      fd: fd.into(),
      restore_token,
    })
  }

  async fn close_object(
    &self,
    path: &OwnedObjectPath,
    interface: &'static str,
  ) -> Result<(), CaptureError> {
    let object = Proxy::new(&self.connection, DESTINATION, path.clone(), interface)
      .await
      .map_err(dbus_error)?;
    match object.call::<_, _, ()>("Close", &()).await {
      Ok(()) => Ok(()),
      Err(zbus::Error::MethodError(name, _, _))
        if matches!(
          name.as_str(),
          "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.portal.Error.NotFound"
            | "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
        ) =>
      {
        Ok(())
      }
      Err(error) => Err(dbus_error(error)),
    }
  }

  async fn close(&mut self) -> Result<(), CaptureError> {
    // 即使请求关闭失败，也继续尝试释放会话。
    let request_result = if let Some(path) = &self.request {
      self
        .close_object(path, "org.freedesktop.portal.Request")
        .await
    } else {
      Ok(())
    };
    if request_result.is_ok() {
      self.request = None;
    }
    let session_result = if let Some(path) = &self.session {
      self
        .close_object(path, "org.freedesktop.portal.Session")
        .await
    } else {
      Ok(())
    };
    if session_result.is_ok() {
      self.session = None;
    }
    session_result.and(request_result)
  }
}

pub(super) async fn supervise(
  parent: String,
  options: CaptureOptions,
  control: Arc<Control>,
  ready: oneshot::Sender<Result<Box<dyn CaptureSession>, CaptureError>>,
) {
  let mut stop = control.stop.subscribe();
  let mut portal = match Portal::new().await {
    Ok(portal) => portal,
    Err(error) => {
      control.request_stop();
      control.cleanup.send_replace(Cleanup::Released);
      let _ = ready.send(Err(error));
      return;
    }
  };
  let mut ready = Some(ready);
  let result = run(
    &mut portal,
    &parent,
    options,
    &control,
    &mut stop,
    &mut ready,
  )
  .await;
  let mut startup_error = None;
  if let Err(error) = result {
    if ready.is_some() {
      startup_error = Some(control.take_error().unwrap_or(error));
    } else {
      control.fail(error);
    }
  }
  control.request_stop();
  loop {
    control.cleanup.send_replace(Cleanup::Running);
    match portal.close().await {
      Ok(()) => {
        control.cleanup.send_replace(Cleanup::Released);
        break;
      }
      Err(error) => {
        control
          .cleanup
          .send_replace(Cleanup::Failed(error.to_string()));
      }
    }
    // 控制句柄仍持有时，下一次 stop 可重试；Drop 后也定时重试，
    // supervisor 不随调用者 future 的取消而终止。
    tokio::select! { _ = stop.changed() => {}, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
  }
  if let Some(ready) = ready {
    let _ = ready.send(Err(startup_error.unwrap_or(CaptureError::Cancelled)));
  }
}

async fn run(
  portal: &mut Portal,
  parent: &str,
  options: CaptureOptions,
  control: &Arc<Control>,
  stop: &mut watch::Receiver<bool>,
  ready: &mut Option<oneshot::Sender<Result<Box<dyn CaptureSession>, CaptureError>>>,
) -> Result<(), CaptureError> {
  let Prepared {
    streams,
    fd,
    restore_token,
  } = portal.prepare(parent, &options, stop).await?;
  let mut closed = portal.closed.take().unwrap();
  let (native_ready, mut started) = oneshot::channel();
  let worker_control = control.clone();
  let sources: Vec<_> = streams.iter().map(|(source, _)| source.clone()).collect();
  // 原生回调开始前建立每个来源的独立邮箱，保留尚未取出流时的首帧及错误。
  control.register_sources(&sources);
  let thread = std::thread::Builder::new()
    .name("toolbox-pipewire".into())
    .spawn(move || runtime::run(fd, streams, options, worker_control, native_ready))
    .map_err(|error| caused_by("无法创建 PipeWire 线程", error))?;
  let mut joined = tokio::task::spawn_blocking(move || {
    thread
      .join()
      .map_err(|_| backend("PipeWire 线程发生 panic"))?
  });
  let startup = tokio::select! {
    biased;
    _ = stop.changed() => Err(CaptureError::Cancelled),
    _ = closed.next() => Err(CaptureError::SessionClosed),
    result = &mut started => result.map_err(|_| backend("PipeWire 在启动前结束")),
    _ = tokio::time::sleep(CALL_TIMEOUT) => Err(backend("PipeWire 连接来源超时")),
  };
  match startup {
    Ok(sources) if !control.stopped.load(Ordering::Acquire) => {
      let session: Box<dyn CaptureSession> =
        Box::new(Session::new(sources, restore_token, control.clone()));
      if ready.take().unwrap().send(Ok(session)).is_ok() {
        tokio::select! {
          _ = stop.changed() => {},
          _ = closed.next() => { control.fail(CaptureError::SessionClosed); },
          result = &mut joined => {
            return result.map_err(|error| caused_by("PipeWire 清理任务失败", error))?;
          }
        }
      }
    }
    Ok(_) => {}
    Err(error) => {
      control.request_stop();
      joined
        .await
        .map_err(|error| caused_by("PipeWire 清理任务失败", error))??;
      return Err(error);
    }
  }
  control.request_stop();
  joined
    .await
    .map_err(|error| caused_by("PipeWire 清理任务失败", error))?
}

#[cfg(test)]
mod tests {
  use super::*;

  fn picker_options() -> CaptureOptions {
    CaptureOptions::new(
      CaptureTarget::Picker(SelectionKind::Window),
      CursorMode::Embedded,
      Some(30),
    )
  }

  fn start_result(token: &str) -> Properties {
    HashMap::from([(
      "restore_token".into(),
      Value::from(token).try_to_owned().unwrap(),
    )])
  }

  fn streams_result(entries: &[(u32, Option<u32>, Option<u64>)]) -> Properties {
    let streams: Vec<(u32, Properties)> = entries
      .iter()
      .map(|&(node, kind, serial)| {
        let mut properties = Properties::new();
        if let Some(kind) = kind {
          properties.insert("source_type".into(), kind.into());
        }
        if let Some(serial) = serial {
          properties.insert("pipewire-serial".into(), serial.into());
        }
        (node, properties)
      })
      .collect();
    HashMap::from([(
      "streams".into(),
      Value::new(streams).try_to_owned().unwrap(),
    )])
  }

  #[test]
  fn selects_multiple_sources_in_one_request_and_restores_with_one_group_token() {
    let options = picker_options().with_multiplicity(CaptureMultiplicity::Multiple);
    let (_, values) = selection_options(3, 3, 3, &options, "request").unwrap();
    assert!(bool::try_from(&values["multiple"]).unwrap());
    assert!(!values.contains_key("restore_token"));

    let token = returned_restore_token(
      &start_result("group-token"),
      CapturePersistence::UntilRevoked,
    )
    .unwrap()
    .unwrap();
    let options = CaptureOptions::new(
      CaptureTarget::RestoreOrPicker {
        token,
        kind: SelectionKind::Any,
      },
      CursorMode::Hidden,
      None,
    )
    .with_multiplicity(CaptureMultiplicity::Multiple)
    .with_persistence(CapturePersistence::UntilRevoked);
    let (types, values) = selection_options(6, 3, 3, &options, "request").unwrap();
    assert_eq!(types, 3);
    assert!(bool::try_from(&values["multiple"]).unwrap());
    assert_eq!(
      <&str>::try_from(&values["restore_token"]).unwrap(),
      "group-token"
    );
    assert_eq!(u32::try_from(&values["persist_mode"]).unwrap(), 2);
    let result = streams_result(&[(10, Some(1), Some(100)), (11, Some(2), Some(101))]);
    let sources = selected_streams(&result, 6, types, options.multiplicity, "session").unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].0.kind, SourceKind::Display);
    assert_eq!(sources[1].0.kind, SourceKind::Window);
    assert_ne!(sources[0].0.id, sources[1].0.id);
    assert_eq!(sources[0].1.serial, Some(100));
    assert_eq!(sources[1].1.serial, Some(101));
  }

  #[test]
  fn enforces_single_and_multiple_source_counts_without_truncating() {
    let one = streams_result(&[(10, Some(2), None)]);
    let two = streams_result(&[(10, Some(2), None), (11, Some(2), None)]);
    let empty = streams_result(&[]);
    for multiplicity in [CaptureMultiplicity::Single, CaptureMultiplicity::Multiple] {
      assert_eq!(
        selected_streams(&one, 4, 2, multiplicity, "session")
          .unwrap()
          .len(),
        1
      );
      assert!(selected_streams(&empty, 4, 2, multiplicity, "session").is_err());
    }
    assert!(selected_streams(&two, 4, 2, CaptureMultiplicity::Single, "session").is_err());
    assert_eq!(
      selected_streams(&two, 4, 2, CaptureMultiplicity::Multiple, "session")
        .unwrap()
        .len(),
      2
    );
  }

  #[test]
  fn rejects_the_whole_selection_when_any_source_is_invalid_or_duplicated() {
    for entries in [
      vec![(10, Some(2), Some(100)), (11, Some(1), Some(101))], // 错误类型。
      vec![(10, Some(2), Some(100)), (11, Some(2), None)],      // 缺少 v6 serial。
      vec![(10, Some(2), Some(100)), (10, Some(2), Some(101))], // 重复 node。
      vec![(10, Some(2), Some(100)), (11, Some(2), Some(100))], // 重复 serial。
      vec![(10, Some(2), Some(100)), (11, Some(4), Some(101))], // 未支持的来源。
    ] {
      assert!(
        selected_streams(
          &streams_result(&entries),
          6,
          2,
          CaptureMultiplicity::Multiple,
          "session"
        )
        .is_err()
      );
    }
  }

  #[test]
  fn accepts_legacy_multi_selection_when_each_source_type_is_known() {
    let result = streams_result(&[(10, None, None), (11, None, None)]);
    let sources =
      selected_streams(&result, 2, 2, CaptureMultiplicity::Multiple, "session").unwrap();
    assert!(
      sources
        .iter()
        .all(|(source, target)| source.kind == SourceKind::Window && target.serial.is_none())
    );
    assert!(selected_streams(&result, 2, 3, CaptureMultiplicity::Multiple, "session").is_err());
  }

  #[test]
  fn preserves_legacy_picker_options_and_maps_persistence_requests() {
    let options = picker_options();
    let (types, values) = selection_options(3, 3, 3, &options, "request").unwrap();
    assert_eq!(types, 2);
    assert_eq!(u32::try_from(&values["types"]).unwrap(), 2);
    assert!(!bool::try_from(&values["multiple"]).unwrap());
    assert_eq!(u32::try_from(&values["cursor_mode"]).unwrap(), 2);
    assert!(!values.contains_key("restore_token"));
    assert!(!values.contains_key("persist_mode"));

    for (persistence, expected) in [
      (CapturePersistence::None, 0),
      (CapturePersistence::Application, 1),
      (CapturePersistence::UntilRevoked, 2),
    ] {
      let options = picker_options().with_persistence(persistence);
      let (_, values) = selection_options(4, 3, 3, &options, "request").unwrap();
      assert_eq!(u32::try_from(&values["persist_mode"]).unwrap(), expected);
      assert!(!values.contains_key("restore_token"));
    }
  }

  #[test]
  fn restores_with_old_token_and_returns_a_serializable_replacement() {
    let old = returned_restore_token(&start_result("old-token"), CapturePersistence::UntilRevoked)
      .unwrap()
      .unwrap();
    let stored = serde_json::to_string(&old).unwrap();
    let options = CaptureOptions::new(
      CaptureTarget::RestoreOrPicker {
        token: serde_json::from_str(&stored).unwrap(),
        kind: SelectionKind::Window,
      },
      CursorMode::Hidden,
      None,
    )
    .with_persistence(CapturePersistence::UntilRevoked);
    let (types, values) = selection_options(4, 3, 3, &options, "request").unwrap();
    assert_eq!(types, 2);
    assert_eq!(
      <&str>::try_from(&values["restore_token"]).unwrap(),
      "old-token"
    );
    assert_eq!(u32::try_from(&values["persist_mode"]).unwrap(), 2);
    assert!(!bool::try_from(&values["multiple"]).unwrap());

    let new = returned_restore_token(&start_result("new-token"), CapturePersistence::UntilRevoked)
      .unwrap()
      .unwrap();
    assert_ne!(new, old);
    assert_eq!(new.payload_for(RESTORE_BACKEND, 1), Some(&b"new-token"[..]));
  }

  #[test]
  fn rejects_restore_and_persistence_on_legacy_portals() {
    let options = picker_options().with_persistence(CapturePersistence::UntilRevoked);
    assert!(matches!(
      selection_options(3, 3, 3, &options, "request"),
      Err(CaptureError::Unsupported(CaptureFeature::Persistence(
        CapturePersistence::UntilRevoked
      )))
    ));
    let token = CaptureRestoreToken::new(RESTORE_BACKEND, 1, b"token".to_vec());
    let options = CaptureOptions::new(
      CaptureTarget::RestoreOrPicker {
        token,
        kind: SelectionKind::Window,
      },
      CursorMode::Hidden,
      None,
    );
    for (version, types) in [(3, 3), (4, 1)] {
      assert!(matches!(
        selection_options(version, types, 3, &options, "request"),
        Err(CaptureError::Unsupported(CaptureFeature::Restore(
          SelectionKind::Window
        )))
      ));
    }
  }

  #[test]
  fn rejects_incompatible_or_malformed_tokens_without_exposing_payload() {
    for token in [
      CaptureRestoreToken::new("foreign-backend", 1, b"secret".to_vec()),
      CaptureRestoreToken::new(RESTORE_BACKEND, 2, b"secret".to_vec()),
      CaptureRestoreToken::new(RESTORE_BACKEND, 1, vec![0xff]),
      CaptureRestoreToken::new(RESTORE_BACKEND, 1, vec![]),
      CaptureRestoreToken::new(RESTORE_BACKEND, 1, b"secret\0".to_vec()),
    ] {
      let options = CaptureOptions::new(
        CaptureTarget::RestoreOrPicker {
          token,
          kind: SelectionKind::Display,
        },
        CursorMode::Hidden,
        None,
      );
      let error = selection_options(4, 3, 3, &options, "request").unwrap_err();
      assert!(matches!(error, CaptureError::InvalidRestoreToken));
      assert!(!format!("{error:?}").contains("secret"));
    }
  }

  #[test]
  fn accepts_ungranted_persistence_and_does_not_return_unrequested_tokens() {
    assert!(
      returned_restore_token(&Properties::new(), CapturePersistence::UntilRevoked)
        .unwrap()
        .is_none()
    );
    assert!(
      returned_restore_token(&start_result("token"), CapturePersistence::None)
        .unwrap()
        .is_none()
    );
  }

  #[test]
  fn rejects_malformed_portal_tokens_without_logging_the_value() {
    for value in [Value::from(42u32), Value::from(""), Value::from("secret\0")] {
      let result = HashMap::from([("restore_token".into(), value.try_to_owned().unwrap())]);
      let error = returned_restore_token(&result, CapturePersistence::Application).unwrap_err();
      assert!(matches!(error, CaptureError::Backend { .. }));
      assert!(!format!("{error:?}").contains("secret"));
    }
  }
}
