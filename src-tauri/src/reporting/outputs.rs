use super::{
  ReportEvent,
  config::{Config, ReportMethod},
};
use axum::{
  Router,
  extract::State,
  http::HeaderValue,
  response::{
    Sse,
    sse::{Event, KeepAlive},
  },
  routing::get,
};
use futures_util::{SinkExt, StreamExt, stream};
use std::{convert::Infallible, time::Duration};
use tauri::{AppHandle, Emitter};
use tokio::{
  net::TcpListener,
  sync::{broadcast, watch},
  task::JoinHandle,
};
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone)]
struct SseState {
  sender: broadcast::Sender<ReportEvent>,
  titles: Vec<String>,
  shutdown: watch::Receiver<bool>,
}

async fn events(
  State(state): State<SseState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
  let receiver = state.sender.subscribe();
  let initial = stream::once(async move {
    Ok(Event::default().data(serde_json::to_string(&state.titles).unwrap()))
  });
  let updates = stream::unfold(
    (receiver, state.shutdown),
    |(mut receiver, mut shutdown)| async move {
      loop {
        if *shutdown.borrow() {
          return None;
        }
        let event = tokio::select! {
          _ = shutdown.changed() => return None,
          event = receiver.recv() => event,
        };
        match event {
          Ok(event) => {
            return Some((
              Ok(Event::default().data(serde_json::to_string(&event).unwrap())),
              (receiver, shutdown),
            ));
          }
          Err(broadcast::error::RecvError::Lagged(_)) => continue,
          Err(broadcast::error::RecvError::Closed) => return None,
        }
      }
    },
  );
  Sse::new(initial.chain(updates)).keep_alive(KeepAlive::new().interval(Duration::from_secs(5)))
}

pub struct Outputs(Vec<JoinHandle<()>>, watch::Sender<bool>);
impl Outputs {
  pub async fn shutdown(mut self) {
    let _ = self.1.send(true);
    for task in &self.0 {
      task.abort();
    }
    for task in self.0.drain(..) {
      let _ = task.await;
    }
  }
}
impl Drop for Outputs {
  fn drop(&mut self) {
    let _ = self.1.send(true);
    for task in &self.0 {
      task.abort();
    }
  }
}

pub fn report_error(app: &AppHandle, message: impl Into<String>) {
  let message = message.into();
  log::warn!("{message}");
  let _ = app.emit("reporting-error", message);
}

pub async fn start(
  config: &Config,
  sender: &broadcast::Sender<ReportEvent>,
  app: &AppHandle,
) -> Result<Outputs, String> {
  let mut tasks = Outputs(Vec::new(), watch::channel(false).0);
  for method in &config.report_methods {
    match method {
      ReportMethod::Sse { host, port } => {
        let listener = TcpListener::bind((std::net::Ipv4Addr::from(*host), *port))
          .await
          .map_err(|e| format!("SSE 绑定失败：{e}"))?;
        let state = SseState {
          sender: sender.clone(),
          titles: config.characters.iter().map(|c| c.title.clone()).collect(),
          shutdown: tasks.1.subscribe(),
        };
        let router = Router::new()
          .route("/events", get(events))
          .with_state(state)
          .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
              response
                .headers_mut()
                .insert("access-control-allow-origin", HeaderValue::from_static("*"));
              response
            },
          ));
        let app = app.clone();
        tasks.0.push(tokio::spawn(async move {
          if let Err(e) = axum::serve(listener, router).await {
            report_error(&app, format!("SSE 服务失败：{e}"));
          }
        }));
      }
      ReportMethod::ReverseWebsocket {
        url,
        try_forever,
        try_spacing,
      } => {
        let (url, retry, spacing) = (url.clone(), *try_forever, *try_spacing);
        let sender = sender.clone();
        let app = app.clone();
        tasks.0.push(tokio::spawn(async move {
          loop {
            let mut receiver = sender.subscribe();
            let result: anyhow::Result<()> = async {
              let (mut socket, _) = tokio_tungstenite::connect_async(&url).await?;
              loop {
                tokio::select! {
                  event = receiver.recv() => match event {
                    Ok(event) => socket.send(Message::Text(serde_json::to_string(&event)?.into())).await?,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return Ok(()),
                  },
                  message = socket.next() => match message {
                    Some(Ok(Message::Close(_))) | None => anyhow::bail!("连接已关闭"),
                    Some(Err(e)) => return Err(e.into()),
                    Some(Ok(Message::Ping(data))) => socket.send(Message::Pong(data)).await?,
                    _ => {},
                  }
                }
              }
            }.await;
            if let Err(e) = result { report_error(&app, format!("反向 WebSocket：{e}")); }
            if !retry { break; }
            tokio::time::sleep(spacing).await;
          }
        }));
      }
      ReportMethod::Notification => {
        let mut receiver = sender.subscribe();
        let app = app.clone();
        tasks.0.push(tokio::spawn(async move {
          loop {
            match receiver.recv().await {
              Ok(event) => {
                if let Err(e) = notify(&event).await {
                  report_error(&app, format!("桌面通知失败：{e}"));
                }
              }
              Err(broadcast::error::RecvError::Lagged(_)) => continue,
              Err(_) => break,
            }
          }
        }));
      }
      ReportMethod::Ntfy {
        server_url,
        topic,
        token,
      } => {
        let client = ntfy_client_builder()
          .build()
          .map_err(|e| format!("ntfy 客户端初始化失败：{e}"))?;
        let (server_url, topic, token) = (server_url.clone(), topic.clone(), token.clone());
        let mut receiver = sender.subscribe();
        let app = app.clone();
        tasks.0.push(tokio::spawn(async move {
          loop {
            match receiver.recv().await {
              Ok(event) => {
                if let Err(e) = publish_ntfy(&client, &server_url, &topic, &token, &event).await {
                  report_error(&app, format!("ntfy 通知失败：{e}"));
                }
              }
              Err(broadcast::error::RecvError::Lagged(count)) => {
                report_error(&app, format!("ntfy 通知积压，丢失 {count} 条事件"))
              }
              Err(_) => break,
            }
          }
        }));
      }
      // 音频由 Tauri WebView 的 Audio 播放，避免新增系统音频库依赖。
      ReportMethod::Voice {
        warn_voice_path,
        reminder_voice_path,
      } => {
        for path in [warn_voice_path, reminder_voice_path] {
          let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("无法读取音频 {path}：{e}"))?;
          if !metadata.is_file() || metadata.len() > 20 * 1024 * 1024 {
            return Err("音频必须是小于 20 MiB 的文件".into());
          }
        }
      }
      ReportMethod::Onebot { url, token, target } => {
        let options = OnebotOptions {
          url: url.clone(),
          token: token.clone(),
          target: *target,
        };
        tasks
          .0
          .push(tokio::spawn(onebot(options, sender.clone(), app.clone())));
      }
    }
  }
  Ok(tasks)
}

