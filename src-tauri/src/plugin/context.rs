use std::{
  any::Any,
  collections::HashMap,
  future::Future,
  sync::{Arc, Mutex},
  time::Duration,
};

use anyhow::bail;
use tokio::{sync::watch, task::JoinHandle};

use super::{
  PluginId, PluginInfo,
  service::{Service, ServiceError, ServiceId, ServiceRegistry},
};

/// 每次启动创建独立停止信号；旧任务不能误用下一次启动的信号。
#[derive(Clone)]
pub struct StopSignal(watch::Receiver<bool>);

impl StopSignal {
  pub async fn cancelled(&mut self) {
    while !*self.0.borrow_and_update() {
      if self.0.changed().await.is_err() {
        return;
      }
    }
  }

  pub fn is_cancelled(&self) -> bool {
    *self.0.borrow()
  }
}

#[derive(PartialEq, Eq)]
enum Phase {
  Idle,
  Starting,
  Running,
  Stopping,
}

struct Resources {
  phase: Phase,
  pending: HashMap<ServiceId, Service>,
  stop: watch::Sender<bool>,
  tasks: Vec<JoinHandle<anyhow::Result<()>>>,
}

pub struct PluginContext {
  id: PluginId,
  info: PluginInfo,
  services: Arc<ServiceRegistry>,
  resources: Mutex<Resources>,
}

impl PluginContext {
  pub(crate) fn new(id: PluginId, info: PluginInfo, services: Arc<ServiceRegistry>) -> Self {
    Self {
      id,
      info,
      services,
      resources: Mutex::new(Resources {
        phase: Phase::Idle,
        pending: HashMap::new(),
        stop: watch::channel(true).0,
        tasks: Vec::new(),
      }),
    }
  }

  pub fn plugin_id(&self) -> &PluginId {
    &self.id
  }

  pub fn get_service<T: Any + Send + Sync>(&self, id: &ServiceId) -> Result<Arc<T>, ServiceError> {
    if !self.info.dependencies.contains(id) {
      return Err(ServiceError::Undeclared(id.clone()));
    }
    let resources = self.resources.lock().unwrap();
    if resources.phase == Phase::Idle {
      return Err(ServiceError::InvalidPhase);
    }
    self.services.get_service(id)
  }

  /// 仅在 start 内提交服务；成功完成 start 后统一对外发布。
  pub fn provide<T: Any + Send + Sync>(
    &self,
    id: ServiceId,
    service: T,
  ) -> Result<(), ServiceError> {
    if !self.info.provide.contains(&id) {
      return Err(ServiceError::Undeclared(id));
    }
    let mut resources = self.resources.lock().unwrap();
    if resources.phase != Phase::Starting {
      return Err(ServiceError::InvalidPhase);
    }
    if resources.pending.contains_key(&id) {
      return Err(ServiceError::Duplicate(id));
    }
    resources.pending.insert(id, Arc::new(service));
    Ok(())
  }

  pub fn stop_signal(&self) -> Result<StopSignal, ServiceError> {
    let resources = self.resources.lock().unwrap();
    if !matches!(resources.phase, Phase::Starting | Phase::Running) {
      return Err(ServiceError::InvalidPhase);
    }
    Ok(StopSignal(resources.stop.subscribe()))
  }

  /// 任务应监听 stop_signal 并协作退出。停止时等待任务，超时后 abort 并报告错误。
  pub fn spawn<F>(&self, future: F) -> Result<(), ServiceError>
  where
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
  {
    let mut resources = self.resources.lock().unwrap();
    if !matches!(resources.phase, Phase::Starting | Phase::Running) {
      return Err(ServiceError::InvalidPhase);
    }
    resources.tasks.push(tokio::spawn(future));
    Ok(())
  }

  pub(crate) fn begin(&self) {
    let mut resources = self.resources.lock().unwrap();
    resources.phase = Phase::Starting;
    resources.stop = watch::channel(false).0;
  }

  pub(crate) fn commit(&self) -> anyhow::Result<()> {
    let mut resources = self.resources.lock().unwrap();
    for id in &self.info.provide {
      if !resources.pending.contains_key(id) {
        bail!("插件 {:?} 未提交声明的服务 {id:?}", self.id);
      }
    }
    self.services.publish(&self.id, &mut resources.pending)?;
    resources.phase = Phase::Running;
    Ok(())
  }

  pub(crate) fn request_stop(&self) {
    let pending = {
      let mut resources = self.resources.lock().unwrap();
      resources.phase = Phase::Stopping;
      resources.stop.send_replace(true);
      std::mem::take(&mut resources.pending)
    };
    drop(pending);
  }

  pub(crate) async fn finish_tasks(&self) -> anyhow::Result<()> {
    let tasks = std::mem::take(&mut self.resources.lock().unwrap().tasks);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut errors = Vec::new();
    for mut task in tasks {
      match tokio::time::timeout_at(deadline, &mut task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => errors.push(format!("后台任务失败：{error:#}")),
        Ok(Err(error)) => errors.push(format!("后台任务异常：{error}")),
        Err(_) => {
          task.abort();
          let _ = task.await;
          errors.push("后台任务停止超时，已中止".to_string());
        }
      }
    }
    if !errors.is_empty() {
      bail!("{}", errors.join("；"));
    }
    Ok(())
  }

  pub(crate) fn mark_idle(&self) {
    self.resources.lock().unwrap().phase = Phase::Idle;
  }

  #[cfg(test)]
  pub(super) fn pending_for_test<T: Any + Send + Sync>(&self, id: &ServiceId) -> Option<Arc<T>> {
    self
      .resources
      .lock()
      .unwrap()
      .pending
      .get(id)
      .cloned()?
      .downcast()
      .ok()
  }
}
