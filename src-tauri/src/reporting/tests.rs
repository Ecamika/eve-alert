use super::ReportEvent;
use super::config::*;
use image::{Rgba, RgbaImage};

fn region(x: u32, color: [u8; 3]) -> Region {
  Region {
    start: [x, 0],
    end: [x, 0],
    rgb: vec![color],
  }
}
fn character() -> Character {
  Character {
    title: "EVE - test".into(),
    enabled: true,
    warn_region: region(0, [255, 0, 0]),
    reminder_now_region: region(1, [0, 255, 0]),
    reminder_gate_region: region(4, [255, 128, 0]),
    reminder_enemy_region: region(2, [0, 0, 255]),
  }
}

#[test]
fn matches_inclusive_boundary_and_ignores_alpha_without_accepting_nearby_colors() {
  let mut image = RgbaImage::new(3, 2);
  image.put_pixel(2, 1, Rgba([10, 20, 30, 0]));
  let mut r = Region {
    start: [1, 0],
    end: [2, 1],
    rgb: vec![[10, 20, 30]],
  };
  assert!(r.matches(&image));
  r.rgb = vec![[10, 20, 31]];
  assert!(!r.matches(&image));
  r.start = [10, 10];
  r.end = [u32::MAX, u32::MAX];
  assert!(!r.matches(&image));
  assert!(!r.matches(&RgbaImage::new(0, 0)));
}

fn reminder_frame(rgb1: bool, rgb2: bool, rgb3: bool) -> RgbaImage {
  let mut image = RgbaImage::new(5, 1);
  if rgb1 {
    image.put_pixel(1, 0, Rgba([0, 255, 0, 255]));
  }
  if rgb2 {
    image.put_pixel(4, 0, Rgba([255, 128, 0, 255]));
  }
  if rgb3 {
    image.put_pixel(2, 0, Rgba([0, 0, 255, 255]));
  }
  image
}

#[test]
fn reminder_reports_both_stages_and_requires_rgb1_to_clear_before_rearming() {
  use ReminderStage::{Ended, LastWave};
  let c = character();
  let cases = [
    (
      "RGB2 是 RGB3 的前置，RGB2 消失不撤销，结束后需重新开启前置",
      vec![
        (false, true, true, None),
        (true, false, true, Some(LastWave)),
        (true, false, false, None),
        (true, true, false, None),
        (true, false, true, None),
        (true, false, true, None),
        (true, false, false, Some(Ended)),
        (true, true, true, None),
        (false, false, false, None),
        (true, false, true, Some(LastWave)),
        (true, false, false, None),
        (false, false, false, Some(Ended)),
        (false, false, false, None),
      ],
    ),
    (
      "等待 RGB2 或 RGB3 时 RGB1 消失均结束，下一轮不继承前置记录",
      vec![
        (true, false, false, Some(LastWave)),
        (false, true, true, Some(Ended)),
        (true, true, false, Some(LastWave)),
        (true, false, false, None),
        (false, false, true, Some(Ended)),
        (true, false, true, Some(LastWave)),
        (true, false, false, None),
        (false, false, false, Some(Ended)),
      ],
    ),
    (
      "首次命中同帧记录 RGB2 和 RGB3，多色同时消失仅结束一次",
      vec![
        (true, true, true, Some(LastWave)),
        (true, false, false, Some(Ended)),
        (false, false, false, None),
        (true, true, true, Some(LastWave)),
        (false, false, false, Some(Ended)),
        (false, false, false, None),
      ],
    ),
    (
      "本轮中 RGB2 命中的同一帧立即记录 RGB3",
      vec![
        (true, false, false, Some(LastWave)),
        (true, false, true, None),
        (true, true, true, None),
        (true, true, false, Some(Ended)),
        (true, true, false, None),
      ],
    ),
  ];
  for (name, frames) in cases {
    let mut state = ReminderState::default();
    for (index, (rgb1, rgb2, rgb3, expected)) in frames.into_iter().enumerate() {
      assert_eq!(
        state.advance(&c, &reminder_frame(rgb1, rgb2, rgb3)),
        expected,
        "{name}，帧 {index}"
      );
    }
  }
}

