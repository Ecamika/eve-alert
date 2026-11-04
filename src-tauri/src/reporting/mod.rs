//! 图像识别与报告业务；平台捕获通过 Screencast 契约接入。
mod capture;
mod config;
mod outputs;
#[cfg(test)]
mod tests;

use crate::platform::screencast::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use config::{Config, ReminderStage, ReminderState, ReportMethod, WarningState};
use futures_util::StreamExt;
use image::ImageFormat;
use serde::Serialize;
use std::{
  collections::HashMap,
  io::Cursor,
  sync::{Arc, RwLock},
};
use tauri::{AppHandle, Emitter, State};
use tokio::{
  sync::{Mutex, broadcast, watch},
  task::JoinHandle,
};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum ReportEvent {
  Warn {
    title: String,
  },
  WarnCleared {
    title: String,
  },
  Reminder {
    title: String,
    stage: ReminderStage,
    message: &'static str,
  },
}
impl ReportEvent {
  fn reminder(title: String, stage: ReminderStage) -> Self {
    Self::Reminder {
      title,
      stage,
      message: stage.message(),
    }
  }

  fn kind(&self) -> &str {
    match self {
      Self::Warn { .. } => "Warn",
      Self::WarnCleared { .. } => "WarnCleared",
      Self::Reminder { .. } => "Reminder",
    }
  }
  fn title(&self) -> &str {
    match self {
      Self::Warn { title } | Self::WarnCleared { title } | Self::Reminder { title, .. } => title,
    }
  }
  fn label(&self) -> &str {
    match self {
      Self::Warn { .. } => "告警",
      Self::WarnCleared { .. } => "告警解除",
      Self::Reminder { .. } => "提醒",
    }
  }

  fn message(&self) -> &str {
    match self {
      Self::Warn { .. } => "检测到告警颜色",
      Self::WarnCleared { .. } => "告警颜色已消失",
      Self::Reminder { message, .. } => message,
    }
  }

  fn notification_body(&self) -> String {
    match self {
      Self::Reminder { .. } => format!("{}：{}", self.title(), self.message()),
      _ => self.title().to_string(),
    }
  }

  fn onebot_text(&self) -> String {
    format!("{} {}", self.kind(), self.notification_body())
  }
}

struct CaptureWorker {
  task: JoinHandle<()>,
  latest: Arc<RwLock<Option<ArcCaptureFrame>>>,
  session: Arc<dyn CaptureSession>,
  ready: watch::Receiver<Option<Result<(), String>>>,
}
impl CaptureWorker {
  async fn wait_ready(&self) -> Result<(), String> {
    let mut ready = self.ready.clone();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
      ready
        .wait_for(|result| result.is_some())
        .await
        .map_err(|_| "捕获任务已结束".to_string())?
        .as_ref()
        .unwrap()
        .clone()
    })
    .await
    .map_err(|_| "等待捕获首帧超时，请检查来源或重新获取来源".to_string())?
  }

  async fn stop(mut self) -> Result<(), String> {
    self.task.abort();
    let _ = (&mut self.task).await;
    self.session.stop().await.map_err(|e| e.to_string())
  }
}
impl Drop for CaptureWorker {
  fn drop(&mut self) {
    self.task.abort();
  }
}

pub struct Reporting {
  operation: Mutex<()>,
  workers: Mutex<HashMap<String, CaptureWorker>>,
  active: Arc<RwLock<Option<Config>>>,
  outputs: Mutex<Option<outputs::Outputs>>,
  sender: broadcast::Sender<ReportEvent>,
}

impl Default for Reporting {
  fn default() -> Self {
    Self {
      operation: Mutex::new(()),
      workers: Mutex::new(HashMap::new()),
      active: Arc::new(RwLock::new(None)),
      outputs: Mutex::new(None),
      sender: broadcast::channel(128).0,
    }
  }
}

#[derive(Serialize)]
pub struct Settings {
  config: Config,
  path: String,
}

