use notify::Watcher;
use std::path::{Path, PathBuf};
use tokio::{sync::broadcast, task::JoinHandle};

pub mod watcher_source;

struct NotifyEventHandler {
  tx: flume::Sender<Result<notify::Event, notify::Error>>,
}

impl NotifyEventHandler {
  pub fn new(tx: flume::Sender<Result<notify::Event, notify::Error>>) -> Self {
    Self { tx }
  }
}

impl notify::EventHandler for NotifyEventHandler {
  fn handle_event(&mut self, event: notify::Result<notify::Event>) {
    let _ = self.tx.send(event);
  }
}

pub struct ChatLogs {
  pub path: PathBuf,
  pub watcher: notify::INotifyWatcher,
  pub handle: JoinHandle<anyhow::Result<()>>,
}

impl ChatLogs {
  pub fn start_with_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
    let (tx, rx) = flume::bounded::<Result<notify::Event, notify::Error>>(8);

    let notify_event_handler = NotifyEventHandler::new(tx);
    let mut watcher = notify::recommended_watcher(notify_event_handler)?;

    watcher.watch(path.as_ref(), notify::RecursiveMode::NonRecursive)?;

    let handle = tokio::spawn(Self::listener(rx));

    Ok(Self {
      path: path.as_ref().to_path_buf(),
      watcher,
      handle,
    })
  }
}

impl ChatLogs {
  async fn listener(
    notify_receiver: flume::Receiver<Result<notify::Event, notify::Error>>,
  ) -> anyhow::Result<()> {
    Ok(())
  }
}

struct ChatLogsListener {}

impl ChatLogsListener {
  async fn init(path: impl AsRef<Path>) -> anyhow::Result<Self> {
    let mut dir = tokio::fs::read_dir(path).await?;
    let mut file_name_vec = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await
      && let Ok(metadata) = entry.metadata().await
      && metadata.is_file()
    {
      let file_name = entry
        .file_name()
        .to_str()
        .map(|s| watcher_source::FileName::from_str(s))
        .flatten();

      if let Some(file_name) = file_name {
        file_name_vec.push((file_name, entry.path()));
      }
    }

    Ok(Self {})
  }
}