struct OnebotOptions {
  url: String,
  token: String,
  target: i64,
}

async fn onebot(options: OnebotOptions, sender: broadcast::Sender<ReportEvent>, app: AppHandle) {
  use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue};
  loop {
    let mut receiver = sender.subscribe();
    let result: anyhow::Result<()> = async {
      let mut request = options.url.as_str().into_client_request()?;
      if !options.token.is_empty() {
        request.headers_mut().insert("Authorization", HeaderValue::from_str(&format!("Bearer {}", options.token))?);
      }
      let (mut socket, _) = tokio_tungstenite::connect_async(request).await?;
      loop {
        tokio::select! {
          event = receiver.recv() => match event {
            Ok(event) => {
              let message = serde_json::json!({
                "action": "send_private_msg",
                "params": { "user_id": options.target, "message": [{ "type": "text", "data": { "text": format!("{} {}", event.kind(), event.title()) } }] },
                "echo": "eve-alert"
              });
              socket.send(Message::Text(message.to_string().into())).await?;
            },
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return Ok(()),
          },
          message = socket.next() => match message {
            Some(Ok(Message::Text(text))) => {
              if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
                && value["echo"] == "eve-alert" && value["status"] == "failed" {
                  report_error(&app, format!("OneBot 私聊失败：{}", value["retcode"]));
                }
            },
            Some(Ok(Message::Ping(data))) => socket.send(Message::Pong(data)).await?,
            Some(Ok(Message::Close(_))) | None => anyhow::bail!("连接已关闭"),
            Some(Err(e)) => return Err(e.into()),
            _ => {},
          }
        }
      }
    }.await;
    if let Err(e) = result {
      report_error(&app, format!("OneBot：{e}"));
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
  }
}

fn ntfy_client_builder() -> reqwest::ClientBuilder {
  // rustls-no-provider 要求在构建 reqwest 客户端前显式注册加密提供者。
  // 若其他 TLS 使用者已注册默认提供者，保留已有配置；重复调用也是安全的。
  let _ = rustls::crypto::ring::default_provider().install_default();
  reqwest::Client::builder()
    .timeout(Duration::from_secs(10))
    .redirect(reqwest::redirect::Policy::none())
}

async fn publish_ntfy(
  client: &reqwest::Client,
  server_url: &str,
  topic: &str,
  token: &str,
  event: &ReportEvent,
) -> Result<(), reqwest::Error> {
  let body = serde_json::json!({
    "topic": topic,
    "title": format!("EVE Alert · {}", event.label()),
    "message": format!("{}：{}", event.title(), match event {
      ReportEvent::Warn { .. } => "检测到告警颜色",
      ReportEvent::WarnCleared { .. } => "告警颜色已消失",
      ReportEvent::Reminder { .. } => "提醒条件已满足",
    }),
    "priority": if matches!(event, ReportEvent::Warn { .. }) { 4 } else { 3 },
    "tags": [if matches!(event, ReportEvent::Warn { .. }) { "warning" } else { "white_check_mark" }],
  });
  let mut request = client
    .post(format!("{}/", server_url.trim_end_matches('/')))
    .json(&body);
  if !token.is_empty() {
    request = request.bearer_auth(token);
  }
  request.send().await?.error_for_status()?;
  Ok(())
}