#[tauri::command]
pub async fn reporting_load(state: State<'_, Reporting>) -> Result<Settings, String> {
  let _guard = state.operation.lock().await;
  let path = config::config_path()?;
  let config = match std::fs::read(&path) {
    Ok(bytes) => {
      serde_json::from_slice::<Config>(&bytes).map_err(|e| format!("配置文件读取失败：{e}"))?
    }
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
    Err(e) => return Err(e.to_string()),
  };
  config.validate()?;
  Ok(Settings {
    config,
    path: path.display().to_string(),
  })
}

#[tauri::command]
pub async fn reporting_save(config: Config, state: State<'_, Reporting>) -> Result<String, String> {
  let _guard = state.operation.lock().await;
  if state.active.read().unwrap().is_some() {
    return Err("请先停止监控再修改配置".into());
  }
  config.validate()?;
  let path = config::config_path()?;
  std::fs::create_dir_all(path.parent().ok_or("配置目录无效")?).map_err(|e| e.to_string())?;
  let bytes = serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?;
  let temporary = path.with_extension("json.tmp");
  let mut options = std::fs::OpenOptions::new();
  options.create(true).truncate(true).write(true);
  #[cfg(unix)]
  {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
  }
  let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
  std::io::Write::write_all(&mut file, &bytes).map_err(|e| e.to_string())?;
  file.sync_all().map_err(|e| e.to_string())?;
  drop(file);
  std::fs::rename(temporary, &path).map_err(|e| e.to_string())?;
  Ok(path.display().to_string())
}

#[tauri::command]
pub async fn reporting_select(
  title: String,
  reselect: Option<bool>,
  app: AppHandle,
  state: State<'_, Reporting>,
) -> Result<(), String> {
  let _guard = state.operation.lock().await;
  if state.active.read().unwrap().is_some() {
    return Err("请先停止监控再重新选源".into());
  }
  if title.trim().is_empty() {
    return Err("请先填写角色名称".into());
  }
  select_capture(title, reselect.unwrap_or(false), app, &state).await
}

async fn select_capture(
  title: String,
  reselect: bool,
  app: AppHandle,
  state: &Reporting,
) -> Result<(), String> {
  #[cfg(target_os = "linux")]
  let backend = crate::platform::screencast::pipewire::PipeWireScreencast::default();
  #[cfg(not(target_os = "linux"))]
  return Err("当前项目仅实现 Linux PipeWire 屏幕捕获".into());
  #[cfg(target_os = "linux")]
  {
    let mut store = capture::RestoreStore::load()?;
    let mut session = capture::start(&backend, &mut store, &title, reselect).await?;
    let source = session
      .sources()
      .first()
      .ok_or("未选中捕获来源")?
      .id
      .clone();
    let mut frames = session.take_frames(&source).ok_or("无法取得画面流")?;
    let session: Arc<dyn CaptureSession> = Arc::from(session);
    let task_session = session.clone();
    let latest = Arc::new(RwLock::new(None));
    let latest_clone = latest.clone();
    let active = state.active.clone();
    let sender = state.sender.clone();
    let worker_title = title.clone();
    let (ready_sender, ready) = watch::channel(None);
    let task = tokio::spawn(async move {
      let mut reminder = ReminderState::default();
      let mut warning = WarningState::default();
      let mut invalid_dimensions = false;
      while let Some(frame) = frames.next().await {
        let frame = match frame {
          Ok(frame) => frame,
          Err(e) => {
            ready_sender.send_replace(Some(Err(e.to_string())));
            outputs::report_error(&app, format!("{worker_title} 捕获失败：{e}"));
            break;
          }
        };
        let character = active
          .read()
          .unwrap()
          .as_ref()
          .and_then(|c| c.characters.iter().find(|c| c.title == worker_title))
          .cloned();
        if let Some(character) = character {
          if !character.fits(&frame.image) {
            if !invalid_dimensions {
              outputs::report_error(
                &app,
                format!("{worker_title} 的画面尺寸已变化，识别区域越界；请停止监控并重新配置"),
              );
            }
            invalid_dimensions = true;
            reminder = ReminderState::default();
            *latest_clone.write().unwrap() = Some(frame);
            ready_sender.send_replace(Some(Ok(())));
            continue;
          }
          invalid_dimensions = false;
          let mut events = Vec::new();
          if let Some(matched) = warning.advance(&character.warn_region, &frame.image) {
            events.push(if matched {
              ReportEvent::Warn {
                title: worker_title.clone(),
              }
            } else {
              ReportEvent::WarnCleared {
                title: worker_title.clone(),
              }
            });
          }
          if let Some(stage) = reminder.advance(&character, &frame.image) {
            events.push(ReportEvent::reminder(worker_title.clone(), stage));
          }
          for event in events {
            let _ = sender.send(event.clone());
            let _ = app.emit("reporting-event", event);
          }
        } else {
          reminder = ReminderState::default();
          warning = WarningState::default();
        }
        *latest_clone.write().unwrap() = Some(frame);
        if ready_sender.borrow().is_none() {
          ready_sender.send_replace(Some(Ok(())));
        }
      }
      *latest_clone.write().unwrap() = None;
      ready_sender.send_replace(Some(Err("捕获已结束，请恢复或重新获取来源".into())));
      let _ = task_session.stop().await;
      outputs::report_error(
        &app,
        format!("{worker_title} 的捕获已结束，请恢复或重新获取来源"),
      );
    });
    let old = state.workers.lock().await.insert(
      title,
      CaptureWorker {
        task,
        latest,
        session,
        ready,
      },
    );
    if let Some(old) = old {
      old.stop().await?;
    }
    Ok(())
  }
}

