use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use service::ServiceError;

fn pid(n: usize) -> PluginId {
  PluginId(format!("p{n}"))
}
fn sid(n: usize) -> ServiceId {
  ServiceId(format!("s{n}"))
}

#[derive(Default)]
struct Control {
  starts: AtomicUsize,
  stops: AtomicUsize,
  fail_stop: AtomicBool,
  release: tokio::sync::Notify,
  task_stopped: AtomicBool,
}

struct Fixture<const N: usize> {
  ctx: Arc<PluginContext>,
  control: Arc<Control>,
}

impl<const N: usize> PluginCreator for Fixture<N> {
  fn plugin_id() -> PluginId {
    pid(N)
  }
  fn plugin_info() -> PluginInfo {
    PluginInfo {
      provide: if N == 6 {
        vec![sid(0)]
      } else if N == 7 {
        vec![sid(7), sid(70)]
      } else {
        vec![sid(N)]
      },
      dependencies: match N {
        1 | 11 => vec![sid(0)],
        2 => vec![sid(1)],
        3 => vec![sid(99)],
        4 => vec![sid(5)],
        5 => vec![sid(4)],
        _ => vec![],
      },
    }
  }
  fn plugin(ctx: Arc<PluginContext>) -> Box<dyn Plugin> {
    Box::new(Self {
      ctx,
      control: Arc::default(),
    })
  }
}

impl<const N: usize> Plugin for Fixture<N> {
  fn start(&self) -> PluginFuture<'_, ()> {
    Box::pin(async move {
      self.control.starts.fetch_add(1, Ordering::SeqCst);
      if N == 1 {
        self.ctx.get_service::<Arc<Control>>(&sid(0))?;
      }
      self.ctx.provide(sid(N), Arc::clone(&self.control))?;
      if N == 8 {
        self.control.release.notified().await;
      }
      if N == 2 || N == 11 {
        bail!("模拟启动失败");
      }
      if N == 9 {
        let mut stop = self.ctx.stop_signal()?;
        let control = Arc::clone(&self.control);
        self.ctx.spawn(async move {
          stop.cancelled().await;
          control.task_stopped.store(true, Ordering::SeqCst);
          Ok(())
        })?;
      }
      if N == 10 {
        panic!("模拟启动 panic");
      }
      Ok(())
    })
  }
  fn stop(&self) -> PluginFuture<'_, ()> {
    Box::pin(async move {
      let stops = self.control.stops.fetch_add(1, Ordering::SeqCst);
      if N == 11 && stops == 0 {
        bail!("模拟首次清理失败");
      }
      if self.control.fail_stop.load(Ordering::SeqCst) {
        bail!("模拟停止失败");
      }
      Ok(())
    })
  }
}

fn control(registry: &PluginRegistry, n: usize) -> Arc<Control> {
  Arc::clone(
    &registry
      .services()
      .get_service::<Arc<Control>>(&sid(n))
      .unwrap(),
  )
}

