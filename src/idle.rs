use std::time::{Duration, Instant};

/// Whether the deck's panel should be dark. It goes dark only once every
/// light is off and nothing has been touched for a while; a light being on
/// (whether controlled from this application or from anywhere else), keeps
/// the deck lit.
pub struct Idle {
    timeout: Duration,
    last: Instant,
    dark: bool,
}

impl Idle {
    pub fn new(timeout: Duration, now: Instant) -> Self {
        Idle { timeout, last: now, dark: false }
    }

    pub fn dark(&self) -> bool {
        self.dark
    }

    /// Note input from the deck. Returns false if the input only woke the
    /// panel and should go no further, so a blind press can't toggle a light.
    pub fn input(&mut self, now: Instant) -> bool {
        self.last = now;
        !std::mem::replace(&mut self.dark, false)
    }

    /// Called on every pass of the event loop.
    pub fn tick(&mut self, now: Instant, any_on: bool) {
        if any_on {
            self.last = now;
            self.dark = false;
        } else if now.duration_since(self.last) >= self.timeout {
            self.dark = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(600);

    fn later(start: Instant, seconds: u64) -> Instant {
        start + Duration::from_secs(seconds)
    }

    #[test]
    fn goes_dark_after_the_timeout_with_lights_off() {
        let start = Instant::now();
        let mut idle = Idle::new(TIMEOUT, start);
        idle.tick(later(start, 599), false);
        assert!(!idle.dark());
        idle.tick(later(start, 600), false);
        assert!(idle.dark());
    }

    #[test]
    fn stays_lit_while_any_light_is_on() {
        let start = Instant::now();
        let mut idle = Idle::new(TIMEOUT, start);
        idle.tick(later(start, 3600), true);
        assert!(!idle.dark());
    }

    #[test]
    fn timer_starts_when_the_last_light_goes_off() {
        let start = Instant::now();
        let mut idle = Idle::new(TIMEOUT, start);
        idle.tick(later(start, 3000), true);
        idle.tick(later(start, 3001), false);
        idle.tick(later(start, 3599), false);
        assert!(!idle.dark());
        idle.tick(later(start, 3600), false);
        assert!(idle.dark());
    }

    #[test]
    fn input_restarts_the_timer() {
        let start = Instant::now();
        let mut idle = Idle::new(TIMEOUT, start);
        assert!(idle.input(later(start, 500)));
        idle.tick(later(start, 1000), false);
        assert!(!idle.dark());
        idle.tick(later(start, 1100), false);
        assert!(idle.dark());
    }

    #[test]
    fn input_that_wakes_goes_no_further() {
        let start = Instant::now();
        let mut idle = Idle::new(TIMEOUT, start);
        idle.tick(later(start, 600), false);
        assert!(idle.dark());

        assert!(!idle.input(later(start, 700)), "the waking press is swallowed");
        assert!(!idle.dark());
        assert!(idle.input(later(start, 701)), "the next one acts");
    }

    #[test]
    fn a_light_turned_on_elsewhere_wakes_it() {
        let start = Instant::now();
        let mut idle = Idle::new(TIMEOUT, start);
        idle.tick(later(start, 600), false);
        assert!(idle.dark());
        idle.tick(later(start, 700), true);
        assert!(!idle.dark());
        assert!(idle.input(later(start, 701)), "already awake, so input acts");
    }
}
