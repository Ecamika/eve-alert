//! 服务按插件成批发布和撤销；读取只克隆句柄，不持锁调用服务。

use std::{
  any::Any,
  collections::HashMap,
  sync::{Arc, RwLock},
};

use super::PluginId;

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct ServiceId(pub String);

pub type Service = Arc<dyn Any + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
  #[error("服务未就绪：{0:?}")]
  Unavailable(ServiceId),
  #[error("服务类型不匹配：{0:?}")]
  TypeMismatch(ServiceId),
  #[error("服务已存在：{0:?}")]
  Duplicate(ServiceId),
  #[error("插件未声明此服务：{0:?}")]
  Undeclared(ServiceId),
  #[error("当前插件阶段不允许此操作")]
  InvalidPhase,
}

struct ServiceEntry {
  owner: PluginId,
  value: Service,
}

#[derive(Default)]
pub struct ServiceRegistry {
  services: RwLock<HashMap<ServiceId, ServiceEntry>>,
}

impl ServiceRegistry {
  /// 注册和获取必须使用完全相同的 T，例如 Box<dyn Screencast>。
  /// 已取出的 Arc 不会被撤销；服务实现需要自行处理停止后的调用。
  pub fn get_service<T: Any + Send + Sync>(&self, id: &ServiceId) -> Result<Arc<T>, ServiceError> {
    let services = self.services.read().unwrap();
    let value = &services
      .get(id)
      .ok_or_else(|| ServiceError::Unavailable(id.clone()))?
      .value;
    Arc::clone(value)
      .downcast::<T>()
      .map_err(|_| ServiceError::TypeMismatch(id.clone()))
  }

  pub(crate) fn publish(
    &self,
    owner: &PluginId,
    pending: &mut HashMap<ServiceId, Service>,
  ) -> Result<(), ServiceError> {
    let mut services = self.services.write().unwrap();
    for id in pending.keys() {
      if services.contains_key(id) {
        return Err(ServiceError::Duplicate(id.clone()));
      }
    }
    for (id, value) in pending.drain() {
      services.insert(
        id,
        ServiceEntry {
          owner: owner.clone(),
          value,
        },
      );
    }
    Ok(())
  }

  pub(crate) fn revoke(&self, owner: &PluginId) {
    // 用户服务的 Drop 在锁外运行。
    let removed = {
      let mut services = self.services.write().unwrap();
      let ids: Vec<_> = services
        .iter()
        .filter(|(_, entry)| &entry.owner == owner)
        .map(|(id, _)| id.clone())
        .collect();
      ids
        .into_iter()
        .filter_map(|id| services.remove(&id))
        .collect::<Vec<_>>()
    };
    drop(removed);
  }
}