#[cfg(target_os = "linux")]
async fn notify(event: &ReportEvent) -> anyhow::Result<()> {
  let connection = zbus::Connection::session().await?;
  let proxy = zbus::Proxy::new(
    &connection,
    "org.freedesktop.Notifications",
    "/org/freedesktop/Notifications",
    "org.freedesktop.Notifications",
  )
  .await?;
  let hints = std::collections::HashMap::<&str, zbus::zvariant::Value<'_>>::new();
  let _: u32 = proxy
    .call(
      "Notify",
      &(
        "EVE Alert",
        0u32,
        "",
        event.label(),
        event.title(),
        Vec::<String>::new(),
        hints,
        5000i32,
      ),
    )
    .await?;
  Ok(())
}
#[cfg(not(target_os = "linux"))]
async fn notify(_: &ReportEvent) -> anyhow::Result<()> {
  anyhow::bail!("当前平台尚未实现桌面通知")
}

#[cfg(test)]
mod tests {
  use super::*;
  use axum::response::IntoResponse;

  #[tokio::test]
  async fn ntfy_posts_unicode_json_with_authentication_and_reports_http_errors() {
    use axum::{
      Json,
      http::{HeaderMap, StatusCode},
      routing::post,
    };
    use tokio::sync::mpsc;
    type Received = (HeaderMap, serde_json::Value);
    async fn receive(
      State(tx): State<mpsc::Sender<Received>>,
      headers: HeaderMap,
      Json(body): Json<serde_json::Value>,
    ) -> StatusCode {
      let status = if body["topic"] == "denied" {
        StatusCode::FORBIDDEN
      } else {
        StatusCode::OK
      };
      tx.send((headers, body)).await.unwrap();
      status
    }
    let (tx, mut rx) = mpsc::channel(8);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/ntfy", listener.local_addr().unwrap());
    let router = Router::new().route("/ntfy/", post(receive)).with_state(tx);
    let server = tokio::spawn(async move {
      axum::serve(listener, router).await.unwrap();
    });
    let client = ntfy_client_builder()
      .no_proxy()
      .timeout(Duration::from_secs(2))
      .build()
      .unwrap();
    for (event, label, priority) in [
      (
        ReportEvent::Warn {
          title: "EVE - 测试".into(),
        },
        "告警",
        4,
      ),
      (
        ReportEvent::WarnCleared {
          title: "EVE - 测试".into(),
        },
        "告警解除",
        3,
      ),
      (
        ReportEvent::Reminder {
          title: "EVE - 测试".into(),
        },
        "提醒",
        3,
      ),
    ] {
      publish_ntfy(&client, &url, "eve_test", "tk_local", &event)
        .await
        .unwrap();
      let (headers, body) = rx.recv().await.unwrap();
      assert_eq!(headers["authorization"], "Bearer tk_local");
      assert_eq!(body["topic"], "eve_test");
      assert_eq!(body["title"], format!("EVE Alert · {label}"));
      assert!(body["message"].as_str().unwrap().contains("EVE - 测试"));
      assert_eq!(body["priority"], priority);
    }
    let error = publish_ntfy(
      &client,
      &url,
      "denied",
      "",
      &ReportEvent::Warn {
        title: "test".into(),
      },
    )
    .await
    .unwrap_err();
    assert_eq!(error.status(), Some(StatusCode::FORBIDDEN));
    assert!(!rx.recv().await.unwrap().0.contains_key("authorization"));
    server.abort();
    let _ = server.await;
  }

  #[tokio::test]
  async fn sse_sends_titles_first_and_continues_after_receiver_lags() {
    let (sender, _) = broadcast::channel(1);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let response = events(State(SseState {
      sender: sender.clone(),
      titles: vec!["EVE - test".into()],
      shutdown: shutdown_rx,
    }))
    .await
    .into_response();
    let mut stream = response.into_body().into_data_stream();
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(
      std::str::from_utf8(&first).unwrap(),
      "data: [\"EVE - test\"]\n\n"
    );
    let _ = sender.send(ReportEvent::Warn {
      title: "stale".into(),
    });
    let _ = sender.send(ReportEvent::Reminder {
      title: "EVE - test".into(),
    });
    let next = tokio::time::timeout(Duration::from_secs(1), stream.next())
      .await
      .unwrap()
      .unwrap()
      .unwrap();
    assert_eq!(
      std::str::from_utf8(&next).unwrap(),
      "data: {\"type\":\"Reminder\",\"title\":\"EVE - test\"}\n\n"
    );
    shutdown.send(true).unwrap();
    assert!(
      tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .unwrap()
        .is_none()
    );
  }

  #[test]
  fn secure_websocket_tls_configuration_has_a_crypto_provider() {
    let _ = rustls::ClientConfig::builder()
      .with_root_certificates(rustls::RootCertStore::empty())
      .with_no_client_auth();
  }
}