#[derive(Serialize)]
pub struct Snapshot {
  data_url: String,
  width: u32,
  height: u32,
}

#[tauri::command]
pub async fn reporting_snapshot(
  title: String,
  state: State<'_, Reporting>,
) -> Result<Snapshot, String> {
  let frame = state
    .workers
    .lock()
    .await
    .get(&title)
    .and_then(|w| w.latest.read().unwrap().clone())
    .ok_or("尚未收到画面，请先选择来源并等待首帧")?;
  tokio::task::spawn_blocking(move || {
    let mut bytes = Cursor::new(Vec::new());
    frame
      .image
      .write_to(&mut bytes, ImageFormat::Png)
      .map_err(|e| e.to_string())?;
    Ok(Snapshot {
      data_url: format!(
        "data:image/png;base64,{}",
        STANDARD.encode(bytes.into_inner())
      ),
      width: frame.image.width(),
      height: frame.image.height(),
    })
  })
  .await
  .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn reporting_release(title: String, state: State<'_, Reporting>) -> Result<(), String> {
  let _guard = state.operation.lock().await;
  if state.active.read().unwrap().is_some() {
    return Err("请先停止监控".into());
  }
  let worker = state.workers.lock().await.remove(&title);
  if let Some(worker) = worker {
    worker.stop().await?;
  }
  Ok(())
}

#[tauri::command]
pub async fn reporting_start(
  config: Config,
  app: AppHandle,
  state: State<'_, Reporting>,
) -> Result<(), String> {
  let _guard = state.operation.lock().await;
  config.validate()?;
  if state.active.read().unwrap().is_some() {
    return Err("监控已启动".into());
  }
  let config = config.into_monitoring()?;
  // 配置时可预览任意角色，监控启动后只保留已启用角色的捕获。
  let excluded = {
    let mut workers = state.workers.lock().await;
    let titles: Vec<_> = workers
      .keys()
      .filter(|title| !config.characters.iter().any(|c| c.title == title.as_str()))
      .cloned()
      .collect();
    titles
      .into_iter()
      .filter_map(|title| workers.remove(&title))
      .collect::<Vec<_>>()
  };
  let mut errors = Vec::new();
  for worker in excluded {
    if let Err(e) = worker.stop().await {
      errors.push(e);
    }
  }
  if !errors.is_empty() {
    return Err(errors.join("；"));
  }
  for character in &config.characters {
    let connected = state
      .workers
      .lock()
      .await
      .get(&character.title)
      .is_some_and(|worker| !worker.task.is_finished());
    if !connected {
      select_capture(character.title.clone(), false, app.clone(), &state).await?;
    }
  }
  {
    let workers = state.workers.lock().await;
    for c in &config.characters {
      let worker = workers
        .get(&c.title)
        .filter(|w| !w.task.is_finished())
        .ok_or_else(|| format!("{} 的捕获已结束", c.title))?;
      worker
        .wait_ready()
        .await
        .map_err(|e| format!("{}：{e}", c.title))?;
      let frame = worker
        .latest
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| format!("{} 尚无画面", c.title))?;
      if !c.fits(&frame.image) {
        return Err(format!("{} 的识别区域超出了画面边界", c.title));
      }
    }
  }
  let outputs = outputs::start(&config, &state.sender, &app).await?;
  *state.outputs.lock().await = Some(outputs);
  *state.active.write().unwrap() = Some(config);
  Ok(())
}

