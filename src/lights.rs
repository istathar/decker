use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ureq::Agent;

use crate::config::{self, LightConfig};
use crate::discovery::Directory;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct State {
    pub on: bool,
    pub brightness: i32,
    pub temperature: i32,
}

impl Default for State {
    fn default() -> Self {
        State { on: false, brightness: 20, temperature: 230 }
    }
}

/// How to reach a light: where mDNS last saw it, otherwise its configured host.
#[derive(Clone)]
pub struct Locator {
    id: Option<String>,
    host: Option<String>,
    directory: Directory,
}

impl Locator {
    pub fn new(config: &LightConfig, directory: &Directory) -> Self {
        Locator { id: config.id.clone(), host: config.host.clone(), directory: directory.clone() }
    }

    /// host:port, looked up afresh each time so a light that moves is followed.
    /// None if the light has no host and hasn't been found by its id.
    pub fn address(&self) -> Option<String> {
        if let Some(found) = self.id.as_deref().and_then(|id| self.directory.lookup(id)) {
            return Some(found.to_string());
        }
        self.host.as_ref().map(|host| format!("{}:{}", host, config::PORT))
    }
}

/// A state for the worker to write, numbered in the order it was set.
type Change = (u64, State);

pub struct Light {
    pub config: LightConfig,
    pub state: State,
    online: bool,
    /// Set once the light has been read. Until then the state is a guess and
    /// nothing is sent, so a dial turned during startup can't clobber it.
    known: bool,
    /// Bumped with every change handed to the worker.
    sequence: u64,
    locator: Locator,
    sender: Sender<Change>,
}

impl Light {
    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn online(&self) -> bool {
        self.online
    }

    pub fn address(&self) -> Option<String> {
        self.locator.address()
    }

    /// Record the new state locally and hand it to the worker. Never blocks.
    fn push(&mut self, state: State) {
        if !self.known || state == self.state {
            return;
        }
        self.state = state;
        self.sequence += 1;
        let _ = self.sender.send((self.sequence, state));
    }

    pub fn set_on(&mut self, on: bool) {
        let mut state = self.state;
        state.on = on;
        self.push(state);
    }

    pub fn toggle(&mut self) {
        self.set_on(!self.state.on);
    }

    pub fn nudge_brightness(&mut self, delta: i32) {
        let mut state = self.state;
        state.brightness = (state.brightness + delta)
            .clamp(config::BRIGHTNESS_MIN, config::BRIGHTNESS_MAX);
        self.push(state);
    }

    pub fn nudge_temperature(&mut self, delta: i32) {
        let mut state = self.state;
        state.temperature = (state.temperature + delta)
            .clamp(config::TEMPERATURE_MIN, config::TEMPERATURE_MAX);
        self.push(state);
    }
}

/// What a worker learned from its light: the state it read or wrote, or None
/// if the light didn't answer. `sequence` is the newest change the worker had
/// been given at the time, so a report overtaken by later changes is known.
pub struct Report {
    index: usize,
    sequence: u64,
    state: Option<State>,
}

pub struct Lights {
    pub items: Vec<Light>,
    reports: Receiver<Report>,
}

impl Lights {
    /// Start a worker per light. Each reads its light straight away; use
    /// `settle` to wait for that, and `poll` to pick up reports thereafter.
    pub fn connect(configs: Vec<LightConfig>, directory: &Directory) -> Self {
        let (reporter, reports) = channel();

        let items = configs
            .into_iter()
            .enumerate()
            .map(|(index, config)| {
                let locator = Locator::new(&config, directory);
                Light {
                    sender: spawn(index, locator.clone(), reporter.clone()),
                    config,
                    state: State::default(),
                    online: false,
                    known: false,
                    sequence: 0,
                    locator,
                }
            })
            .collect();

        Lights { items, reports }
    }

