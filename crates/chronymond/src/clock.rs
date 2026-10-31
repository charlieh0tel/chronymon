//! Steps seen on the clock: the realtime clock moving against boot time
//! between two polls by more than any slew could.  `makestep`'s rule
//! says which corrections chronyd steps, from its configuration; it
//! cannot see `chronyc makestep`, chronyd's `-R`, or anything else
//! setting the clock.  Where the two disagree, it is journaled.
//!
//! Boot time, not the monotonic clock, which stops while the host is
//! suspended: a resume would look like a step.

use jiff::Timestamp;
use rustix::time::ClockId;
use rustix::time::Timespec;
use rustix::time::clock_gettime;

use chronymon::event::EventKind;

use crate::time::NANOSECOND;

/// The most a slew can move the clock against boot time, as a fraction
/// of the time elapsed: Linux's tick adjustment, which chronyd slews
/// with, reaches 10%, above chronyd's default `maxslewrate`, 1/12
/// (4.8 `conf.c`, `max_slew_rate`; `sys_linux.c`).
const MAX_SLEW: f64 = 0.1;

/// Allowance, in seconds, for reading the two clocks a moment apart
/// and a poll late.
const MARGIN: f64 = 0.05;

/// How many polls a step on the clock and one by `makestep`'s rule may
/// be apart and be the same step: chronyd may step between the daemon
/// reading the clocks and reading its report.
const SAME_STEP: u32 = 3;

/// Seconds in a day, for a step at midnight UTC: a leap second.
const DAY: f64 = 86_400.0;

/// A leap second.
const LEAP_SECOND: f64 = 1.0;

/// Both clocks at one moment, in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Reading {
    realtime: f64,
    boottime: f64,
}

impl Reading {
    /// The clocks now.
    pub(crate) fn now() -> Self {
        Self {
            realtime: seconds(clock_gettime(ClockId::Realtime)),
            boottime: seconds(clock_gettime(ClockId::Boottime)),
        }
    }
}

fn seconds(time: Timespec) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "seconds since the epoch or boot fit a double's 53 bits"
    )]
    let (seconds, nanoseconds) = (time.tv_sec as f64, time.tv_nsec as f64);
    seconds + nanoseconds * NANOSECOND
}

/// A step not yet matched: when, how many polls ago, and its size in
/// seconds, the clock's move or the update's offset.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pending {
    at: Timestamp,
    polls: u32,
    seconds: f64,
}

/// Steps seen and steps by the rule, poll to poll.
#[derive(Debug, Default)]
pub(crate) struct Steps {
    last: Option<Reading>,
    seen: Option<Pending>,
    ruled: Option<Pending>,
}

/// An event to journal: when, its kind and detail.
type Journaled = (Timestamp, EventKind, String);

impl Steps {
    /// Take in the clocks read at `at`, and the offset of a step
    /// `makestep`'s rule said this poll, and say what to journal.
    pub(crate) fn observe(
        &mut self,
        at: Timestamp,
        now: Reading,
        ruled: Option<f64>,
    ) -> Vec<Journaled> {
        for pending in [&mut self.seen, &mut self.ruled].into_iter().flatten() {
            pending.polls += 1;
        }
        // The smallest move no slew could make since the last poll: a
        // ruled step under it could not be seen, so is not looked for.
        let last = self.last.replace(now);
        let seeable = last.map_or(f64::INFINITY, |last| {
            MAX_SLEW * (now.boottime - last.boottime).abs() + MARGIN
        });
        if let Some(last) = last {
            let moved = (now.realtime - last.realtime) - (now.boottime - last.boottime);
            if moved.abs() > seeable {
                self.seen = Some(Pending {
                    at,
                    polls: 0,
                    seconds: moved,
                });
            }
        }
        if let Some(offset) = ruled.filter(|offset| offset.abs() > seeable) {
            self.ruled = Some(Pending {
                at,
                polls: 0,
                seconds: offset,
            });
        }
        if self.seen.is_some() && self.ruled.is_some() {
            self.seen = None;
            self.ruled = None;
        }
        let mut journal = Vec::new();
        if let Some(seen) = self.seen.filter(|p| p.polls >= SAME_STEP) {
            let leap = if leap_second(&seen, now.realtime) {
                ", at midnight UTC: a leap second"
            } else {
                ""
            };
            journal.push((
                seen.at,
                EventKind::ClockStep,
                format!(
                    "{:+.3} s, not a step by makestep's rule{leap}",
                    seen.seconds
                ),
            ));
            self.seen = None;
        }
        if let Some(ruled) = self.ruled.filter(|p| p.polls >= SAME_STEP) {
            journal.push((
                ruled.at,
                EventKind::StepUnseen,
                format!(
                    "makestep's rule stepped {} s; the clock did not jump",
                    ruled.seconds
                ),
            ));
            self.ruled = None;
        }
        journal
    }
}