#[test]
fn reminder_records_any_rgb3_color_and_ends_only_after_all_configured_colors_disappear() {
  let colors = [[0, 0, 255], [255, 255, 0], [255, 0, 255]];
  let mut c = character();
  c.reminder_enemy_region.end = [3, 0];
  c.reminder_enemy_region.rgb = colors.to_vec();
  let frame = |pixels: &[(u32, [u8; 3])]| {
    let mut image = RgbaImage::new(5, 1);
    // 保持 RGB1 和前置 RGB2 命中，验证 RGB3 颜色列表的结束条件。
    image.put_pixel(4, 0, Rgba([255, 128, 0, 255]));
    image.put_pixel(1, 0, Rgba([0, 255, 0, 255]));
    for &(x, [r, g, b]) in pixels {
      image.put_pixel(x, 0, Rgba([r, g, b, 255]));
    }
    image
  };
  for (index, color) in colors.into_iter().enumerate() {
    let mut state = ReminderState::default();
    assert_eq!(
      state.advance(&c, &frame(&[])),
      Some(ReminderStage::LastWave)
    );
    // 任意单个配置颜色就能建立记录，无需所有颜色都出现。
    assert_eq!(state.advance(&c, &frame(&[(2, color)])), None);
    assert_eq!(state.advance(&c, &frame(&[])), Some(ReminderStage::Ended));

    let mut state = ReminderState::default();
    assert_eq!(
      state.advance(&c, &frame(&[])),
      Some(ReminderStage::LastWave)
    );
    assert_eq!(state.advance(&c, &frame(&[(2, color)])), None);
    let other = colors[(index + 1) % colors.len()];
    let last = colors[(index + 2) % colors.len()];
    // 最初命中的颜色消失，但其他配置颜色仍在（包括换到别的像素）不能结束。
    assert_eq!(state.advance(&c, &frame(&[(3, other)])), None);
    assert_eq!(state.advance(&c, &frame(&[(2, other), (3, last)])), None);
    assert_eq!(state.advance(&c, &frame(&[(3, last)])), None);
    assert_eq!(state.advance(&c, &frame(&[])), Some(ReminderStage::Ended));
    assert_eq!(state.advance(&c, &frame(&[])), None);
    // RGB1 未清除前，RGB3 再次出现也不能开启新一轮。
    assert_eq!(state.advance(&c, &frame(&[(2, color)])), None);
  }
}

#[test]
fn reminder_states_are_independent_per_character() {
  let c = character();
  let mut first = ReminderState::default();
  let mut second = ReminderState::default();
  assert_eq!(
    first.advance(&c, &reminder_frame(true, true, true)),
    Some(ReminderStage::LastWave)
  );
  assert_eq!(second.advance(&c, &reminder_frame(false, true, true)), None);
  assert_eq!(
    first.advance(&c, &reminder_frame(true, true, false)),
    Some(ReminderStage::Ended)
  );
  assert_eq!(
    second.advance(&c, &reminder_frame(true, true, false)),
    Some(ReminderStage::LastWave)
  );
  assert_eq!(first.advance(&c, &reminder_frame(true, true, true)), None);
  assert_eq!(
    second.advance(&c, &reminder_frame(false, true, false)),
    Some(ReminderStage::Ended)
  );
}

#[test]
fn invalid_frames_reset_reminders_without_reporting_an_end() {
  let c = character();
  for invalid in [RgbaImage::new(4, 1), RgbaImage::new(0, 0)] {
    let mut state = ReminderState::default();
    assert_eq!(
      state.advance(&c, &reminder_frame(true, true, true)),
      Some(ReminderStage::LastWave)
    );
    assert_eq!(state.advance(&c, &invalid), None);
    assert_eq!(state.advance(&c, &reminder_frame(false, true, false)), None);
    assert_eq!(
      state.advance(&c, &reminder_frame(true, true, false)),
      Some(ReminderStage::LastWave)
    );
  }
}

