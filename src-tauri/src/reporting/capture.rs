//! 业务层保存整组选源的恢复凭据；不保存会随 Portal 会话失效的 node ID。
use super::config;
use crate::platform::screencast::*;
use std::{collections::HashMap, path::PathBuf};

pub(super) struct RestoreStore {
  path: PathBuf,
  tokens: HashMap<String, CaptureRestoreToken>,
}

impl RestoreStore {
  pub fn load() -> Result<Self, String> {
    Self::open(config::config_path()?.with_file_name("reporting-capture.json"))
  }

  fn open(path: PathBuf) -> Result<Self, String> {
    let tokens = match std::fs::read(&path) {
      Ok(bytes) => serde_json::from_slice(&bytes)
        .map_err(|_| "捕获授权文件无效，请检查 reporting-capture.json".to_string())?,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
      Err(e) => return Err(format!("读取捕获授权失败：{e}")),
    };
    Ok(Self { path, tokens })
  }

  fn save(&self) -> Result<(), String> {
    std::fs::create_dir_all(self.path.parent().ok_or("捕获授权目录无效")?)
      .map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&self.tokens).map_err(|e| e.to_string())?;
    let temporary = self.path.with_extension("json.tmp");
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
    std::fs::rename(temporary, &self.path).map_err(|e| e.to_string())
  }
}

/// 调用方须串行调用。在请求之前落盘移除旧凭据，失败或取消也不能重用。
pub(super) async fn start(
  backend: &dyn Screencast,
  store: &mut RestoreStore,
  title: &str,
  reselect: bool,
) -> Result<Box<dyn CaptureSession>, String> {
  let saved = store.tokens.remove(title);
  store.save()?;
  let target = match saved.filter(|_| !reselect) {
    Some(token) => CaptureTarget::RestoreOrPicker {
      token,
      kind: SelectionKind::Any,
    },
    None => CaptureTarget::Picker(SelectionKind::Any),
  };
  let mut session = backend
    .start(
      CaptureOptions::new(target, CursorMode::Hidden, Some(2))
        .with_persistence(CapturePersistence::UntilRevoked),
    )
    .await
    .map_err(|e| e.to_string())?;
  if let Some(token) = session.take_restore_token() {
    store.tokens.insert(title.to_string(), token);
    if let Err(error) = store.save() {
      let _ = session.stop().await;
      return Err(format!("保存捕获授权失败：{error}"));
    }
  }
  Ok(session)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Mutex;

  #[derive(Default)]
  struct Backend {
    requests: Mutex<Vec<CaptureOptions>>,
    result: Mutex<Option<Result<Option<CaptureRestoreToken>, CaptureError>>>,
  }

  impl Screencast for Backend {
    fn capabilities(&self) -> CaptureFuture<'_, CaptureCapabilities> {
      unreachable!()
    }
    fn sources(&self) -> CaptureFuture<'_, Vec<CaptureSource>> {
      unreachable!()
    }
    fn start(&self, options: CaptureOptions) -> CaptureFuture<'_, Box<dyn CaptureSession>> {
      self.requests.lock().unwrap().push(options);
      let result = self.result.lock().unwrap().take().unwrap();
      Box::pin(async move { Ok(Box::new(Session(result?)) as Box<dyn CaptureSession>) })
    }
  }

  struct Session(Option<CaptureRestoreToken>);
  impl CaptureSession for Session {
    fn sources(&self) -> &[CaptureSource] {
      &[]
    }
    fn take_restore_token(&mut self) -> Option<CaptureRestoreToken> {
      self.0.take()
    }
    fn take_frames(&mut self, _: &SourceId) -> Option<FrameStream> {
      None
    }
    fn stop(&self) -> CaptureFuture<'_, ()> {
      Box::pin(async { Ok(()) })
    }
  }

  struct Directory(PathBuf);
  impl Directory {
    fn new() -> Self {
      use std::sync::atomic::{AtomicUsize, Ordering};
      static NEXT: AtomicUsize = AtomicUsize::new(0);
      Self(std::env::temp_dir().join(format!(
        "eve-capture-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
      )))
    }
    fn store(&self) -> RestoreStore {
      RestoreStore::open(self.0.join("capture.json")).unwrap()
    }
  }
  impl Drop for Directory {
    fn drop(&mut self) {
      let _ = std::fs::remove_dir_all(&self.0);
    }
  }

  fn token(value: &str) -> CaptureRestoreToken {
    CaptureRestoreToken::new("test", 1, value.as_bytes().to_vec())
  }

  #[tokio::test]
  async fn persists_after_stop_rotates_on_restore_and_allows_manual_picker() {
    let directory = Directory::new();
    let backend = Backend::default();
    *backend.result.lock().unwrap() = Some(Ok(Some(token("first"))));
    let session = start(&backend, &mut directory.store(), "角色", false)
      .await
      .unwrap();
    session.stop().await.unwrap();
    drop(session);

    *backend.result.lock().unwrap() = Some(Ok(Some(token("renewed"))));
    start(&backend, &mut directory.store(), "角色", false)
      .await
      .unwrap();
    assert_eq!(directory.store().tokens["角色"], token("renewed"));

    *backend.result.lock().unwrap() = Some(Ok(None));
    start(&backend, &mut directory.store(), "角色", true)
      .await
      .unwrap();
    assert!(!directory.store().tokens.contains_key("角色"));
    let requests = backend.requests.lock().unwrap();
    assert!(matches!(
      requests[0].target,
      CaptureTarget::Picker(SelectionKind::Any)
    ));
    assert_eq!(
      requests[1].target,
      CaptureTarget::RestoreOrPicker {
        token: token("first"),
        kind: SelectionKind::Any,
      }
    );
    assert!(matches!(
      requests[2].target,
      CaptureTarget::Picker(SelectionKind::Any)
    ));
    assert!(
      requests
        .iter()
        .all(|r| r.persistence == CapturePersistence::UntilRevoked)
    );
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      assert_eq!(
        std::fs::metadata(directory.store().path)
          .unwrap()
          .permissions()
          .mode()
          & 0o777,
        0o600
      );
    }
  }

  #[tokio::test]
  async fn failed_restore_removes_consumed_token_but_preserves_other_characters() {
    let directory = Directory::new();
    let mut store = directory.store();
    store.tokens.insert("角色".into(), token("consumed"));
    store.tokens.insert("其他角色".into(), token("other"));
    store.save().unwrap();
    let backend = Backend::default();
    *backend.result.lock().unwrap() = Some(Err(CaptureError::Cancelled));
    assert!(start(&backend, &mut store, "角色", false).await.is_err());
    let stored = directory.store();
    assert!(!stored.tokens.contains_key("角色"));
    assert_eq!(stored.tokens["其他角色"], token("other"));
  }
}
