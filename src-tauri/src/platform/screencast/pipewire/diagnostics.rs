use std::{
  num::NonZeroU32,
  sync::atomic::{AtomicU64, Ordering},
  time::Duration,
};

use pipewire::spa::{param::video::VideoInfoRaw, utils::Fraction};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub(super) const REPORT_INTERVAL: Duration = Duration::from_secs(5);

// 只使用本次进程内的诊断编号，不记录来源名称、Portal 凭据或完整配置。
pub(super) struct FrameStats {
  id: u64,
  requested: Option<NonZeroU32>,
  negotiated: Option<(Fraction, Fraction)>,
  since: Duration,
  finished: bool,
  pub received: u64,
  pub valid: u64,
  pub invalid: u64,
  pub converted: u64,
  pub skipped: u64,
  pub mailbox_dropped: u64,
  delivered: u64,
  conversion_total: Duration,
  conversion_max: Duration,
  last_delivery: Option<Duration>,
  interval_count: u64,
  interval_total: Duration,
  interval_min: Option<Duration>,
  interval_max: Duration,
}

pub(super) fn upstream_is_limited(format: VideoInfoRaw, requested: NonZeroU32) -> bool {
  if format.framerate().denom == 0 {
    return false;
  }
  let rate = if format.framerate().num == 0 {
    format.max_framerate()
  } else {
    format.framerate()
  };
  rate.num > 0
    && rate.denom > 0
    && u64::from(rate.num) <= u64::from(requested.get()) * u64::from(rate.denom)
}

impl FrameStats {
  pub fn new(requested: Option<NonZeroU32>, since: Duration) -> Self {
    Self::with_id(NEXT_ID.fetch_add(1, Ordering::Relaxed), requested, since)
  }

  fn with_id(id: u64, requested: Option<NonZeroU32>, since: Duration) -> Self {
    Self {
      id,
      requested,
      since,
      finished: false,
      negotiated: None,
      received: 0,
      valid: 0,
      invalid: 0,
      converted: 0,
      skipped: 0,
      mailbox_dropped: 0,
      delivered: 0,
      conversion_total: Duration::ZERO,
      conversion_max: Duration::ZERO,
      last_delivery: None,
      interval_count: 0,
      interval_total: Duration::ZERO,
      interval_min: None,
      interval_max: Duration::ZERO,
    }
  }

  pub fn negotiated(&mut self, format: VideoInfoRaw) {
    self.negotiated = Some((format.framerate(), format.max_framerate()));
    log::info!(
      "PipeWire 捕获 #{}: 请求上限={:?} FPS, 协商 framerate={:?}, max_framerate={:?}, 尺寸={}×{}",
      self.id,
      self.requested.map(NonZeroU32::get),
      format.framerate(),
      format.max_framerate(),
      format.size().width,
      format.size().height
    );
    if self
      .requested
      .is_some_and(|fps| !upstream_is_limited(format, fps))
    {
      log::warn!(
        "PipeWire 捕获 #{}: 上游上限未确认或超过请求，使用本地限速；不重连、不重新授权",
        self.id
      );
    }
  }

  pub fn conversion(&mut self, duration: Duration) {
    self.conversion_total += duration;
    self.conversion_max = self.conversion_max.max(duration);
  }

  pub fn delivered(&mut self, elapsed: Duration) {
    self.delivered += 1;
    if let Some(last) = self.last_delivery.replace(elapsed) {
      let interval = elapsed.saturating_sub(last);
      self.interval_count += 1;
      self.interval_total += interval;
      self.interval_min = Some(self.interval_min.map_or(interval, |min| min.min(interval)));
      self.interval_max = self.interval_max.max(interval);
    }
  }

  pub fn report(&mut self, elapsed: Duration, final_report: bool) {
    if self.finished {
      return;
    }
    let window = elapsed.saturating_sub(self.since);
    if !final_report && window < REPORT_INTERVAL {
      return;
    }
    let seconds = window.as_secs_f64();
    let rate = |count: u64| {
      if seconds > 0.0 {
        count as f64 / seconds
      } else {
        0.0
      }
    };
    let average_ms = |total: Duration, count: u64| {
      if count > 0 {
        total.as_secs_f64() * 1000.0 / count as f64
      } else {
        0.0
      }
    };
    log::info!(
      "PipeWire 捕获 #{}{}: 窗口={:.2}s 请求上限={:?} FPS 协商={:?}; received={} ({:.2}/s) valid={} invalid={} converted={} ({:.2}/s) skipped={} delivered={} ({:.2}/s) mailbox_dropped={}; 转换 avg/max={:.3}/{:.3}ms 交付间隔 n={} avg/min/max={:.3}/{:?}/{:.3}ms",
      self.id,
      if final_report { " 结束" } else { "" },
      seconds,
      self.requested.map(NonZeroU32::get),
      self.negotiated,
      self.received,
      rate(self.received),
      self.valid,
      self.invalid,
      self.converted,
      rate(self.converted),
      self.skipped,
      self.delivered,
      rate(self.delivered),
      self.mailbox_dropped,
      average_ms(self.conversion_total, self.converted),
      self.conversion_max.as_secs_f64() * 1000.0,
      self.interval_count,
      average_ms(self.interval_total, self.interval_count),
      self.interval_min.map(|v| v.as_secs_f64() * 1000.0),
      self.interval_max.as_secs_f64() * 1000.0
    );
    // 保留来源、协商信息和最后交付时间，其他字段按统计窗口清零。
    let mut next = Self::with_id(self.id, self.requested, elapsed);
    next.negotiated = self.negotiated;
    next.last_delivery = self.last_delivery;
    next.finished = final_report;
    *self = next;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn checks_effective_negotiated_rate_including_missing_and_fractional_values() {
    let mut format = VideoInfoRaw::default();
    let requested = NonZeroU32::new(2).unwrap();
    for (rate, max, expected) in [
      ((0, 1), (2, 1), true),
      ((0, 1), (119998, 1000), false),
      ((0, 1), (0, 1), false),
      ((0, 1), (2, 0), false),
      ((0, 0), (2, 1), false),
      ((1999, 1000), (60, 1), true),
      ((3, 1), (2, 1), false),
    ] {
      format.set_framerate(Fraction {
        num: rate.0,
        denom: rate.1,
      });
      format.set_max_framerate(Fraction {
        num: max.0,
        denom: max.1,
      });
      assert_eq!(upstream_is_limited(format, requested), expected);
    }
  }
}
