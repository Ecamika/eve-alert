use dashmap::DashMap;
use std::{any::Any, sync::Arc};

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub struct ServiceId(pub String);

pub type Service = Box<dyn Any + Send + Sync>;

pub struct ServiceRegistry {
  services: DashMap<ServiceId, Service>,
}

impl ServiceRegistry {
  pub fn register<T: Any + Send + Sync>(&self, service_id: ServiceId, service: T) {
    let arc_service = Arc::new(service);
    self.services.insert(service_id, Box::new(arc_service));
  }

  pub fn get_service<T: Any + Send + Sync>(&self, service_id: &ServiceId) -> Option<Arc<T>> {
    let any = self.services.get(service_id)?;
    let downcasted = any.downcast_ref::<Arc<T>>()?;
    let arc_service = Arc::clone(downcasted);
    Some(arc_service)
  }
}

#[cfg(test)]
mod tests {
  use dashmap::DashMap;

  #[test]
  fn dyn_type_check() {
    trait TestTrait: Send + Sync {
      fn test_function(&self) -> bool {
        true
      }
    }

    struct TestStruct {}

    impl TestTrait for TestStruct {}

    let service_registry = super::ServiceRegistry {
      services: DashMap::new(),
    };

    let test_struct = TestStruct {};

    let service_id = super::ServiceId("private:dyn_type_check".to_string());

    service_registry.register::<Box<dyn TestTrait> /* register 的泛型声明是必须的，且必须声明为 dyn 类型，否则会导致 downcast 无法匹配 TypeId */ >(service_id.clone(), Box::new(test_struct));

    let service = service_registry
      .get_service::<Box<dyn TestTrait>>(&service_id)
      .unwrap();

    assert!(service.test_function())
  }
}