/// Whether a step `seen` is a leap second: a second, close to midnight
/// UTC, `now` being the realtime clock's.
fn leap_second(seen: &Pending, now: f64) -> bool {
    let since_midnight = now.rem_euclid(DAY);
    let near = since_midnight < f64::from(SAME_STEP + 2) || DAY - since_midnight < 1.0;
    near && (seen.seconds.abs() - LEAP_SECOND).abs() < MAX_SLEW + MARGIN
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clocks `second` seconds after an arbitrary start, the
    /// realtime clock `ahead` seconds off boot time.
    fn reading(second: f64, ahead: f64) -> Reading {
        Reading {
            realtime: 1_800_000_000.0 + second + ahead,
            boottime: 1000.0 + second,
        }
    }

    fn at(second: i64) -> Timestamp {
        Timestamp::new(1_800_000_000 + second, 0).expect("in range")
    }

    /// The kinds journaled for `polls`, each the clocks' offset and a
    /// ruled step's offset, a second apart.
    fn kinds(steps: &mut Steps, polls: &[(f64, Option<f64>)]) -> Vec<&'static str> {
        polls
            .iter()
            .enumerate()
            .flat_map(|(n, &(ahead, ruled))| {
                let second = i64::try_from(n).expect("few");
                steps.observe(at(second), reading(second as f64, ahead), ruled)
            })
            .map(|(_, kind, _)| kind.name())
            .collect()
    }

    #[test]
    fn a_slew_is_no_step() {
        let mut steps = Steps::default();
        // 50 ms a second, faster than chronyd's default 1/12.
        let slewing: Vec<(f64, Option<f64>)> =
            (0..10).map(|n| (f64::from(n) * 0.05, None)).collect();
        assert!(kinds(&mut steps, &slewing).is_empty());
    }

    #[test]
    fn a_step_the_rule_said_is_journaled_by_it_alone() {
        let mut steps = Steps::default();
        // Ruled a poll before the clock jumps, and a poll after.
        let polls = [
            (0.0, None),
            (0.0, Some(1.5)),
            (-1.5, None),
            (-1.5, None),
            (-1.5, None),
            (-1.5, None),
        ];
        assert!(kinds(&mut steps, &polls).is_empty());
        let mut steps = Steps::default();
        let polls = [
            (0.0, None),
            (-1.5, None),
            (-1.5, Some(1.5)),
            (-1.5, None),
            (-1.5, None),
        ];
        assert!(kinds(&mut steps, &polls).is_empty());
    }

    #[test]
    fn a_step_the_rule_did_not_say_is_journaled() {
        let mut steps = Steps::default();
        let polls = [
            (0.0, None),
            (2.0, None),
            (2.0, None),
            (2.0, None),
            (2.0, None),
        ];
        let journal: Vec<Journaled> = polls
            .iter()
            .enumerate()
            .flat_map(|(n, &(ahead, ruled))| {
                steps.observe(at(n as i64), reading(n as f64, ahead), ruled)
            })
            .collect();
        assert_eq!(
            journal,
            [(
                at(1),
                EventKind::ClockStep,
                "+2.000 s, not a step by makestep's rule".to_owned()
            )]
        );
    }

    #[test]
    fn a_step_the_rule_said_but_the_clock_did_not_take_is_journaled() {
        let mut steps = Steps::default();
        let polls = [
            (0.0, None),
            (0.0, Some(1.5)),
            (0.0, None),
            (0.0, None),
            (0.0, None),
        ];
        assert_eq!(kinds(&mut steps, &polls), ["step_unseen"]);
    }

    #[test]
    fn a_step_too_small_to_see_is_not_looked_for() {
        let mut steps = Steps::default();
        let polls = [
            (0.0, None),
            (0.0, Some(0.12)),
            (0.0, None),
            (0.0, None),
            (0.0, None),
        ];
        assert!(kinds(&mut steps, &polls).is_empty());
    }

    #[test]
    fn a_suspend_is_no_step() {
        // Boot time runs on through a suspend, as the realtime clock does.
        let mut steps = Steps::default();
        let mut observe = |second: f64| steps.observe(at(0), reading(second, 0.0), None);
        assert!(observe(0.0).is_empty());
        assert!(observe(3600.0).is_empty());
        for second in 3601..3606 {
            assert!(observe(f64::from(second)).is_empty());
        }
    }

    #[test]
    fn a_second_back_at_midnight_is_a_leap_second() {
        let seen = Pending {
            at: at(0),
            polls: SAME_STEP,
            seconds: -1.0,
        };
        let midnight = 1_798_761_600.0;
        assert!(leap_second(&seen, midnight + 2.0));
        assert!(!leap_second(&seen, midnight + 3600.0));
        assert!(!leap_second(
            &Pending {
                seconds: -2.0,
                ..seen
            },
            midnight + 2.0
        ));
    }
}
