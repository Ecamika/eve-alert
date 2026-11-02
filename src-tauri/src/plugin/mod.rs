//! 内置插件运行时。生命周期串行协调，插件回调期间不持有注册表锁。

use std::{
  collections::{HashMap, HashSet},
  panic::AssertUnwindSafe,
  sync::{Arc, Mutex},
};

use anyhow::{Context, bail};
use futures_util::{FutureExt, future::BoxFuture};

mod context;
pub mod service;
pub use context::{PluginContext, StopSignal};
use service::{ServiceId, ServiceRegistry};

pub type PluginFuture<'a, T> = BoxFuture<'a, anyhow::Result<T>>;

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct PluginId(pub String);

#[derive(Debug, Clone, Default)]
pub struct PluginInfo {
  pub provide: Vec<ServiceId>,
  pub dependencies: Vec<ServiceId>,
}

/// 构造函数保持无副作用；资源在 start 中创建，在 stop 中释放。
/// stop 必须支持部分启动失败后的清理和失败重试。
/// 回调不能等待本管理器的其他生命周期操作，以免串行队列相互等待。
pub trait Plugin: Send + Sync {
  fn start(&self) -> PluginFuture<'_, ()>;
  fn stop(&self) -> PluginFuture<'_, ()>;
}

pub trait PluginCreator {
  fn plugin_info() -> PluginInfo;
  fn plugin(ctx: Arc<PluginContext>) -> Box<dyn Plugin>;
  fn plugin_id() -> PluginId;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePhase {
  Start,
  Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginState {
  Registered,
  Starting,
  Running,
  Stopping,
  Failed {
    phase: FailurePhase,
    error: String,
    cleanup_required: bool,
  },
}

impl PluginState {
  fn needs_cleanup(&self) -> bool {
    matches!(
      self,
      Self::Starting
        | Self::Running
        | Self::Stopping
        | Self::Failed {
          cleanup_required: true,
          ..
        }
    )
  }
}

struct PluginEntry {
  info: PluginInfo,
  plugin: Arc<dyn Plugin>,
  context: Arc<PluginContext>,
  state: PluginState,
}

#[derive(Default)]
struct RegistryInner {
  plugins: Mutex<HashMap<PluginId, PluginEntry>>,
  services: Arc<ServiceRegistry>,
  lifecycle: tokio::sync::Mutex<()>,
}

#[derive(Clone, Default)]
pub struct PluginRegistry {
  inner: Arc<RegistryInner>,
}

impl PluginRegistry {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn services(&self) -> Arc<ServiceRegistry> {
    Arc::clone(&self.inner.services)
  }

  pub fn state(&self, id: &PluginId) -> Option<PluginState> {
    self
      .inner
      .plugins
      .lock()
      .unwrap()
      .get(id)
      .map(|entry| entry.state.clone())
  }

  /// 同步注册不等待正在执行的生命周期操作，忙时返回错误，允许稍后重试。
  pub fn register<T: PluginCreator>(&self) -> anyhow::Result<()> {
    let _operation = self
      .inner
      .lifecycle
      .try_lock()
      .context("插件生命周期操作进行中")?;
    let id = T::plugin_id();
    let info = T::plugin_info();
    self.validate_registration(&id, &info, false)?;
    let entry = self.create::<T>(&id, info)?;
    self.inner.plugins.lock().unwrap().insert(id, entry);
    Ok(())
  }

  fn validate_registration(
    &self,
    id: &PluginId,
    info: &PluginInfo,
    replacing: bool,
  ) -> anyhow::Result<()> {
    let plugins = self.inner.plugins.lock().unwrap();
    if !replacing && plugins.contains_key(id) {
      bail!("插件 ID 已注册：{id:?}");
    }
    let mut provided = HashSet::new();
    for service in &info.provide {
      if !provided.insert(service) {
        bail!("插件 {id:?} 重复声明服务：{service:?}");
      }
      for (owner, entry) in plugins.iter() {
        if (!replacing || owner != id) && entry.info.provide.contains(service) {
          bail!("服务 {service:?} 已由插件 {owner:?} 提供");
        }
      }
    }
    let mut dependencies = HashSet::new();
    for service in &info.dependencies {
      if !dependencies.insert(service) {
        bail!("插件 {id:?} 重复声明依赖：{service:?}");
      }
    }
    Ok(())
  }

  fn create<T: PluginCreator>(
    &self,
    id: &PluginId,
    info: PluginInfo,
  ) -> anyhow::Result<PluginEntry> {
    let context = Arc::new(PluginContext::new(
      id.clone(),
      info.clone(),
      self.services(),
    ));
    let plugin = std::panic::catch_unwind(AssertUnwindSafe(|| T::plugin(Arc::clone(&context))))
      .map_err(|_| anyhow::anyhow!("插件 {id:?} 构造时 panic"))?;
    Ok(PluginEntry {
      info,
      plugin: Arc::from(plugin),
      context,
      state: PluginState::Registered,
    })
  }

  /// 操作交给独立任务；取消等待不会取消已经提交的生命周期操作。
  pub async fn start(&self, id: PluginId) -> anyhow::Result<()> {
    let registry = self.clone();
    tokio::spawn(async move {
      let _operation = registry.inner.lifecycle.lock().await;
      let plan = registry.start_plan(&id)?;
      let mut started = Vec::new();
      for next in plan {
        if registry.state(&next) == Some(PluginState::Running) {
          continue;
        }
        if let Err(error) = registry.start_one(&next).await {
          let mut errors = vec![format!("启动 {id:?} 失败：{error:#}")];
          for previous in started.iter().rev() {
            if let Err(error) = registry.stop_one(previous).await {
              errors.push(format!("回滚 {previous:?} 失败：{error:#}"));
            }
          }
          bail!("{}", errors.join("；"));
        }
        started.push(next);
      }
      Ok(())
    })
    .await
    .context("插件启动管理任务异常")?
  }

  fn start_plan(&self, id: &PluginId) -> anyhow::Result<Vec<PluginId>> {
    fn visit(
      id: &PluginId,
      plugins: &HashMap<PluginId, PluginEntry>,
      path: &mut Vec<PluginId>,
      visited: &mut HashSet<PluginId>,
      plan: &mut Vec<PluginId>,
    ) -> anyhow::Result<()> {
      if visited.contains(id) {
        return Ok(());
      }
      if path.contains(id) {
        bail!("插件循环依赖：{path:?} -> {id:?}");
      }
      let entry = plugins
        .get(id)
        .with_context(|| format!("插件 ID 未找到：{id:?}"))?;
      if matches!(
        entry.state,
        PluginState::Failed {
          cleanup_required: true,
          ..
        }
      ) {
        bail!("插件 {id:?} 存在待清理资源，请先重试停止");
      }
      path.push(id.clone());
      for service in &entry.info.dependencies {
        let (provider, _) = plugins
          .iter()
          .find(|(_, entry)| entry.info.provide.contains(service))
          .with_context(|| format!("插件依赖链 {path:?} 缺少服务 {service:?} 的提供者"))?;
        visit(provider, plugins, path, visited, plan)?;
      }
      path.pop();
      visited.insert(id.clone());
      plan.push(id.clone());
      Ok(())
    }
    let plugins = self.inner.plugins.lock().unwrap();
    let mut plan = Vec::new();
    visit(
      id,
      &plugins,
      &mut Vec::new(),
      &mut HashSet::new(),
      &mut plan,
    )?;
    Ok(plan)
  }

  fn handles(&self, id: &PluginId) -> anyhow::Result<(Arc<dyn Plugin>, Arc<PluginContext>)> {
    let plugins = self.inner.plugins.lock().unwrap();
    let entry = plugins
      .get(id)
      .with_context(|| format!("插件 ID 未找到：{id:?}"))?;
    Ok((Arc::clone(&entry.plugin), Arc::clone(&entry.context)))
  }

  fn set_state(&self, id: &PluginId, state: PluginState) {
    self
      .inner
      .plugins
      .lock()
      .unwrap()
      .get_mut(id)
      .unwrap()
      .state = state;
  }

  async fn start_one(&self, id: &PluginId) -> anyhow::Result<()> {
    let (plugin, context) = self.handles(id)?;
    self.set_state(id, PluginState::Starting);
    context.begin();
    let result = AssertUnwindSafe(async { plugin.start().await })
      .catch_unwind()
      .await
      .unwrap_or_else(|_| Err(anyhow::anyhow!("插件 start panic")))
      .and_then(|()| context.commit());
    if let Err(error) = result {
      let cleanup = self.stop_one(id).await;
      let cleanup_required = cleanup.is_err();
      let message = match cleanup {
        Ok(()) => format!("{error:#}"),
        Err(cleanup) => format!("{error:#}；启动失败后清理失败：{cleanup:#}"),
      };
      self.set_state(
        id,
        PluginState::Failed {
          phase: FailurePhase::Start,
          error: message.clone(),
          cleanup_required,
        },
      );
      bail!("插件 {id:?}：{message}");
    }
    self.set_state(id, PluginState::Running);
    Ok(())
  }

  fn ensure_no_consumers(&self, id: &PluginId) -> anyhow::Result<()> {
    let plugins = self.inner.plugins.lock().unwrap();
    let entry = plugins
      .get(id)
      .with_context(|| format!("插件 ID 未找到：{id:?}"))?;
    for (consumer, other) in plugins.iter() {
      if consumer != id
        && other.state.needs_cleanup()
        && other
          .info
          .dependencies
          .iter()
          .any(|service| entry.info.provide.contains(service))
      {
        bail!("插件 {id:?} 仍被活动插件 {consumer:?} 依赖");
      }
    }
    Ok(())
  }

  async fn stop_one(&self, id: &PluginId) -> anyhow::Result<()> {
    self.ensure_no_consumers(id)?;
    let (plugin, context) = self.handles(id)?;
    if !self.state(id).unwrap().needs_cleanup() {
      return Ok(());
    }
    self.set_state(id, PluginState::Stopping);
    context.request_stop();
    self.inner.services.revoke(id);
    let hook = AssertUnwindSafe(async { plugin.stop().await })
      .catch_unwind()
      .await
      .unwrap_or_else(|_| Err(anyhow::anyhow!("插件 stop panic")));
    let tasks = context.finish_tasks().await;
    if hook.is_err() || tasks.is_err() {
      let message = [hook.err(), tasks.err()]
        .into_iter()
        .flatten()
        .map(|error| format!("{error:#}"))
        .collect::<Vec<_>>()
        .join("；");
      self.set_state(
        id,
        PluginState::Failed {
          phase: FailurePhase::Stop,
          error: message.clone(),
          cleanup_required: true,
        },
      );
      bail!("插件 {id:?} 停止失败：{message}");
    }
    context.mark_idle();
    self.set_state(id, PluginState::Registered);
    Ok(())
  }

  pub async fn stop(&self, id: PluginId) -> anyhow::Result<()> {
    let registry = self.clone();
    tokio::spawn(async move {
      let _operation = registry.inner.lifecycle.lock().await;
      registry.stop_one(&id).await
    })
    .await
    .context("插件停止管理任务异常")?
  }

  pub async fn uninstall(&self, id: PluginId) -> anyhow::Result<()> {
    let registry = self.clone();
    tokio::spawn(async move {
      let _operation = registry.inner.lifecycle.lock().await;
      registry.stop_one(&id).await?;
      let removed = registry.inner.plugins.lock().unwrap().remove(&id);
      drop(removed);
      Ok(())
    })
    .await
    .context("插件卸载管理任务异常")?
  }

  /// 应用退出前显式等待，按消费者优先的顺序停止。清理失败保留实例供重试。
  pub async fn shutdown(&self) -> anyhow::Result<()> {
    let registry = self.clone();
    tokio::spawn(async move {
      let _operation = registry.inner.lifecycle.lock().await;
      let mut remaining: HashSet<_> = registry
        .inner
        .plugins
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, entry)| entry.state.needs_cleanup())
        .map(|(id, _)| id.clone())
        .collect();
      let mut errors = Vec::new();
      while !remaining.is_empty() {
        // 失败的消费者会继续阻止提供者停止，不破坏清理时的依赖。
        let candidate = remaining
          .iter()
          .find(|id| registry.ensure_no_consumers(id).is_ok())
          .cloned();
        let Some(id) = candidate else {
          errors.push(format!("以下插件仍被未清理的消费者依赖：{remaining:?}"));
          break;
        };
        remaining.remove(&id);
        if let Err(error) = registry.stop_one(&id).await {
          errors.push(format!("{error:#}"));
        }
      }
      if !errors.is_empty() {
        bail!("{}", errors.join("；"));
      }
      Ok(())
    })
    .await
    .context("插件关闭管理任务异常")?
  }

  /// 只替换已停止且清理完成的实例。构造或声明校验失败时保留旧实例。
  pub async fn replace_plugin<T: PluginCreator>(&self) -> anyhow::Result<()> {
    let registry = self.clone();
    tokio::spawn(async move {
      let _operation = registry.inner.lifecycle.lock().await;
      let id = T::plugin_id();
      registry.ensure_no_consumers(&id)?;
      if registry.state(&id).unwrap().needs_cleanup() {
        bail!("替换前请先停止插件 {id:?}");
      }
      let info = T::plugin_info();
      registry.validate_registration(&id, &info, true)?;
      let entry = registry.create::<T>(&id, info)?;
      let old = registry.inner.plugins.lock().unwrap().insert(id, entry);
      drop(old);
      Ok(())
    })
    .await
    .context("插件替换管理任务异常")?
  }
}

#[cfg(test)]
mod tests;
