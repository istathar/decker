mod config;
mod lights;
mod media;
mod render;

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use clap::builder::{OsStr, Resettable};
use clap::{Arg, ArgAction, Command};
use elgato_streamdeck::images::ImageRect;
use elgato_streamdeck::info::Kind;
use elgato_streamdeck::{list_devices, new_hidapi, DeviceStateUpdate, StreamDeck};
use image::DynamicImage;

use config::{LightConfig, BRIGHTNESS_STEP, TEMPERATURE_STEP};
use lights::Lights;
use media::{Action, Media};
use render::Renderer;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dial {
    Brightness,
    Temperature,
}

impl Dial {
    fn flip(self) -> Self {
        match self {
            Dial::Brightness => Dial::Temperature,
            Dial::Temperature => Dial::Brightness,
        }
    }
}

fn main() {
    // Shown by --help and used when --config isn't given; none without $HOME.
    let default_config: Resettable<OsStr> = config::default_path()
        .map(|path| OsStr::from(path.display().to_string()))
        .into();

    let matches = Command::new("decker")
        .version(VERSION)
        .author("Andrew Cowie")
        .about("Control Elgato Key Lights and GNOME media playback using a Stream Deck Plus.")
        .arg(
            Arg::new("list")
                .long("list")
                .action(ArgAction::SetTrue)
                .help("List attached Stream Deck devices and exit"),
        )
        .arg(
            Arg::new("brightness")
                .long("brightness")
                .short('b')
                .value_name("PERCENT")
                .value_parser(clap::value_parser!(u8).range(5..=100))
                .default_value("45")
                .help("Panel brightness of the Stream Deck itself"),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .short('c')
                .value_name("FILE")
                .default_value(default_config)
                .help("Lights to control"),
        )
        .arg(
            Arg::new("verbose")
                .long("verbose")
                .short('v')
                .action(ArgAction::SetTrue)
                .help("Print device events as they arrive"),
        )
        .get_matches();

    let listing = *matches.get_one::<bool>("list").unwrap();
    let brightness = *matches.get_one::<u8>("brightness").unwrap();
    let verbose = *matches.get_one::<bool>("verbose").unwrap();
    let config = matches.get_one::<String>("config").map(PathBuf::from);

    let result = if listing {
        list()
    } else {
        run(config, brightness, verbose)
    };

    if let Err(problem) = result {
        eprintln!("decker: {}", problem);
        std::process::exit(1);
    }
}

fn list() -> Result<(), Box<dyn Error>> {
    let hidapi = new_hidapi()?;
    let devices = list_devices(&hidapi);

    if devices.is_empty() {
        println!("No Stream Deck devices found.");
        println!("If StreamController or another similar application is running, it has the device claimed. You'll need to shut it down before trying this program.");
        return Ok(());
    }

    for (kind, serial) in devices {
        println!("{:?}  serial {}  keys {}  dials {}", kind, serial, kind.key_count(), kind.encoder_count());
    }

    Ok(())
}

/// Connect, and wait for each light's first answer, all of them in parallel.
fn find(configs: Vec<LightConfig>) -> Lights {
    let mut lights = Lights::connect(configs);
    lights.settle(config::HTTP_TIMEOUT + Duration::from_millis(500));
    lights
}

fn configured(config: Option<PathBuf>) -> Result<Vec<LightConfig>, Box<dyn Error>> {
    let path = config
        .or_else(config::default_path)
        .ok_or("HOME is not set; give the lights file with --config")?;
    config::load(&path)
}

fn run(config: Option<PathBuf>, brightness: u8, verbose: bool) -> Result<(), Box<dyn Error>> {
    let configs = configured(config)?;

    let hidapi = new_hidapi()?;
    let (kind, serial) = list_devices(&hidapi)
        .into_iter()
        .find(|(kind, _)| matches!(kind, Kind::Plus | Kind::PlusXl))
        .ok_or("No Stream Deck Plus found. Is another controller application still running?)")?;

    let deck = StreamDeck::connect(&hidapi, kind, &serial)?;
    deck.reset()?;
    deck.set_brightness(brightness)?;

    let renderer = Renderer::new()?;
    let mut lights = find(configs);
    let media = Media::start();
    let mut modes = [Dial::Brightness; 4];

    println!("decker: {:?} serial {}, firmware {}", kind, serial, deck.firmware_version()?);
    for light in &lights.items {
        println!(
            "  {:<5} {:<21} {}",
            light.name(),
            light.address(),
            if light.online() { "ok" } else { "unreachable" }
        );
    }

    let mut dirty = Dirty::all();
    let mut generation = media.generation();

    let deck = std::sync::Arc::new(deck);
    let reader = deck.get_reader();

    loop {
        for update in reader.read(Some(Duration::from_millis(100)))? {
            if verbose {
                println!("{:?}", update);
            }

            match update {
                DeviceStateUpdate::ButtonDown(key) => match key {
                    0 => media.send(Action::Previous),
                    1 => media.send(Action::PlayPause),
                    2 => media.send(Action::Next),
                    3 => media.send(Action::ToggleMute),
                    4..=6 => {
                        let index = (key - 4) as usize;
                        if let Some(light) = lights.items.get_mut(index) {
                            light.toggle();
                            dirty.keys[key as usize] = true;
                            dirty.keys[7] = true;
                            dirty.zones[index] = true;
                            dirty.zones[3] = true;
                        }
                    }
                    7 => {
                        let wanted = !lights.any_on();
                        lights.set_all(wanted);
                        dirty = Dirty::all();
                    }
                    _ => {}
                },

                DeviceStateUpdate::EncoderTwist(dial, delta) => {
                    let index = dial as usize;
                    let delta = delta as i32;

                    if index < 3 {
                        if let Some(light) = lights.items.get_mut(index) {
                            turn(light, modes[index], delta);
                            dirty.keys[index + 4] = true;
                            dirty.zones[index] = true;
                            dirty.zones[3] = true;
                        }
                    } else if index == 3 {
                        match modes[3] {
                            Dial::Brightness => lights.nudge_all_brightness(delta * BRIGHTNESS_STEP),
                            Dial::Temperature => lights.nudge_all_temperature(-delta * TEMPERATURE_STEP),
                        }
                        dirty = Dirty::all();
                    }
                }

                DeviceStateUpdate::EncoderDown(dial) => {
                    let index = dial as usize;
                    if index == 3 {
                        modes = [modes[3].flip(); 4];
                        dirty.zones = [true; 4];
                    } else if index < 3 {
                        modes[index] = modes[index].flip();
                        dirty.zones[index] = true;
                    }
                }

                _ => {}
            }
        }

        let latest = media.generation();
        if latest != generation {
            generation = latest;
            dirty.keys[1] = true;
            dirty.keys[3] = true;
        }

        for index in lights.poll() {
            dirty.keys[index + 4] = true;
            dirty.keys[7] = true;
            dirty.zones[index] = true;
            dirty.zones[3] = true;
        }

        paint(&deck, &renderer, &lights, &media, &modes, &mut dirty)?;
    }
}