    /// Wait until every light has reported once, or the timeout passes.
    pub fn settle(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut heard = vec![false; self.items.len()];

        while heard.contains(&false) {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return;
            };
            match self.reports.recv_timeout(remaining) {
                Ok(report) => {
                    if let Some(flag) = heard.get_mut(report.index) {
                        *flag = true;
                    }
                    self.apply(report);
                }
                Err(_) => return,
            }
        }
    }

    /// Apply whatever the workers have reported, without waiting. Returns the
    /// lights whose state or reachability changed.
    pub fn poll(&mut self) -> Vec<usize> {
        let mut changed = Vec::new();
        while let Ok(report) = self.reports.try_recv() {
            let index = report.index;
            if self.apply(report) && !changed.contains(&index) {
                changed.push(index);
            }
        }
        changed
    }

    /// A report's state is taken only if nothing newer has been sent since,
    /// so a re-read never undoes a dial turn still on its way to the light.
    fn apply(&mut self, report: Report) -> bool {
        let Some(light) = self.items.get_mut(report.index) else {
            return false;
        };
        let before = (light.state, light.online);

        light.online = report.state.is_some();
        if let Some(state) = report.state {
            if report.sequence == light.sequence {
                light.state = state;
                light.known = true;
            }
        }

        (light.state, light.online) != before
    }

    pub fn any_on(&self) -> bool {
        self.items.iter().any(|light| light.state.on)
    }

    pub fn set_all(&mut self, on: bool) {
        for light in &mut self.items {
            light.set_on(on);
        }
    }

    /// Same signed delta to every light, so their relative spacing survives.
    pub fn nudge_all_brightness(&mut self, delta: i32) {
        for light in &mut self.items {
            light.nudge_brightness(delta);
        }
    }

    pub fn nudge_all_temperature(&mut self, delta: i32) {
        for light in &mut self.items {
            light.nudge_temperature(delta);
        }
    }

    pub fn mean_brightness(&self) -> i32 {
        let total: i32 = self.items.iter().map(|light| light.state.brightness).sum();
        total / self.items.len().max(1) as i32
    }

    pub fn mean_temperature(&self) -> i32 {
        let total: i32 = self.items.iter().map(|light| light.state.temperature).sum();
        total / self.items.len().max(1) as i32
    }
}

fn agent() -> Agent {
    let config = Agent::config_builder()
        .timeout_global(Some(config::HTTP_TIMEOUT))
        .build();
    Agent::new_with_config(config)
}

fn endpoint(address: &str) -> String {
    format!("http://{}/elgato/lights", address)
}

fn fetch(agent: &Agent, address: &str) -> Option<State> {
    let body: Value = agent.get(&endpoint(address)).call().ok()?.body_mut().read_json().ok()?;
    parse(&body)
}

/// Decode the first light from a GET response body.
fn parse(body: &Value) -> Option<State> {
    let light = body.get("lights")?.get(0)?;

    Some(State {
        on: light.get("on")?.as_i64()? != 0,
        brightness: light.get("brightness")?.as_i64()? as i32,
        temperature: light.get("temperature")?.as_i64()? as i32,
    })
}

fn send(agent: &Agent, address: &str, state: State) -> bool {
    agent.put(&endpoint(address)).send_json(payload(state)).is_ok()
}

/// Encode a state as the PUT body the light expects.
fn payload(state: State) -> Value {
    json!({
        "numberOfLights": 1,
        "lights": [{
            "on": i32::from(state.on),
            "brightness": state.brightness,
            "temperature": state.temperature,
        }]
    })
}

/// One worker per light, owning all its network traffic, so the event loop is
/// never waiting on the network.
fn spawn(index: usize, locator: Locator, reports: Sender<Report>) -> Sender<Change> {
    let (sender, receiver) = channel();

    thread::spawn(move || {
        let agent = agent();
        work(
            &receiver,
            config::RESYNC,
            || locator.address().and_then(|address| fetch(&agent, &address)),
            |state| locator.address().is_some_and(|address| send(&agent, &address, state)),
            |sequence, state| reports.send(Report { index, sequence, state }).is_ok(),
        );
    });

    sender
}