#[test]
fn legacy_characters_default_to_enabled_and_disabled_characters_round_trip() {
  let mut legacy = serde_json::to_value(character()).unwrap();
  legacy.as_object_mut().unwrap().remove("enabled");
  let decoded: Character = serde_json::from_value(legacy).unwrap();
  assert!(decoded.enabled);
  let disabled = Character {
    enabled: false,
    ..decoded
  };
  let saved = serde_json::to_value(&disabled).unwrap();
  assert_eq!(saved["enabled"], false);
  assert!(!serde_json::from_value::<Character>(saved).unwrap().enabled);
}

#[test]
fn legacy_reminder_regions_are_preserved_and_gate_is_an_independent_copy() {
  let mut legacy = serde_json::to_value(character()).unwrap();
  legacy
    .as_object_mut()
    .unwrap()
    .remove("reminder_gate_region");
  let mut decoded: Character = serde_json::from_value(legacy).unwrap();
  assert_eq!(decoded.reminder_enemy_region.start, [2, 0]);
  assert_eq!(decoded.reminder_gate_region.start, [2, 0]);
  assert_eq!(decoded.reminder_gate_region.rgb, vec![[0, 0, 255]]);
  decoded.reminder_gate_region.start = [4, 0];
  decoded.reminder_gate_region.end = [4, 0];
  decoded.reminder_gate_region.rgb = vec![[255, 128, 0]];
  assert_eq!(decoded.reminder_enemy_region.start, [2, 0]);
  assert_eq!(decoded.reminder_enemy_region.rgb, vec![[0, 0, 255]]);
  let saved = serde_json::to_value(&decoded).unwrap();
  let reloaded: Character = serde_json::from_value(saved).unwrap();
  assert_eq!(reloaded.reminder_gate_region.start, [4, 0]);
  assert_eq!(reloaded.reminder_gate_region.rgb, vec![[255, 128, 0]]);
  let config = Config {
    characters: vec![reloaded],
    report_methods: vec![],
  };
  assert!(config.validate().is_ok());
}

#[test]
fn monitoring_excludes_disabled_characters_and_rejects_an_empty_enabled_set() {
  let enabled = character();
  let mut disabled = character();
  disabled.title = "EVE - disabled".into();
  disabled.enabled = false;
  disabled.warn_region.end = [999, 999];
  let config = Config {
    characters: vec![disabled.clone(), enabled],
    report_methods: vec![ReportMethod::Notification],
  };
  assert!(config.validate().is_ok());
  let active = config.clone().into_monitoring().unwrap();
  assert_eq!(active.characters.len(), 1);
  assert_eq!(active.characters[0].title, "EVE - test");
  assert!(active.characters[0].fits(&RgbaImage::new(5, 1)));
  assert!(matches!(
    active.report_methods.as_slice(),
    [ReportMethod::Notification]
  ));
  // 启动时的筛选不删除持久配置，也不改变禁用角色的识别设置。
  assert_eq!(config.characters.len(), 2);
  assert_eq!(config.characters[0].warn_region.end, [999, 999]);
  assert!(
    Config {
      characters: vec![disabled],
      report_methods: vec![]
    }
    .into_monitoring()
    .is_err()
  );
  assert!(Config::default().into_monitoring().is_err());
}

#[test]
fn rejects_invalid_regions_duplicate_characters_and_duplicate_reporters() {
  let mut config = Config {
    characters: vec![character()],
    report_methods: vec![ReportMethod::Notification],
  };
  assert!(config.validate().is_ok());
  config.characters[0].reminder_gate_region.rgb.clear();
  assert!(config.validate().is_err());
  config.characters[0] = character();
  config.characters[0].warn_region.rgb.clear();
  assert!(config.validate().is_err());
  config.characters[0] = character();
  config.characters.push(character());
  assert!(config.validate().is_err());
  config.characters.pop();
  config.report_methods.push(ReportMethod::Notification);
  assert!(config.validate().is_err());
}