fn turn(light: &mut lights::Light, mode: Dial, delta: i32) {
    match mode {
        Dial::Brightness => light.nudge_brightness(delta * BRIGHTNESS_STEP),
        // Clockwise should read as more Kelvin, which is fewer mireds.
        Dial::Temperature => light.nudge_temperature(-delta * TEMPERATURE_STEP),
    }
}

fn paint(
    deck: &StreamDeck,
    renderer: &Renderer,
    lights: &Lights,
    media: &Media,
    modes: &[Dial; 4],
    dirty: &mut Dirty,
) -> Result<(), Box<dyn Error>> {
    let status = media.status();
    let mut flush = false;

    for key in 0..8usize {
        if !dirty.keys[key] {
            continue;
        }
        dirty.keys[key] = false;
        flush = true;

        let image = key_image(renderer, lights, &status, key);
        deck.set_button_image(key as u8, image)?;
    }

    if flush {
        deck.flush()?;
    }

    for zone in 0..4usize {
        if !dirty.zones[zone] {
            continue;
        }
        dirty.zones[zone] = false;

        let image = zone_image(renderer, lights, modes, zone);
        let x = (zone as u32 * render::ZONE_WIDTH) as u16;
        deck.write_lcd(x, 0, &ImageRect::from_image(image)?)?;
    }

    Ok(())
}

/// Which keys and LCD zones need drawing again.
struct Dirty {
    keys: [bool; 8],
    zones: [bool; 4],
}

impl Dirty {
    fn all() -> Self {
        Dirty { keys: [true; 8], zones: [true; 4] }
    }
}

fn key_image(renderer: &Renderer, lights: &Lights, status: &media::Status, key: usize) -> DynamicImage {
    match key {
        0..=3 => renderer.media_key(key as u8, status),
        4..=6 => match lights.items.get(key - 4) {
            Some(light) => renderer.light_key(light.name(), light.config.shape, &light.state, light.online()),
            None => renderer.empty_key(),
        },
        _ => renderer.all_key(lights.any_on()),
    }
}

fn zone_image(renderer: &Renderer, lights: &Lights, modes: &[Dial; 4], zone: usize) -> DynamicImage {
    if zone == 3 {
        let summary = lights::State {
            on: lights.any_on(),
            brightness: lights.mean_brightness(),
            temperature: lights.mean_temperature(),
        };
        return renderer.zone("ALL", modes[3], summary, true);
    }

    match lights.items.get(zone) {
        Some(light) => renderer.zone(light.name(), modes[zone], light.state, light.online()),
        None => renderer.empty_zone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lights::{Light, State};

    fn light() -> Light {
        let state = State { on: true, brightness: 50, temperature: 200 };
        let config = LightConfig { name: "Key".into(), host: "192.0.2.1".into(), shape: config::Shape::Panel };
        Light::detached(config, state).0
    }

    #[test]
    fn flip_alternates() {
        assert!(Dial::Brightness.flip() == Dial::Temperature);
        assert!(Dial::Temperature.flip() == Dial::Brightness);
    }

    #[test]
    fn clockwise_is_brighter() {
        let mut light = light();
        turn(&mut light, Dial::Brightness, 3);
        assert_eq!(light.state.brightness, 50 + 3 * BRIGHTNESS_STEP);
        assert_eq!(light.state.temperature, 200);
        turn(&mut light, Dial::Brightness, -5);
        assert_eq!(light.state.brightness, 50 - 2 * BRIGHTNESS_STEP);
    }

    #[test]
    fn clockwise_is_cooler() {
        let mut light = light();
        let before = config::kelvin(light.state.temperature);
        turn(&mut light, Dial::Temperature, 2);
        assert_eq!(light.state.temperature, 200 - 2 * TEMPERATURE_STEP);
        assert!(config::kelvin(light.state.temperature) > before);
        assert_eq!(light.state.brightness, 50);
    }
}