/// The worker loop, apart from the network: read the light first, then write
/// each change once it settles, and re-read whenever things go quiet. Every
/// outcome is reported; stops when either end goes away.
fn work(
    receiver: &Receiver<Change>,
    resync: Duration,
    mut read: impl FnMut() -> Option<State>,
    mut write: impl FnMut(State) -> bool,
    mut report: impl FnMut(u64, Option<State>) -> bool,
) {
    let mut sequence = 0;
    if !report(sequence, read()) {
        return;
    }

    loop {
        let state = match next(receiver, resync) {
            Next::Write(newer, target) => {
                sequence = newer;
                write(target).then_some(target)
            }
            Next::Idle => read(),
            Next::Done => return,
        };
        if !report(sequence, state) {
            return;
        }
    }
}

enum Next {
    /// The newest change queued once the debounce window has passed.
    Write(u64, State),
    /// Nothing asked of the light for a whole resync period.
    Idle,
    /// The Light has gone away.
    Done,
}

/// Wait for a change, let the debounce window pass, and take the newest one
/// queued by then, so a burst of dial detents becomes a single write.
fn next(receiver: &Receiver<Change>, resync: Duration) -> Next {
    let mut latest = match receiver.recv_timeout(resync) {
        Ok(change) => change,
        Err(RecvTimeoutError::Timeout) => return Next::Idle,
        Err(RecvTimeoutError::Disconnected) => return Next::Done,
    };
    thread::sleep(config::DEBOUNCE);

    loop {
        match receiver.try_recv() {
            Ok(newer) => latest = newer,
            Err(TryRecvError::Empty) => return Next::Write(latest.0, latest.1),
            Err(TryRecvError::Disconnected) => return Next::Done,
        }
    }
}

