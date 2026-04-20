use nix::libc;
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::Duration;

const STRUCTURED_PROFILE_ENV: &str = "HANDTERM_PROFILE_JSON";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessCpuTime {
    pub user: Duration,
    pub system: Duration,
}

impl ProcessCpuTime {
    pub fn capture() -> Option<Self> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }

        let usage = unsafe { usage.assume_init() };
        Some(Self {
            user: duration_from_timeval(usage.ru_utime),
            system: duration_from_timeval(usage.ru_stime),
        })
    }

    pub fn delta_since(self, earlier: Self) -> Self {
        Self {
            user: self.user.saturating_sub(earlier.user),
            system: self.system.saturating_sub(earlier.system),
        }
    }

    pub fn total(self) -> Duration {
        self.user.saturating_add(self.system)
    }

    pub fn user_ms(self) -> f64 {
        duration_ms(self.user)
    }

    pub fn system_ms(self) -> f64 {
        duration_ms(self.system)
    }

    pub fn total_ms(self) -> f64 {
        duration_ms(self.total())
    }
}

fn duration_from_timeval(tv: libc::timeval) -> Duration {
    let secs = u64::try_from(tv.tv_sec).unwrap_or(0);
    let micros = u64::try_from(tv.tv_usec).unwrap_or(0).min(999_999);
    Duration::from_secs(secs).saturating_add(Duration::from_micros(micros))
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn structured_profile_enabled_from(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

pub fn structured_profile_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        structured_profile_enabled_from(std::env::var(STRUCTURED_PROFILE_ENV).ok().as_deref())
    })
}

pub fn structured_profile_event_line(event: &str, data: Value) -> String {
    serde_json::to_string(&json!({
        "type": "handterm_profile",
        "event": event,
        "data": data,
    }))
    .expect("structured profile event should serialize")
}

pub fn emit_structured_profile_event(event: &str, data: Value) {
    if !structured_profile_enabled() {
        return;
    }
    eprintln!("{}", structured_profile_event_line(event, data));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn delta_since_saturates_each_component() {
        let earlier = ProcessCpuTime {
            user: Duration::from_millis(8),
            system: Duration::from_millis(5),
        };
        let later = ProcessCpuTime {
            user: Duration::from_millis(11),
            system: Duration::from_millis(3),
        };

        let delta = later.delta_since(earlier);
        assert_eq!(delta.user, Duration::from_millis(3));
        assert_eq!(delta.system, Duration::ZERO);
        assert_eq!(delta.total(), Duration::from_millis(3));
    }

    #[test]
    fn reports_component_and_total_milliseconds() {
        let cpu = ProcessCpuTime {
            user: Duration::from_micros(1_500),
            system: Duration::from_micros(2_250),
        };

        assert!((cpu.user_ms() - 1.5).abs() < f64::EPSILON);
        assert!((cpu.system_ms() - 2.25).abs() < f64::EPSILON);
        assert!((cpu.total_ms() - 3.75).abs() < f64::EPSILON);
    }

    #[test]
    fn structured_profile_env_accepts_common_truthy_values() {
        for value in [Some("1"), Some("true"), Some("YES"), Some(" on ")] {
            assert!(structured_profile_enabled_from(value));
        }
        for value in [None, Some("0"), Some("false"), Some("no")] {
            assert!(!structured_profile_enabled_from(value));
        }
    }

    #[test]
    fn structured_profile_event_line_is_machine_parsable_json() {
        let line = structured_profile_event_line("gpu_host_open_window", json!({"id": 2}));
        let parsed: serde_json::Value =
            serde_json::from_str(&line).expect("event line should parse as json");
        assert_eq!(parsed["type"], "handterm_profile");
        assert_eq!(parsed["event"], "gpu_host_open_window");
        assert_eq!(parsed["data"]["id"], 2);
    }
}