#[tokio::test]
async fn starts_dependencies_once_and_protects_active_consumers() {
  let registry = PluginRegistry::new();
  registry.register::<Fixture<1>>().unwrap();
  registry.register::<Fixture<0>>().unwrap();
  registry.start(pid(1)).await.unwrap();
  let provider = control(&registry, 0);
  registry.start(pid(1)).await.unwrap();
  assert_eq!(provider.starts.load(Ordering::SeqCst), 1);
  assert!(registry.stop(pid(0)).await.is_err());
  assert!(registry.uninstall(pid(0)).await.is_err());
  assert!(registry.replace_plugin::<Fixture<0>>().await.is_err());
  registry.shutdown().await.unwrap();
  assert_eq!(provider.stops.load(Ordering::SeqCst), 1);
  assert!(matches!(
    registry.services().get_service::<Arc<Control>>(&sid(0)),
    Err(ServiceError::Unavailable(_))
  ));
  registry.stop(pid(0)).await.unwrap();
  assert_eq!(provider.stops.load(Ordering::SeqCst), 1);
  registry.start(pid(1)).await.unwrap();
  registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn validates_graph_before_any_plugin_starts() {
  let registry = PluginRegistry::new();
  registry.register::<Fixture<3>>().unwrap();
  assert!(format!("{:#}", registry.start(pid(3)).await.unwrap_err()).contains("s99"));
  assert_eq!(registry.state(&pid(3)), Some(PluginState::Registered));
  registry.register::<Fixture<4>>().unwrap();
  registry.register::<Fixture<5>>().unwrap();
  assert!(format!("{:#}", registry.start(pid(4)).await.unwrap_err()).contains("循环依赖"));
  assert_eq!(registry.state(&pid(5)), Some(PluginState::Registered));
  registry.register::<Fixture<0>>().unwrap();
  assert!(registry.register::<Fixture<0>>().is_err());
  assert!(registry.register::<Fixture<6>>().is_err());
  assert!(registry.state(&pid(6)).is_none());
}

#[tokio::test]
async fn rolls_back_new_dependencies_but_preserves_existing_running_plugins() {
  let registry = PluginRegistry::new();
  registry.register::<Fixture<0>>().unwrap();
  registry.register::<Fixture<1>>().unwrap();
  registry.register::<Fixture<2>>().unwrap();
  assert!(registry.start(pid(2)).await.is_err());
  assert_eq!(registry.state(&pid(0)), Some(PluginState::Registered));
  assert_eq!(registry.state(&pid(1)), Some(PluginState::Registered));
  assert!(matches!(
    registry.state(&pid(2)),
    Some(PluginState::Failed {
      cleanup_required: false,
      ..
    })
  ));
  assert!(
    registry
      .services()
      .get_service::<Arc<Control>>(&sid(2))
      .is_err()
  );
  registry.start(pid(0)).await.unwrap();
  assert!(registry.start(pid(2)).await.is_err());
  assert_eq!(registry.state(&pid(0)), Some(PluginState::Running));
  registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn rejects_partial_service_publication_and_cleans_up_panics() {
  let registry = PluginRegistry::new();
  registry.register::<Fixture<7>>().unwrap();
  assert!(registry.start(pid(7)).await.is_err());
  assert!(
    registry
      .services()
      .get_service::<Arc<Control>>(&sid(7))
      .is_err()
  );
  registry.register::<Fixture<10>>().unwrap();
  assert!(registry.start(pid(10)).await.is_err());
  assert!(matches!(
    registry.state(&pid(10)),
    Some(PluginState::Failed {
      phase: FailurePhase::Start,
      cleanup_required: false,
      ..
    })
  ));
  assert!(
    registry
      .services()
      .get_service::<Arc<Control>>(&sid(10))
      .is_err()
  );
}

#[tokio::test]
async fn failed_uninstall_retains_instance_and_dependencies_for_cleanup_retry() {
  let registry = PluginRegistry::new();
  registry.register::<Fixture<0>>().unwrap();
  registry.register::<Fixture<1>>().unwrap();
  registry.start(pid(1)).await.unwrap();
  let consumer = control(&registry, 1);
  consumer.fail_stop.store(true, Ordering::SeqCst);
  assert!(registry.uninstall(pid(1)).await.is_err());
  assert!(matches!(
    registry.state(&pid(1)),
    Some(PluginState::Failed {
      cleanup_required: true,
      ..
    })
  ));
  assert!(
    registry
      .services()
      .get_service::<Arc<Control>>(&sid(1))
      .is_err()
  );
  assert!(registry.stop(pid(0)).await.is_err());
  assert!(registry.start(pid(1)).await.is_err());
  assert!(registry.shutdown().await.is_err());
  assert_eq!(registry.state(&pid(0)), Some(PluginState::Running));
  consumer.fail_stop.store(false, Ordering::SeqCst);
  registry.uninstall(pid(1)).await.unwrap();
  assert!(registry.state(&pid(1)).is_none());
  assert_eq!(consumer.stops.load(Ordering::SeqCst), 3);
  registry.shutdown().await.unwrap();
}

struct BrokenReplacement;
impl PluginCreator for BrokenReplacement {
  fn plugin_id() -> PluginId {
    pid(0)
  }
  fn plugin_info() -> PluginInfo {
    PluginInfo::default()
  }
  fn plugin(_: Arc<PluginContext>) -> Box<dyn Plugin> {
    panic!("构造失败")
  }
}

#[tokio::test]
async fn failed_replacement_preserves_old_instance() {
  let registry = PluginRegistry::new();
  registry.register::<Fixture<0>>().unwrap();
  registry.start(pid(0)).await.unwrap();
  let old = control(&registry, 0);
  registry.stop(pid(0)).await.unwrap();
  assert!(
    registry
      .replace_plugin::<BrokenReplacement>()
      .await
      .is_err()
  );
  registry.start(pid(0)).await.unwrap();
  assert!(Arc::ptr_eq(&old, &control(&registry, 0)));
  registry.stop(pid(0)).await.unwrap();
  registry.replace_plugin::<Fixture<0>>().await.unwrap();
  registry.start(pid(0)).await.unwrap();
  assert!(!Arc::ptr_eq(&old, &control(&registry, 0)));
  registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelling_start_waiter_does_not_leave_partial_start() {
  let registry = PluginRegistry::new();
  registry.register::<Fixture<8>>().unwrap();
  let other = registry.clone();
  let waiter = tokio::spawn(async move { other.start(pid(8)).await });
  // 暂存服务只用于本测试同步，不通过对外注册表泄漏。
  let ctx = registry.handles(&pid(8)).unwrap().1;
  let pending = tokio::time::timeout(std::time::Duration::from_secs(2), async {
    loop {
      if let Some(control) = ctx.pending_for_test::<Arc<Control>>(&sid(8)) {
        break control;
      }
      tokio::task::yield_now().await;
    }
  })
  .await
  .unwrap();
  assert!(
    registry
      .services()
      .get_service::<Arc<Control>>(&sid(8))
      .is_err()
  );
  assert!(registry.register::<Fixture<0>>().is_err());
  waiter.abort();
  let _ = waiter.await;
  pending.release.notify_one();
  registry.stop(pid(8)).await.unwrap();
  assert_eq!(pending.starts.load(Ordering::SeqCst), 1);
  assert_eq!(pending.stops.load(Ordering::SeqCst), 1);
  assert_eq!(registry.state(&pid(8)), Some(PluginState::Registered));
}

#[tokio::test]
async fn stops_tracked_tasks_and_distinguishes_service_errors() {
  let registry = PluginRegistry::new();
  registry.register::<Fixture<9>>().unwrap();
  registry.start(pid(9)).await.unwrap();
  let task = control(&registry, 9);
  assert!(matches!(
    registry.services().get_service::<String>(&sid(9)),
    Err(ServiceError::TypeMismatch(_))
  ));
  let ctx = registry.handles(&pid(9)).unwrap().1;
  assert!(matches!(
    ctx.get_service::<String>(&sid(0)),
    Err(ServiceError::Undeclared(_))
  ));
  assert!(matches!(
    ctx.provide(sid(9), String::new()),
    Err(ServiceError::InvalidPhase)
  ));
  registry.stop(pid(9)).await.unwrap();
  assert!(task.task_stopped.load(Ordering::SeqCst));
  assert!(matches!(ctx.stop_signal(), Err(ServiceError::InvalidPhase)));
}

#[tokio::test]
async fn failed_start_cleanup_keeps_provider_until_retry_succeeds() {
  let registry = PluginRegistry::new();
  registry.register::<Fixture<0>>().unwrap();
  registry.register::<Fixture<11>>().unwrap();
  assert!(registry.start(pid(11)).await.is_err());
  assert!(matches!(
    registry.state(&pid(11)),
    Some(PluginState::Failed {
      cleanup_required: true,
      ..
    })
  ));
  assert_eq!(registry.state(&pid(0)), Some(PluginState::Running));
  registry.shutdown().await.unwrap();
  assert_eq!(registry.state(&pid(11)), Some(PluginState::Registered));
  assert_eq!(registry.state(&pid(0)), Some(PluginState::Registered));
}

#[test]
fn supports_trait_object_services_with_exact_type_matching() {
  trait Example: Send + Sync {
    fn value(&self) -> u32;
  }
  struct Implementation;
  impl Example for Implementation {
    fn value(&self) -> u32 {
      42
    }
  }
  let services = Arc::new(ServiceRegistry::default());
  let ctx = PluginContext::new(
    pid(0),
    PluginInfo {
      provide: vec![sid(0)],
      dependencies: vec![],
    },
    Arc::clone(&services),
  );
  ctx.begin();
  ctx
    .provide::<Box<dyn Example>>(sid(0), Box::new(Implementation))
    .unwrap();
  assert!(services.get_service::<Box<dyn Example>>(&sid(0)).is_err());
  assert!(matches!(
    ctx.provide(sid(0), 1u32),
    Err(ServiceError::Duplicate(_))
  ));
  ctx.commit().unwrap();
  assert_eq!(
    services
      .get_service::<Box<dyn Example>>(&sid(0))
      .unwrap()
      .value(),
    42
  );
  assert!(matches!(
    services.get_service::<Implementation>(&sid(0)),
    Err(ServiceError::TypeMismatch(_))
  ));
}
