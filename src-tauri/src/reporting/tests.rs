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
    warn_region: region(0, [255, 0, 0]),
    reminder_now_region: region(1, [0, 255, 0]),
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

#[test]
fn reminder_requires_preparation_enemy_and_then_disappearance_and_rearms() {
  let c = character();
  let mut state = ReminderState::default();
  let mut image = RgbaImage::new(3, 1);
  image.put_pixel(2, 0, Rgba([0, 0, 255, 255]));
  assert!(!state.advance(&c, &image));
  image.put_pixel(2, 0, Rgba([0, 0, 0, 255]));
  assert!(!state.advance(&c, &image));
  for _ in 0..2 {
    image.put_pixel(1, 0, Rgba([0, 255, 0, 255]));
    assert!(!state.advance(&c, &image));
    image.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
    assert!(!state.advance(&c, &image));
    image.put_pixel(2, 0, Rgba([0, 0, 255, 255]));
    assert!(!state.advance(&c, &image));
    assert!(!state.advance(&c, &image));
    image.put_pixel(2, 0, Rgba([0, 0, 0, 255]));
    assert!(state.advance(&c, &image));
    assert!(!state.advance(&c, &image));
  }
}

#[test]
fn rejects_invalid_regions_duplicate_characters_and_duplicate_reporters() {
  let mut config = Config {
    characters: vec![character()],
    report_methods: vec![ReportMethod::Notification],
  };
  assert!(config.validate().is_ok());
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
  assert!(c.fits(&RgbaImage::new(3, 1)));
  assert!(!c.fits(&RgbaImage::new(2, 1)));
  assert!(!c.fits(&RgbaImage::new(3, 0)));
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
