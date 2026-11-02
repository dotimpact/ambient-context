//! A timed pause: capture stops now and starts again by itself once the
//! chosen time has passed. Unlike Stop Capturing, a pause does not write
//! `enabled = false`, so it is never remembered across a relaunch: an app
//! that starts again during a pause simply records, as it would have done
//! once the pause ran out.

use crate::{capture, reader, settings, tray};
use chrono::{DateTime, Duration, Local};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

/// The lengths offered in the menu, in minutes, with their menu ids.
pub const CHOICES: &[(&str, i64, &str)] = &[
    ("pause_10", 10, "For 10 Minutes"),
    ("pause_30", 30, "For 30 Minutes"),
    ("pause_60", 60, "For 1 Hour"),
    ("pause_120", 120, "For 2 Hours"),
];

/// How often the timer thread wakes to check the clock. Short enough that
/// resuming is on time to the second, long enough to cost nothing.
const TICK: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone, Default)]
pub struct PauseState {
    until: Arc<Mutex<Option<DateTime<Local>>>>,
    /// Bumped on every pause and every cancel. A timer thread acts only
    /// while the generation it was started with is current, so a second
    /// pause or an early resume retires the first timer instead of racing
    /// it.
    generation: Arc<AtomicU64>,
}

impl PauseState {
    /// When capture will start again, or None when nothing is paused.
    pub fn until(&self) -> Option<DateTime<Local>> {
        self.until.lock().ok().and_then(|slot| *slot)
    }

    fn set(&self, until: Option<DateTime<Local>>) -> u64 {
        if let Ok(mut slot) = self.until.lock() {
            *slot = until;
        }
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn is_current(&self, mine: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == mine
    }
}

/// The minutes for a menu id, when the id is one of the pause choices.
pub fn minutes_for(id: &str) -> Option<i64> {
    CHOICES
        .iter()
        .find(|(choice, _, _)| *choice == id)
        .map(|(_, minutes, _)| *minutes)
}

/// Whole minutes left, rounded up so the menu never says 0 while paused.
pub fn minutes_left(until: DateTime<Local>, now: DateTime<Local>) -> i64 {
    let seconds = (until - now).num_seconds().max(0);
    (seconds + 59) / 60
}

/// The disabled line the menu shows while paused, e.g.
/// "Paused until 14:30 (25 min left)".
pub fn status_label(until: DateTime<Local>, now: DateTime<Local>) -> String {
    let left = minutes_left(until, now);
    let left = if left >= 60 {
        let (hours, minutes) = (left / 60, left % 60);
        if minutes == 0 {
            format!("{hours} h")
        } else {
            format!("{hours} h {minutes} min")
        }
    } else {
        format!("{left} min")
    };
    format!("Paused until {} ({left} left)", until.format("%H:%M"))
}

/// Stops capture for `minutes` and arranges for it to start again. Does
/// nothing when capture is not running: there is nothing to pause, and
/// the menu only offers a pause while recording.
pub fn pause(app: &AppHandle, minutes: i64) {
    let capture_state = app.state::<capture::CaptureState>().inner().clone();
    if !capture_state.is_running() {
        return;
    }
    let pause_state = app.state::<PauseState>().inner().clone();
    let until = Local::now() + Duration::minutes(minutes);
    let mine = pause_state.set(Some(until));
    capture::stop(&capture_state);
    tray::refresh(app, false);

    let app = app.clone();
    std::thread::spawn(move || {
        let mut last_label_minute = minutes_left(until, Local::now());
        loop {
            std::thread::sleep(TICK);
            if !pause_state.is_current(mine) {
                return;
            }
            // Wall clock, not Instant: Instant stops while the Mac sleeps,
            // and a one-hour pause must end an hour later on the clock.
            let now = Local::now();
            if now >= until {
                resume_now(&app);
                return;
            }
            // Rebuild the menu once a minute so the time left stays true.
            let left = minutes_left(until, now);
            if left != last_label_minute {
                last_label_minute = left;
                tray::refresh(&app, false);
            }
        }
    });
}

/// Ends a pause early, or on time, and starts capture again if the app is
/// still set up to record.
pub fn resume_now(app: &AppHandle) {
    let pause_state = app.state::<PauseState>().inner().clone();
    if pause_state.until().is_none() {
        return;
    }
    pause_state.set(None);
    let capture_state = app.state::<capture::CaptureState>().inner().clone();
    let config = settings::load(app);
    let can_record = config.enabled
        && config.folder.is_some()
        && reader::macos::permission_status() == reader::Permission::Granted;
    if can_record && !capture_state.is_running() {
        capture::start(app.clone(), &capture_state, config);
    }
    tray::refresh(app, capture_state.is_running());
}

/// Forgets a pause without starting anything. Called when the user starts
/// or stops capture by hand, which says what they want more plainly than a
/// timer set earlier.
pub fn cancel(app: &AppHandle) {
    if let Some(state) = app.try_state::<PauseState>() {
        if state.until().is_some() {
            state.set(None);
        }
    }
}

/// Whether a pause is in force right now.
pub fn is_paused(app: &AppHandle) -> bool {
    app.try_state::<PauseState>()
        .and_then(|state| state.until())
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 1, 15, hour, minute, second)
            .single()
            .unwrap()
    }

    #[test]
    fn every_menu_choice_maps_to_its_minutes() {
        assert_eq!(minutes_for("pause_10"), Some(10));
        assert_eq!(minutes_for("pause_30"), Some(30));
        assert_eq!(minutes_for("pause_60"), Some(60));
        assert_eq!(minutes_for("pause_120"), Some(120));
        assert_eq!(minutes_for("toggle"), None);
    }

    #[test]
    fn minutes_left_rounds_up_and_never_goes_negative() {
        assert_eq!(minutes_left(at(10, 30, 0), at(10, 0, 0)), 30);
        assert_eq!(minutes_left(at(10, 30, 0), at(10, 29, 59)), 1);
        assert_eq!(minutes_left(at(10, 30, 0), at(10, 29, 1)), 1);
        assert_eq!(minutes_left(at(10, 30, 0), at(10, 30, 0)), 0);
        assert_eq!(minutes_left(at(10, 30, 0), at(10, 31, 0)), 0);
    }

    #[test]
    fn the_label_names_the_resume_time_and_what_is_left() {
        assert_eq!(
            status_label(at(14, 30, 0), at(14, 5, 0)),
            "Paused until 14:30 (25 min left)"
        );
        assert_eq!(
            status_label(at(16, 0, 0), at(14, 0, 0)),
            "Paused until 16:00 (2 h left)"
        );
        assert_eq!(
            status_label(at(15, 30, 0), at(14, 0, 0)),
            "Paused until 15:30 (1 h 30 min left)"
        );
    }
}
