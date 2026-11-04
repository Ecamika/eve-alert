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
#[serde(from = "CharacterConfig")]
pub struct Character {
  pub title: String,
  pub enabled: bool,
  pub warn_region: Region,
  pub reminder_now_region: Region,
  pub reminder_gate_region: Region,
  pub reminder_enemy_region: Region,
}

#[derive(Deserialize)]
struct CharacterConfig {
  title: String,
  #[serde(default = "default_enabled")]
  enabled: bool,
  warn_region: Region,
  reminder_now_region: Region,
  #[serde(default)]
  reminder_gate_region: Option<Region>,
  reminder_enemy_region: Region,
}

impl From<CharacterConfig> for Character {
  fn from(config: CharacterConfig) -> Self {
    Self {
      title: config.title,
      enabled: config.enabled,
      warn_region: config.warn_region,
      reminder_now_region: config.reminder_now_region,
      // 旧配置保留原区域，并复制为前置区域的初始值，保持原有检测行为。
      reminder_gate_region: config
        .reminder_gate_region
        .unwrap_or_else(|| config.reminder_enemy_region.clone()),
      reminder_enemy_region: config.reminder_enemy_region,
    }
  }
}

fn default_enabled() -> bool {
  true
}

impl Character {
  pub fn fits(&self, image: &RgbaImage) -> bool {
    [
      &self.warn_region,
      &self.reminder_now_region,
      &self.reminder_gate_region,
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
  pub fn into_monitoring(mut self) -> Result<Self, String> {
    self.characters.retain(|c| c.enabled);
    if self.characters.is_empty() {
      return Err("请至少启用一个角色".into());
    }
    Ok(self)
  }

  pub fn validate(&self) -> Result<(), String> {
    let mut titles = std::collections::HashSet::new();
    for c in &self.characters {
      if c.title.trim().is_empty() || !titles.insert(&c.title) {
        return Err("角色名称不能为空或重复".into());
      }
      for r in [
        &c.warn_region,
        &c.reminder_now_region,
        &c.reminder_gate_region,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ReminderStage {
  LastWave,
  Ended,
}

impl ReminderStage {
  pub fn message(self) -> &'static str {
    match self {
      Self::LastWave => "异常进入最后一波次",
      Self::Ended => "异常结束",
    }
  }
}

#[derive(Default)]
pub enum ReminderState {
  #[default]
  Idle,
  WaitingRgb2,
  WaitingRgb3,
  Rgb3Seen,
  WaitingRgb1Clear,
}

impl ReminderState {
  pub fn advance(&mut self, c: &Character, image: &RgbaImage) -> Option<ReminderStage> {
    // 无效帧不能解释为颜色消失；与监控循环一样重置提醒状态。
    if !c.fits(image) {
      *self = Self::default();
      return None;
    }
    let rgb1 = c.reminder_now_region.matches(image);
    match self {
      Self::Idle if rgb1 => {
        *self = Self::WaitingRgb2;
        self.advance_detection(c, image);
        return Some(ReminderStage::LastWave);
      }
      Self::WaitingRgb2 | Self::WaitingRgb3 | Self::Rgb3Seen if !rgb1 => {
        *self = Self::Idle;
        return Some(ReminderStage::Ended);
      }
      // matches 对整个 RGB3 列表取任意命中；取反表示所有目标色均已消失。
      Self::Rgb3Seen if !c.reminder_enemy_region.matches(image) => {
        // RGB1 仍在，等待其消失，避免同一轮再次发送第一次提醒。
        *self = Self::WaitingRgb1Clear;
        return Some(ReminderStage::Ended);
      }
      Self::WaitingRgb1Clear if !rgb1 => *self = Self::Idle,
      Self::WaitingRgb2 | Self::WaitingRgb3 => self.advance_detection(c, image),
      _ => {}
    }
    None
  }

  fn advance_detection(&mut self, c: &Character, image: &RgbaImage) {
    if matches!(self, Self::WaitingRgb2) && c.reminder_gate_region.matches(image) {
      *self = Self::WaitingRgb3;
    }
    // 同帧满足前置条件后立即检测 RGB3；RGB2 后续消失不撤销前置记录。
    if matches!(self, Self::WaitingRgb3) && c.reminder_enemy_region.matches(image) {
      *self = Self::Rgb3Seen;
    }
  }
}