#[cfg(test)]
impl Light {
    /// A light with no worker behind it; what would have been sent to the
    /// network arrives on the returned receiver instead.
    pub fn detached(config: LightConfig, state: State) -> (Self, Receiver<Change>) {
        let (sender, receiver) = channel();
        let light = Light {
            locator: Locator::new(&config, &Directory::default()),
            config,
            state,
            online: true,
            known: true,
            sequence: 0,
            sender,
        };
        (light, receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(brightness: i32, temperature: i32) -> State {
        State { on: true, brightness, temperature }
    }

    fn sample() -> LightConfig {
        LightConfig { name: "Key".into(), id: None, host: Some("192.0.2.1".into()), shape: config::Shape::Panel }
    }

    fn sent(receiver: &Receiver<Change>) -> Vec<State> {
        receiver.try_iter().map(|(_, state)| state).collect()
    }

    fn group(states: &[State]) -> (Lights, Vec<Receiver<Change>>) {
        let (lights, receivers, _reporter) = reporting(states);
        (lights, receivers)
    }

    /// Lights as `connect` makes them, but with the workers' side of the
    /// report channel handed to the test.
    fn reporting(states: &[State]) -> (Lights, Vec<Receiver<Change>>, Sender<Report>) {
        let mut items = Vec::new();
        let mut receivers = Vec::new();
        for state in states {
            let (light, receiver) = Light::detached(sample(), *state);
            items.push(light);
            receivers.push(receiver);
        }
        let (reporter, reports) = channel();
        (Lights { items, reports }, receivers, reporter)
    }

    fn report(index: usize, sequence: u64, state: Option<State>) -> Report {
        Report { index, sequence, state }
    }

    #[test]
    fn toggle_flips_and_sends() {
        let (mut light, receiver) = Light::detached(sample(), State::default());
        light.toggle();
        assert!(light.state.on);
        light.toggle();
        assert!(!light.state.on);

        let states = sent(&receiver);
        assert_eq!(states.len(), 2);
        assert!(states[0].on);
        assert!(!states[1].on);
    }

    #[test]
    fn unchanged_state_is_not_sent() {
        let (mut light, receiver) = Light::detached(sample(), lit(50, 200));
        light.set_on(true);
        light.nudge_brightness(0);
        light.nudge_temperature(0);
        assert!(sent(&receiver).is_empty());
    }

    #[test]
    fn brightness_clamps_at_both_ends() {
        let (mut light, receiver) = Light::detached(sample(), lit(98, 200));
        light.nudge_brightness(10);
        assert_eq!(light.state.brightness, config::BRIGHTNESS_MAX);
        light.nudge_brightness(10);
        assert_eq!(sent(&receiver).len(), 1, "already at the limit, nothing more to send");

        light.nudge_brightness(-500);
        assert_eq!(light.state.brightness, config::BRIGHTNESS_MIN);
    }

    #[test]
    fn temperature_clamps_at_both_ends() {
        let (mut light, _receiver) = Light::detached(sample(), lit(50, 200));
        light.nudge_temperature(1000);
        assert_eq!(light.state.temperature, config::TEMPERATURE_MAX);
        light.nudge_temperature(-1000);
        assert_eq!(light.state.temperature, config::TEMPERATURE_MIN);
    }

    #[test]
    fn nudges_leave_power_alone() {
        let (mut light, receiver) = Light::detached(sample(), State::default());
        light.nudge_brightness(4);
        light.nudge_temperature(4);
        assert!(!light.state.on);
        assert!(sent(&receiver).iter().all(|state| !state.on));
    }

    #[test]
    fn group_summaries() {
        let (lights, _receivers) = group(&[lit(10, 150), State { on: false, brightness: 20, temperature: 250 }, lit(60, 341)]);
        assert!(lights.any_on());
        assert_eq!(lights.mean_brightness(), 30);
        assert_eq!(lights.mean_temperature(), 247);

        let (dark, _receivers) = group(&[State::default(); 3]);
        assert!(!dark.any_on());
    }

    #[test]
    fn empty_group_does_not_divide_by_zero() {
        let (lights, _receivers) = group(&[]);
        assert_eq!(lights.mean_brightness(), 0);
        assert_eq!(lights.mean_temperature(), 0);
        assert!(!lights.any_on());
    }

    #[test]
    fn set_all_only_sends_to_lights_that_change() {
        let (mut lights, receivers) = group(&[lit(10, 200), State::default(), lit(30, 200)]);
        lights.set_all(true);
        assert!(lights.items.iter().all(|light| light.state.on));
        assert_eq!(sent(&receivers[0]).len(), 0);
        assert_eq!(sent(&receivers[1]).len(), 1);
        assert_eq!(sent(&receivers[2]).len(), 0);

        lights.set_all(false);
        assert!(!lights.any_on());
        assert!(receivers.iter().all(|receiver| sent(receiver).len() == 1));
    }

    #[test]
    fn group_nudge_keeps_spacing_until_a_limit() {
        let (mut lights, _receivers) = group(&[lit(10, 200), lit(40, 220), lit(96, 240)]);
        lights.nudge_all_brightness(2);
        let brightness: Vec<i32> = lights.items.iter().map(|light| light.state.brightness).collect();
        assert_eq!(brightness, [12, 42, 98]);

        lights.nudge_all_brightness(4);
        let brightness: Vec<i32> = lights.items.iter().map(|light| light.state.brightness).collect();
        assert_eq!(brightness, [16, 46, 100]);

        lights.nudge_all_temperature(-8);
        let temperature: Vec<i32> = lights.items.iter().map(|light| light.state.temperature).collect();
        assert_eq!(temperature, [192, 212, 232]);
    }

    #[test]
    fn endpoint_targets_the_elgato_api() {
        assert_eq!(endpoint("192.0.2.5:9123"), "http://192.0.2.5:9123/elgato/lights");
    }

    const MAC: &str = "00:00:5E:00:53:01";

    fn configured(id: Option<&str>, host: Option<&str>) -> LightConfig {
        LightConfig { name: "Key".into(), id: id.map(Into::into), host: host.map(Into::into), shape: config::Shape::Panel }
    }

    fn sighting(last: u8) -> crate::discovery::Found {
        crate::discovery::Found {
            id: MAC.into(),
            name: "Elgato Key Light 5301".into(),
            model: String::new(),
            addresses: vec![std::net::SocketAddr::from(([192, 0, 2, last], 9123))],
        }
    }

    #[test]
    fn locator_uses_the_configured_host_until_found() {
        let directory = Directory::default();
        let locator = Locator::new(&configured(Some(MAC), Some("192.0.2.5")), &directory);
        assert_eq!(locator.address().as_deref(), Some("192.0.2.5:9123"));

        directory.record(&sighting(97));
        assert_eq!(locator.address().as_deref(), Some("192.0.2.97:9123"));

        directory.record(&sighting(12));
        assert_eq!(locator.address().as_deref(), Some("192.0.2.12:9123"));
    }

    #[test]
    fn locator_without_a_host_waits_for_discovery() {
        let directory = Directory::default();
        let locator = Locator::new(&configured(Some(MAC), None), &directory);
        assert_eq!(locator.address(), None);
        directory.record(&sighting(97));
        assert_eq!(locator.address().as_deref(), Some("192.0.2.97:9123"));
    }

    #[test]
    fn locator_without_an_id_ignores_discovery() {
        let directory = Directory::default();
        directory.record(&sighting(97));
        let locator = Locator::new(&configured(None, Some("key.local")), &directory);
        assert_eq!(locator.address().as_deref(), Some("key.local:9123"));
    }

    #[test]
    fn parse_reads_the_first_light() {
        let body = json!({
            "numberOfLights": 1,
            "lights": [{ "on": 1, "brightness": 37, "temperature": 213 }]
        });
        assert_eq!(parse(&body), Some(lit(37, 213)));

        let body = json!({ "lights": [{ "on": 0, "brightness": 3, "temperature": 344 }] });
        assert_eq!(parse(&body), Some(State { on: false, brightness: 3, temperature: 344 }));
    }

    #[test]
    fn parse_rejects_malformed_bodies() {
        assert_eq!(parse(&json!({})), None);
        assert_eq!(parse(&json!({ "lights": [] })), None);
        assert_eq!(parse(&json!({ "lights": [{ "on": 1, "brightness": 37 }] })), None);
        assert_eq!(parse(&json!({ "lights": [{ "on": true, "brightness": 37, "temperature": 213 }] })), None);
    }

    #[test]
    fn payload_round_trips_through_parse() {
        for state in [lit(37, 213), State::default(), State { on: false, brightness: 100, temperature: 143 }] {
            let body = payload(state);
            assert_eq!(body["numberOfLights"], 1);
            assert_eq!(parse(&body), Some(state));
        }
        assert_eq!(payload(lit(1, 200))["lights"][0]["on"], 1);
    }

    #[test]
    fn sequence_counts_each_change_sent() {
        let (mut light, receiver) = Light::detached(sample(), lit(50, 200));
        light.nudge_brightness(2);
        light.nudge_brightness(0);
        light.toggle();
        let sequences: Vec<u64> = receiver.try_iter().map(|(sequence, _)| sequence).collect();
        assert_eq!(sequences, [1, 2]);
        assert_eq!(light.sequence, 2);
    }

    #[test]
    fn nothing_is_sent_until_the_light_has_been_read() {
        let (lights, receivers, reporter) = reporting(&[State::default()]);
        let mut lights = lights;
        lights.items[0].known = false;
        lights.items[0].online = false;

        lights.items[0].toggle();
        lights.items[0].nudge_brightness(10);
        assert!(sent(&receivers[0]).is_empty());
        assert_eq!(lights.items[0].state, State::default());

        reporter.send(report(0, 0, Some(lit(37, 213)))).unwrap();
        assert_eq!(lights.poll(), [0]);
        assert_eq!(lights.items[0].state, lit(37, 213));
        assert!(lights.items[0].online());

        lights.items[0].nudge_brightness(2);
        assert_eq!(sent(&receivers[0]), [lit(39, 213)]);
    }

    #[test]
    fn an_unreachable_light_stays_unknown() {
        let (mut lights, receivers, reporter) = reporting(&[State::default()]);
        lights.items[0].known = false;

        reporter.send(report(0, 0, None)).unwrap();
        assert_eq!(lights.poll(), [0]);
        assert!(!lights.items[0].online());

        lights.items[0].toggle();
        assert!(sent(&receivers[0]).is_empty(), "state is still a guess");
    }

    #[test]
    fn a_report_overtaken_by_a_newer_change_is_ignored() {
        let (mut lights, _receivers, reporter) = reporting(&[lit(50, 200)]);
        lights.items[0].nudge_brightness(2);
        lights.items[0].nudge_brightness(2);
        assert_eq!(lights.items[0].sequence, 2);

        // A re-read from before the turns, then the write of the first one.
        reporter.send(report(0, 0, Some(lit(50, 200)))).unwrap();
        reporter.send(report(0, 1, Some(lit(52, 200)))).unwrap();
        assert!(lights.poll().is_empty());
        assert_eq!(lights.items[0].state, lit(54, 200));

        // Once the worker has caught up, what it reads is believed.
        reporter.send(report(0, 2, Some(lit(54, 200)))).unwrap();
        reporter.send(report(0, 2, Some(lit(80, 300)))).unwrap();
        assert_eq!(lights.poll(), [0]);
        assert_eq!(lights.items[0].state, lit(80, 300));
    }

    #[test]
    fn reachability_follows_every_report() {
        let (mut lights, _receivers, reporter) = reporting(&[lit(50, 200), lit(60, 200)]);
        lights.items[0].nudge_brightness(2);

        // A failed write, even an old one, means the light isn't answering.
        reporter.send(report(0, 0, None)).unwrap();
        assert_eq!(lights.poll(), [0]);
        assert!(!lights.items[0].online());
        assert_eq!(lights.items[0].state, lit(52, 200), "local state is kept");

        reporter.send(report(0, 0, Some(lit(50, 200)))).unwrap();
        assert_eq!(lights.poll(), [0]);
        assert!(lights.items[0].online());
        assert_eq!(lights.items[0].state, lit(52, 200));

        reporter.send(report(1, 0, Some(lit(60, 200)))).unwrap();
        assert!(lights.poll().is_empty(), "nothing visible changed");
    }

    #[test]
    fn poll_names_each_changed_light_once() {
        let (mut lights, _receivers, reporter) = reporting(&[lit(10, 200), lit(20, 200), lit(30, 200)]);
        reporter.send(report(2, 0, Some(lit(31, 200)))).unwrap();
        reporter.send(report(0, 0, Some(lit(11, 200)))).unwrap();
        reporter.send(report(2, 0, Some(lit(32, 200)))).unwrap();
        reporter.send(report(7, 0, Some(lit(32, 200)))).unwrap();
        assert_eq!(lights.poll(), [2, 0]);
        assert!(lights.poll().is_empty());
    }

    #[test]
    fn settle_returns_once_every_light_has_reported() {
        let (mut lights, _receivers, reporter) = reporting(&[State::default(), State::default()]);
        let started = Instant::now();
        thread::spawn(move || {
            reporter.send(report(1, 0, None)).unwrap();
            thread::sleep(Duration::from_millis(50));
            reporter.send(report(0, 0, Some(lit(37, 213)))).unwrap();
            thread::sleep(Duration::from_secs(5));
        });

        lights.settle(Duration::from_secs(10));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(lights.items[0].state, lit(37, 213));
        assert!(!lights.items[1].online());
    }

    #[test]
    fn settle_gives_up_at_the_timeout() {
        let (mut lights, _receivers, _reporter) = reporting(&[State::default()]);
        let started = Instant::now();
        lights.settle(Duration::from_millis(100));
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(100) && waited < Duration::from_secs(2));
    }

    #[test]
    fn next_collapses_a_burst() {
        let (sender, receiver) = channel();
        for (sequence, brightness) in [(1, 10), (2, 12), (3, 14), (4, 16)] {
            sender.send((sequence, lit(brightness, 200))).unwrap();
        }
        assert!(matches!(next(&receiver, Duration::from_secs(10)), Next::Write(4, state) if state == lit(16, 200)));

        sender.send((5, lit(18, 200))).unwrap();
        assert!(matches!(next(&receiver, Duration::from_secs(10)), Next::Write(5, state) if state == lit(18, 200)));
    }

    #[test]
    fn next_waits_out_the_debounce() {
        let (sender, receiver) = channel();
        sender.send((1, lit(10, 200))).unwrap();

        let late = thread::spawn(move || {
            thread::sleep(config::DEBOUNCE / 4);
            sender.send((2, lit(20, 200))).unwrap();
            sender
        });

        assert!(matches!(next(&receiver, Duration::from_secs(10)), Next::Write(2, _)));
        drop(late.join().unwrap());
    }

    #[test]
    fn next_goes_idle_when_nothing_arrives() {
        let (_sender, receiver) = channel();
        let started = Instant::now();
        assert!(matches!(next(&receiver, Duration::from_millis(50)), Next::Idle));
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn next_ends_when_the_light_is_dropped() {
        let (sender, receiver) = channel::<Change>();
        drop(sender);
        assert!(matches!(next(&receiver, Duration::from_secs(10)), Next::Done));
    }

    /// Runs `work` with stand-ins for the network until it has made `limit`
    /// reports, returning those and the writes it attempted.
    fn run(
        receiver: &Receiver<Change>,
        resync: Duration,
        reads: &[Option<State>],
        writes_succeed: bool,
        limit: usize,
    ) -> (Vec<(u64, Option<State>)>, Vec<State>) {
        let mut reads = reads.iter().copied();
        let mut written = Vec::new();
        let mut reported = Vec::new();
        work(
            receiver,
            resync,
            || reads.next().flatten(),
            |state| {
                written.push(state);
                writes_succeed
            },
            |sequence, state| {
                reported.push((sequence, state));
                reported.len() < limit
            },
        );
        (reported, written)
    }

    #[test]
    fn work_reads_first_then_writes_the_settled_change() {
        let (sender, receiver) = channel();
        for (sequence, brightness) in [(1, 10), (2, 12), (3, 14)] {
            sender.send((sequence, lit(brightness, 200))).unwrap();
        }

        let (reported, written) = run(&receiver, Duration::from_secs(10), &[Some(lit(37, 213))], true, 2);
        assert_eq!(reported, [(0, Some(lit(37, 213))), (3, Some(lit(14, 200)))]);
        assert_eq!(written, [lit(14, 200)]);
    }

    #[test]
    fn work_reports_a_failed_write_as_unreachable() {
        let (sender, receiver) = channel();
        sender.send((1, lit(10, 200))).unwrap();

        let (reported, _) = run(&receiver, Duration::from_secs(10), &[Some(lit(37, 213))], false, 2);
        assert_eq!(reported, [(0, Some(lit(37, 213))), (1, None)]);
    }

    #[test]
    fn work_rereads_when_idle_under_the_latest_sequence() {
        let (sender, receiver) = channel();
        sender.send((4, lit(10, 200))).unwrap();

        let reads = [Some(lit(37, 213)), None, Some(lit(90, 300))];
        let (reported, _) = run(&receiver, Duration::from_millis(20), &reads, true, 4);
        assert_eq!(
            reported,
            [(0, Some(lit(37, 213))), (4, Some(lit(10, 200))), (4, None), (4, Some(lit(90, 300)))]
        );
    }

    #[test]
    fn work_stops_when_the_light_is_dropped() {
        let (sender, receiver) = channel::<Change>();
        drop(sender);
        let (reported, written) = run(&receiver, Duration::from_secs(10), &[None], true, 100);
        assert_eq!(reported, [(0, None)]);
        assert!(written.is_empty());
    }

    #[test]
    fn work_stops_when_nobody_is_listening() {
        let (_sender, receiver) = channel::<Change>();
        let (reported, _) = run(&receiver, Duration::from_millis(10), &[None; 10], true, 1);
        assert_eq!(reported.len(), 1);
    }
}
