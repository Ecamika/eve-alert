use std::sync::Arc;

use dashmap::DashMap;
use futures_util::future::BoxFuture;

use crate::plugin::service::ServiceId;

pub mod service;

pub type PluginFuture<'a, T> = BoxFuture<'a, anyhow::Result<T>>;

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct PluginId(pub String);

pub struct PluginInfo {
  pub provide: Vec<ServiceId>,
  pub dependencies: Vec<ServiceId>,
}

pub trait Plugin {}

pub trait PluginCreator {
  fn plugin_info() -> PluginInfo;
  fn plugin(ctx: Arc<PluginContext>) -> Box<dyn Plugin>;
  fn plugin_id() -> PluginId;
}

pub struct PluginContext {}

pub struct PluginRegistry {
  pub plugin_map: DashMap<PluginId, (PluginInfo, Box<dyn Plugin>)>,
}

impl PluginRegistry {
  pub fn register<T: PluginCreator>(&self) -> anyhow::Result<()> {
    let plugin_id = T::plugin_id();
    if self.plugin_map.contains_key(&plugin_id) {
      return Err(anyhow::anyhow!("插件ID已注册"));
    }

    let context = PluginContext {};

    let plugin_info = T::plugin_info();
    let plugin = T::plugin(Arc::new(context));

    self.plugin_map.insert(plugin_id, (plugin_info, plugin));

    Ok(())
  }

  pub fn uninstall(&self, id: PluginId) -> anyhow::Result<()> {
    if !self.plugin_map.contains_key(&id) {
      return Err(anyhow::anyhow!("插件ID未找到"));
    }

    self.plugin_map.remove(&id);

    Ok(())
  }

  pub fn replace_plugin<T: PluginCreator>(&self) -> anyhow::Result<()> {
    let plugin_id = T::plugin_id();
    self.uninstall(plugin_id)?;
    self.register::<T>()
  }
}
