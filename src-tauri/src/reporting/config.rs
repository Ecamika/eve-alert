use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Region {
  pub start: [u32; 2],
  pub end: [u32; 2],
  pub rgb: Vec<[u8; 3]>,
}

impl Region {
  // 与参考实现一致：两端均包含，任意一个像素精确命中任意颜色即成立。
  pub fn matches(&self, image: &RgbaImage) -> bool {
    if self.start[0] >= image.width() || self.start[1] >= image.height() {
      return false;
    }
    (self.start[1]..=self.end[1].min(image.height() - 1)).any(|y| {
      (self.start[0]..=self.end[0].min(image.width() - 1)).any(|x| {
        let p = image.get_pixel(x, y);
        self.rgb.contains(&[p[0], p[1], p[2]])
      })
    })
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
  pub title: String,
  pub warn_region: Region,
  pub reminder_now_region: Region,
  pub reminder_enemy_region: Region,
}

impl Character {
  pub fn fits(&self, image: &RgbaImage) -> bool {
    [
      &self.warn_region,
      &self.reminder_now_region,
      &self.reminder_enemy_region,
    ]
    .iter()
    .all(|r| r.end[0] < image.width() && r.end[1] < image.height())
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ReportMethod {
  Voice {
    warn_voice_path: String,
    reminder_voice_path: String,
  },
  Notification,
  Ntfy {
    server_url: String,
    topic: String,
    token: String,
  },
  Sse {
    host: [u8; 4],
    port: u16,
  },
  ReverseWebsocket {
    url: String,
    try_forever: bool,
    try_spacing: std::time::Duration,
  },
  Onebot {
    url: String,
    token: String,
    target: i64,
  },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
  pub characters: Vec<Character>,
  pub report_methods: Vec<ReportMethod>,
}

impl Config {
  pub fn validate(&self) -> Result<(), String> {
    let mut titles = std::collections::HashSet::new();
    for c in &self.characters {
      if c.title.trim().is_empty() || !titles.insert(&c.title) {
        return Err("角色名称不能为空或重复".into());
      }
      for r in [
        &c.warn_region,
        &c.reminder_now_region,
        &c.reminder_enemy_region,
      ] {
        if r.start[0] > r.end[0] || r.start[1] > r.end[1] || r.rgb.is_empty() {
          return Err(format!(
            "{}：区域起点不能大于终点，且至少需要一种颜色",
            c.title
          ));
        }
      }
    }
    let mut voice = false;
    let mut notification = false;
    let mut bindings = std::collections::HashSet::new();
    for method in &self.report_methods {
      match method {
        ReportMethod::Voice {
          warn_voice_path,
          reminder_voice_path,
        } => {
          if voice || warn_voice_path.trim().is_empty() || reminder_voice_path.trim().is_empty() {
            return Err("声音报告只能配置一次，且需要两种音频路径".into());
          }
          voice = true;
        }
        ReportMethod::Notification => {
          if notification {
            return Err("桌面通知只能配置一次".into());
          }
          notification = true;
        }
        ReportMethod::Ntfy {
          server_url,
          topic,
          token,
        } => {
          let url = reqwest::Url::parse(server_url).map_err(|_| "ntfy 服务器地址无效")?;
          if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
          {
            return Err("ntfy 服务器地址须为 HTTP(S) 地址，不包含账号、查询参数或片段".into());
          }
          if topic.is_empty()
            || !topic
              .bytes()
              .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
          {
            return Err("ntfy 主题只能包含英文字母、数字、下划线和连字符，且不能为空".into());
          }
          if reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).is_err() {
            return Err("ntfy 访问令牌包含无效字符".into());
          }
        }
        ReportMethod::Sse { host, port } => {
          if *port == 0 || !bindings.insert((*host, *port)) {
            return Err("SSE 端口不能为零或重复绑定".into());
          }
        }
        ReportMethod::ReverseWebsocket {
          url, try_spacing, ..
        } => {
          use tokio_tungstenite::tungstenite::client::IntoClientRequest;
          if !(url.starts_with("ws://") || url.starts_with("wss://"))
            || url.as_str().into_client_request().is_err()
            || try_spacing.is_zero()
          {
            return Err("WebSocket 地址须为 ws:// 或 wss://，重试间隔须大于零".into());
          }
        }
        ReportMethod::Onebot {
          url, target, token, ..
        } => {
          use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue};
          if *target <= 0
            || !(url.starts_with("ws://") || url.starts_with("wss://"))
            || url.as_str().into_client_request().is_err()
            || HeaderValue::from_str(&format!("Bearer {token}")).is_err()
          {
            return Err(
              "OneBot 需要有效的 WebSocket 地址、访问令牌和正整数私聊目标 ID（令牌可为空）".into(),
            );
          }
        }
      }
    }
    Ok(())
  }
}

/// 只报告告警颜色的出现和消失；无效画面不表示告警解除。
#[derive(Default)]
pub struct WarningState(bool);

impl WarningState {
  pub fn advance(&mut self, region: &Region, image: &RgbaImage) -> Option<bool> {
    if region.end[0] >= image.width() || region.end[1] >= image.height() {
      return None;
    }
    let matched = region.matches(image);
    if matched == self.0 {
      return None;
    }
    self.0 = matched;
    Some(matched)
  }
}

pub fn config_path() -> Result<PathBuf, String> {
  if cfg!(debug_assertions) {
    Ok(
      PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("无法确定项目根目录")?
        .join("reporting.json"),
    )
  } else {
    let home = std::env::var_os("HOME").ok_or("未设置 HOME，无法确定配置目录")?;
    Ok(PathBuf::from(home).join(".local/share/eve-alert/reporting.json"))
  }
}

#[derive(Default)]
pub struct ReminderState(bool, bool);

impl ReminderState {
  pub fn advance(&mut self, c: &Character, image: &RgbaImage) -> bool {
    if !self.0 && !self.1 && c.reminder_now_region.matches(image) {
      self.0 = true;
    } else if self.0 && !self.1 && c.reminder_enemy_region.matches(image) {
      self.1 = true;
    } else if self.0 && self.1 && !c.reminder_enemy_region.matches(image) {
      *self = Self::default();
      return true;
    }
    false
  }
}