#[tauri::command]
pub async fn reporting_stop(state: State<'_, Reporting>) -> Result<(), String> {
  let _guard = state.operation.lock().await;
  *state.active.write().unwrap() = None;
  let outputs = state.outputs.lock().await.take();
  if let Some(outputs) = outputs {
    outputs.shutdown().await;
  }
  // 释放原生捕获资源，保留磁盘上的恢复凭据供下次启动使用。
  let workers: Vec<_> = state.workers.lock().await.drain().map(|(_, w)| w).collect();
  let mut errors = Vec::new();
  for worker in workers {
    if let Err(e) = worker.stop().await {
      errors.push(e);
    }
  }
  if errors.is_empty() {
    Ok(())
  } else {
    Err(errors.join("；"))
  }
}

#[tauri::command]
pub async fn reporting_export(
  title: String,
  state: State<'_, Reporting>,
) -> Result<String, String> {
  let frame = state
    .workers
    .lock()
    .await
    .get(&title)
    .and_then(|w| w.latest.read().unwrap().clone())
    .ok_or("尚无可导出的截图")?;
  let directory = config::config_path()?
    .parent()
    .ok_or("配置目录无效")?
    .join("screenshots");
  let name: String = title
    .chars()
    .map(|c| {
      if c.is_alphanumeric() || c == '-' || c == '_' {
        c
      } else {
        '_'
      }
    })
    .take(80)
    .collect();
  let timestamp = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map_err(|e| e.to_string())?
    .as_nanos();
  tokio::task::spawn_blocking(move || {
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let path = directory.join(format!("{name}-{timestamp}.png"));
    frame.image.save(&path).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
  })
  .await
  .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct Status {
  running: bool,
  sources: Vec<String>,
}
#[tauri::command]
pub async fn reporting_status(state: State<'_, Reporting>) -> Result<Status, String> {
  let running = state.active.read().unwrap().is_some();
  let sources = state
    .workers
    .lock()
    .await
    .iter()
    .filter(|(_, w)| !w.task.is_finished())
    .map(|(title, _)| title.clone())
    .collect();
  Ok(Status { running, sources })
}

#[tauri::command]
pub async fn reporting_audio(kind: String, state: State<'_, Reporting>) -> Result<String, String> {
  let path = {
    let config = state.active.read().unwrap();
    config
      .as_ref()
      .and_then(|c| {
        c.report_methods.iter().find_map(|m| match m {
          ReportMethod::Voice {
            warn_voice_path,
            reminder_voice_path,
          } => Some(if kind == "Warn" {
            warn_voice_path.clone()
          } else {
            reminder_voice_path.clone()
          }),
          _ => None,
        })
      })
      .ok_or("未启用声音报告")?
  };
  let metadata = tokio::fs::metadata(&path)
    .await
    .map_err(|e| e.to_string())?;
  if metadata.len() > 20 * 1024 * 1024 {
    return Err("音频文件不能超过 20 MiB".into());
  }
  let mime = match std::path::Path::new(&path)
    .extension()
    .and_then(|s| s.to_str())
    .unwrap_or("")
    .to_ascii_lowercase()
    .as_str()
  {
    "mp3" => "audio/mpeg",
    "wav" => "audio/wav",
    "ogg" | "oga" => "audio/ogg",
    "flac" => "audio/flac",
    "m4a" => "audio/mp4",
    _ => "application/octet-stream",
  };
  let bytes = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
  Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}