#[test]
fn events_preserve_reference_wire_protocol_and_config_round_trips() {
  let event = ReportEvent::Warn {
    title: "EVE - test".into(),
  };
  assert_eq!(
    serde_json::to_value(event).unwrap(),
    serde_json::json!({ "type": "Warn", "title": "EVE - test" })
  );
  let cleared = ReportEvent::WarnCleared {
    title: "EVE - test".into(),
  };
  assert_eq!(
    serde_json::to_value(&cleared).unwrap(),
    serde_json::json!({ "type": "WarnCleared", "title": "EVE - test" })
  );
  assert_eq!(cleared.onebot_text(), "WarnCleared EVE - test");
  for (stage, stage_name, message) in [
    (ReminderStage::LastWave, "LastWave", "异常进入最后一波次"),
    (ReminderStage::Ended, "Ended", "异常结束"),
  ] {
    let event = ReportEvent::reminder("EVE - test".into(), stage);
    assert_eq!(
      serde_json::to_value(&event).unwrap(),
      serde_json::json!({
        "type": "Reminder", "title": "EVE - test", "stage": stage_name, "message": message
      })
    );
    assert_eq!(event.notification_body(), format!("EVE - test：{message}"));
    assert_eq!(
      event.onebot_text(),
      format!("Reminder EVE - test：{message}")
    );
  }
  let config = Config {
    characters: vec![character()],
    report_methods: vec![ReportMethod::ReverseWebsocket {
      url: "wss://example.com/ws".into(),
      try_forever: true,
      try_spacing: std::time::Duration::from_secs(5),
    }],
  };
  let decoded: Config = serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
  assert!(decoded.validate().is_ok());
  assert_eq!(
    decoded.characters[0].reminder_enemy_region.rgb,
    vec![[0, 0, 255]]
  );
}

#[test]
fn detects_regions_outside_a_resized_frame() {
  let c = character();
  assert!(c.fits(&RgbaImage::new(5, 1)));
  assert!(!c.fits(&RgbaImage::new(4, 1)));
  assert!(!c.fits(&RgbaImage::new(5, 0)));
}

#[test]
fn warning_only_reports_appearance_and_disappearance_and_rearms_per_character() {
  let mut state = WarningState::default();
  let mut other = WarningState::default();
  let r = Region {
    start: [0, 0],
    end: [1, 0],
    rgb: vec![[255, 0, 0], [0, 255, 0]],
  };
  let mut image = RgbaImage::new(2, 1);
  assert_eq!(state.advance(&r, &image), None);
  image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
  assert_eq!(state.advance(&r, &image), Some(true));
  assert_eq!(state.advance(&r, &image), None);
  assert_eq!(other.advance(&r, &image), Some(true));
  image.put_pixel(1, 0, Rgba([0, 255, 0, 255]));
  image.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
  assert_eq!(state.advance(&r, &image), None);
  image.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
  assert_eq!(state.advance(&r, &image), Some(false));
  assert_eq!(state.advance(&r, &image), None);
  image.put_pixel(1, 0, Rgba([0, 255, 0, 255]));
  assert_eq!(state.advance(&r, &image), Some(true));
}

#[test]
fn invalid_frames_do_not_clear_an_active_warning() {
  let mut state = WarningState::default();
  let r = region(0, [255, 0, 0]);
  let image = RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255]));
  assert_eq!(state.advance(&r, &image), Some(true));
  assert_eq!(state.advance(&r, &RgbaImage::new(0, 0)), None);
  assert_eq!(state.advance(&r, &image), None);
  assert_eq!(state.advance(&r, &RgbaImage::new(1, 1)), Some(false));
}

#[test]
fn ntfy_config_validates_endpoints_topics_and_tokens_and_round_trips() {
  let mut config = Config {
    characters: vec![],
    report_methods: vec![ReportMethod::Ntfy {
      server_url: "https://example.com/ntfy".into(),
      topic: "eve_test-1".into(),
      token: "tk_test".into(),
    }],
  };
  let decoded: Config = serde_json::from_slice(&serde_json::to_vec(&config).unwrap()).unwrap();
  assert!(decoded.validate().is_ok());
  for (url, topic, token) in [
    ("file:///tmp/ntfy", "eve", ""),
    ("https://example.com/?x=1", "eve", ""),
    ("https://example.com", "", ""),
    ("https://example.com", "a/b", ""),
    ("https://example.com", "eve", "bad\ntoken"),
  ] {
    config.report_methods = vec![ReportMethod::Ntfy {
      server_url: url.into(),
      topic: topic.into(),
      token: token.into(),
    }];
    assert!(config.validate().is_err());
  }
}
