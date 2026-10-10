use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightConfig {
    pub name: String,
    /// MAC address, by which the light is found over mDNS.
    pub id: Option<String>,
    /// Fixed address, used when there is no id or the light hasn't been found by it.
    pub host: Option<String>,
    pub shape: Shape,
}

/// The two Key Light body styles, drawn on the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Key Light and MK.2: wide, with strongly rounded ends.
    Panel,
    /// Key Light Air: nearly square.
    Air,
}

/// Lights go on keys 4-6 and dials 0-2.
pub const MAX_LIGHTS: usize = 3;

/// Presets fill all eight keys when the picker is up.
pub const MAX_PRESETS: usize = 8;

/// Everything in lights.conf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub lights: Vec<LightConfig>,
    pub presets: Vec<Preset>,
}

/// A named arrangement of the lights, recalled with one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    /// By light name. A light not listed is left as it is.
    pub settings: Vec<(String, Setting)>,
    /// Fills its key in the picker, if given.
    pub colour: Option<[u8; 3]>,
}

impl Preset {
    pub fn setting(&self, light: &str) -> Option<Setting> {
        self.settings.iter().find(|(name, _)| name == light).map(|(_, setting)| *setting)
    }
}

/// What a preset does to one light. Brightness or temperature left as None
/// keeps whatever the light has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub on: bool,
    pub brightness: Option<i32>,
    pub temperature: Option<i32>,
}

/// Every Key Light serves its control API on this port.
pub const PORT: u16 = 9123;

pub const BRIGHTNESS_MIN: i32 = 0;
pub const BRIGHTNESS_MAX: i32 = 100;

// Elgato temperatures are mireds; Kelvin is 1000000/mired.
pub const TEMPERATURE_MIN: i32 = 143;
pub const TEMPERATURE_MAX: i32 = 344;

pub const BRIGHTNESS_STEP: i32 = 2;
pub const TEMPERATURE_STEP: i32 = 4;

/// Window a burst of dial detents is collapsed into before a light is written to.
pub const DEBOUNCE: Duration = Duration::from_millis(40);

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the deck stays illuminate after every light is off and no buttons
/// have been touched.
pub const IDLE: Duration = Duration::from_secs(600);

/// How long to wait at startup for lights to be found over mDNS.
pub const DISCOVERY: Duration = Duration::from_secs(3);

/// How long a light goes without being written to before its state is
/// re-read, in case something else changed it.
pub const RESYNC: Duration = Duration::from_secs(30);

pub fn kelvin(mired: i32) -> i32 {
    let mired = mired.clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
    (1_000_000 / mired / 50) * 50
}

/// $XDG_CONFIG_HOME/decker/lights.conf, falling back to ~/.config.
pub fn default_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(directory) if !directory.is_empty() => PathBuf::from(directory),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("decker").join("lights.conf"))
}

pub fn load(path: &Path) -> Result<Config, Box<dyn Error>> {
    let text = std::fs::read_to_string(path)
        .map_err(|problem| format!("cannot read {}: {}", path.display(), problem))?;
    let config = parse(&text).map_err(|problem| format!("{}: {}", path.display(), problem))?;
    Ok(config)
}

/// One `[Name]` section per light, left to right:
///
/// ```text
/// [Key]
/// id = 00:00:5E:00:53:01
/// host = 192.0.2.20
/// shape = panel
/// ```
///
/// and up to eight `[preset Name]` sections, each setting lights by name:
///
/// ```text
/// [preset Soft]
/// Key = 14% 344
/// Air = on 3200K
/// MK.2 = off
/// ```
///
/// Blank lines and lines starting with `#` or `;` are ignored.
pub fn parse(text: &str) -> Result<Config, String> {
    let mut sections: Vec<Section> = Vec::new();
    let mut presets: Vec<PresetSection> = Vec::new();
    let mut in_preset = false;

    for (number, line) in text.lines().enumerate() {
        let number = number + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        if let Some(rest) = line.strip_prefix('[') {
            let name = rest
                .strip_suffix(']')
                .ok_or_else(|| format!("line {}: section header is missing its ]", number))?
                .trim();
            let preset = name
                .strip_prefix("preset")
                .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
            if let Some(preset) = preset {
                let preset = preset.trim();
                if preset.is_empty() {
                    return Err(format!("line {}: preset has no name", number));
                }
                if presets.iter().any(|section| section.name == preset) {
                    return Err(format!("line {}: [preset {}] appears twice", number, preset));
                }
                presets.push(PresetSection { name: preset.to_string(), line: number, settings: Vec::new(), colour: None });
                in_preset = true;
                continue;
            }

            if name.is_empty() {
                return Err(format!("line {}: light has no name", number));
            }
            if sections.iter().any(|section| section.name == name) {
                return Err(format!("line {}: [{}] appears twice", number, name));
            }
            sections.push(Section { name: name.to_string(), line: number, id: None, host: None, shape: None });
            in_preset = false;
            continue;
        }

        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected setting = value", number))?;
        let (key, value) = (key.trim(), value.trim());

        if value.is_empty() {
            return Err(format!("line {}: {} has no value", number, key));
        }

        if in_preset {
            let preset = presets.last_mut().expect("in_preset follows a preset header");
            if key == "colour" || key == "color" {
                let rgb = colour(value).ok_or_else(|| {
                    format!("line {}: colour is one of {} or #rrggbb, not {}", number, PALETTE_NAMES, value)
                })?;
                set(&mut preset.colour, rgb, number, key)?;
                continue;
            }
            if preset.settings.iter().any(|(light, _, _)| light == key) {
                return Err(format!("line {}: {} given twice", number, key));
            }
            let setting = setting(value).map_err(|problem| format!("line {}: {}: {}", number, key, problem))?;
            preset.settings.push((key.to_string(), setting, number));
            continue;
        }

        let section = sections
            .last_mut()
            .ok_or_else(|| format!("line {}: {} comes before any [light] section", number, key))?;

        match key {
            "id" => {
                let id = mac(value)
                    .ok_or_else(|| format!("line {}: id is a MAC address like 00:00:5E:00:53:01, not {}", number, value))?;
                set(&mut section.id, id, number, key)?
            }
            "host" => set(&mut section.host, value.to_string(), number, key)?,
            "shape" => {
                let shape = match value.to_lowercase().as_str() {
                    "panel" => Shape::Panel,
                    "air" => Shape::Air,
                    _ => return Err(format!("line {}: shape is panel or air, not {}", number, value)),
                };
                set(&mut section.shape, shape, number, key)?
            }
            _ => return Err(format!("line {}: unknown setting {}", number, key)),
        }
    }

    if sections.is_empty() {
        return Err("no lights configured".into());
    }
    if sections.len() > MAX_LIGHTS {
        return Err(format!("{} lights configured, but there is only room for {}", sections.len(), MAX_LIGHTS));
    }
    if presets.len() > MAX_PRESETS {
        return Err(format!("{} presets configured, but there is only room for {}", presets.len(), MAX_PRESETS));
    }

    let mut finished = Vec::new();
    for preset in presets {
        if preset.settings.is_empty() {
            return Err(format!("line {}: [preset {}] sets no lights", preset.line, preset.name));
        }
        for (light, _, line) in &preset.settings {
            if !sections.iter().any(|section| &section.name == light) {
                return Err(format!("line {}: {} is not one of the lights", line, light));
            }
        }
        finished.push(Preset {
            name: preset.name,
            settings: preset.settings.into_iter().map(|(light, setting, _)| (light, setting)).collect(),
            colour: preset.colour,
        });
    }

    let lights = sections.into_iter().map(Section::finish).collect::<Result<_, _>>()?;
    Ok(Config { lights, presets: finished })
}

struct PresetSection {
    name: String,
    line: usize,
    settings: Vec<(String, Setting, usize)>,
    colour: Option<[u8; 3]>,
}

/// Background colours for preset keys: dark enough that the white name stays
/// readable and a glowing bulb stands out.
const PALETTE: [(&str, [u8; 3]); 8] = [
    ("blue", [0x3a, 0x67, 0xa1]),
    ("green", [0x3d, 0x8b, 0x5a]),
    ("purple", [0x75, 0x4f, 0x9f]),
    ("grey", [0x3c, 0x3f, 0x47]),
    ("brown", [0x8b, 0x67, 0x46]),
    ("red", [0x9f, 0x42, 0x42]),
    ("orange", [0xb3, 0x6e, 0x2c]),
    ("teal", [0x2f, 0x8b, 0x8b]),
];

const PALETTE_NAMES: &str = "blue, green, purple, grey, brown, red, orange, teal";

/// A palette name (gray as well as grey), or #rrggbb.
fn colour(value: &str) -> Option<[u8; 3]> {
    let value = value.to_lowercase();
    let value = if value == "gray" { "grey".to_string() } else { value };
    if let Some((_, rgb)) = PALETTE.iter().find(|(name, _)| *name == value) {
        return Some(*rgb);
    }

    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// `off`, or `on` and/or a brightness like `40%` and a temperature, either in
/// mireds like the Elgato API (`213`) or in Kelvin (`4700K`).
fn setting(value: &str) -> Result<Setting, String> {
    let words: Vec<&str> = value.split_whitespace().collect();
    if words.contains(&"off") {
        if words.len() > 1 {
            return Err("off takes nothing else".into());
        }
        return Ok(Setting { on: false, brightness: None, temperature: None });
    }

    let mut setting = Setting { on: true, brightness: None, temperature: None };
    for word in words {
        if word == "on" {
            continue;
        }

        if let Some(number) = word.strip_suffix('%') {
            let brightness: i32 = number.parse().map_err(|_| format!("{} is not a brightness", word))?;
            if !(BRIGHTNESS_MIN..=BRIGHTNESS_MAX).contains(&brightness) {
                return Err(format!("brightness {} is outside 0-100%", word));
            }
            if setting.brightness.replace(brightness).is_some() {
                return Err("brightness given twice".into());
            }
        } else if let Some(number) = word.strip_suffix(['K', 'k']) {
            let kelvin: i32 = number.parse().map_err(|_| format!("{} is not a temperature", word))?;
            if !(2900..=7000).contains(&kelvin) {
                return Err(format!("temperature {} is outside 2900K-7000K", word));
            }
            let mired = (1_000_000.0 / kelvin as f64).round() as i32;
            let mired = mired.clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
            if setting.temperature.replace(mired).is_some() {
                return Err("temperature given twice".into());
            }
        } else {
            let mired: i32 = word
                .parse()
                .map_err(|_| format!("expected off, on, a brightness like 40%, or a temperature like 213 or 4700K, not {}", word))?;
            if !(TEMPERATURE_MIN..=TEMPERATURE_MAX).contains(&mired) {
                return Err(format!("temperature {} is outside {}-{} mireds", word, TEMPERATURE_MIN, TEMPERATURE_MAX));
            }
            if setting.temperature.replace(mired).is_some() {
                return Err("temperature given twice".into());
            }
        }
    }

    Ok(setting)
}

struct Section {
    name: String,
    line: usize,
    id: Option<String>,
    host: Option<String>,
    shape: Option<Shape>,
}

impl Section {
    fn finish(self) -> Result<LightConfig, String> {
        if self.id.is_none() && self.host.is_none() {
            return Err(format!("line {}: [{}] needs an id or a host", self.line, self.name));
        }

        Ok(LightConfig {
            name: self.name,
            id: self.id,
            host: self.host,
            shape: self.shape.unwrap_or(Shape::Panel),
        })
    }
}

/// A MAC address in canonical form, upper case and colon separated. Colons
/// or hyphens are accepted between the octets.
pub fn mac(text: &str) -> Option<String> {
    let octets: Vec<&str> = text.trim().split([':', '-']).collect();
    let valid = octets.len() == 6
        && octets.iter().all(|octet| octet.len() == 2 && octet.chars().all(|c| c.is_ascii_hexdigit()));
    valid.then(|| octets.join(":").to_uppercase())
}

fn set<T>(slot: &mut Option<T>, value: T, number: usize, key: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("line {}: {} given twice", number, key));
    }
    *slot = Some(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kelvin_converts_and_rounds_down_to_fifty() {
        assert_eq!(kelvin(200), 5000);
        assert_eq!(kelvin(250), 4000);
        assert_eq!(kelvin(213), 4650);
    }

    #[test]
    fn kelvin_spans_the_elgato_range() {
        assert_eq!(kelvin(TEMPERATURE_MIN), 6950);
        assert_eq!(kelvin(TEMPERATURE_MAX), 2900);
    }

    #[test]
    fn kelvin_clamps_out_of_range_mireds() {
        assert_eq!(kelvin(0), kelvin(TEMPERATURE_MIN));
        assert_eq!(kelvin(-5), kelvin(TEMPERATURE_MIN));
        assert_eq!(kelvin(10_000), kelvin(TEMPERATURE_MAX));
    }

    #[test]
    fn fewer_mireds_is_more_kelvin() {
        let mut previous = i32::MAX;
        for mired in TEMPERATURE_MIN..=TEMPERATURE_MAX {
            let k = kelvin(mired);
            assert!(k <= previous);
            previous = k;
        }
    }

    fn light(name: &str, id: Option<&str>, host: Option<&str>, shape: Shape) -> LightConfig {
        LightConfig { name: name.into(), id: id.map(Into::into), host: host.map(Into::into), shape }
    }

    fn fails(text: &str, expected: &str) {
        let problem = parse(text).unwrap_err();
        assert!(problem.contains(expected), "{:?} does not mention {:?}", problem, expected);
    }

    #[test]
    fn parse_reads_lights_in_order() {
        let text = "
            # Left to right.
            [MK.2]
            host = 192.0.2.93

            ; the one by the window
            [Air]
            shape = air
            host = 192.0.2.81
        ";
        assert_eq!(
            parse(text).unwrap().lights,
            [
                light("MK.2", None, Some("192.0.2.93"), Shape::Panel),
                light("Air", None, Some("192.0.2.81"), Shape::Air),
            ]
        );
    }

    #[test]
    fn parse_tolerates_spacing_and_case() {
        let text = "[ Key ]\n  host=key.local  \nshape = PANEL\n";
        assert_eq!(parse(text).unwrap().lights, [light("Key", None, Some("key.local"), Shape::Panel)]);
    }

    #[test]
    fn parse_takes_an_id_with_or_without_a_host() {
        let text = "[Key]\nid = 00-00-5e-00-53-01\n[Air]\nid = 00:00:5E:00:53:02\nhost = 192.0.2.5\nshape = air\n";
        assert_eq!(
            parse(text).unwrap().lights,
            [
                light("Key", Some("00:00:5E:00:53:01"), None, Shape::Panel),
                light("Air", Some("00:00:5E:00:53:02"), Some("192.0.2.5"), Shape::Air),
            ]
        );
    }

    #[test]
    fn mac_normalises_and_validates() {
        assert_eq!(mac("00:00:5e:00:53:01").as_deref(), Some("00:00:5E:00:53:01"));
        assert_eq!(mac(" 00-00-5E-00-53-01 ").as_deref(), Some("00:00:5E:00:53:01"));
        assert_eq!(mac("00:00:5E:00:53"), None);
        assert_eq!(mac("00:00:5E:00:53:01:00"), None);
        assert_eq!(mac("00:00:5E:00:53:0G"), None);
        assert_eq!(mac("00005E005301"), None);
        assert_eq!(mac(""), None);
    }

    #[test]
    fn parse_requires_one_to_three_lights() {
        fails("", "no lights");
        fails("# nothing here\n", "no lights");
        let four: String = (1..=4).map(|n| format!("[L{}]\nhost = h{}\n", n, n)).collect();
        fails(&four, "only room for 3");
        let three: String = (1..=3).map(|n| format!("[L{}]\nhost = h{}\n", n, n)).collect();
        assert_eq!(parse(&three).unwrap().lights.len(), 3);
    }

    #[test]
    fn parse_rejects_mistakes_with_line_numbers() {
        fails("[Key]\n", "line 1: [Key] needs an id or a host");
        fails("[Key]\nid = 3C:6A\n", "line 2: id is a MAC address");
        fails("host = a\n", "line 1: host comes before any [light] section");
        fails("[Key\nhost = a\n", "line 1: section header");
        fails("[]\nhost = a\n", "line 1: light has no name");
        fails("[Key]\nhost = a\n[Key]\nhost = b\n", "line 3: [Key] appears twice");
        fails("[Key]\nhost a\n", "line 2: expected setting = value");
        fails("[Key]\nhost =\n", "line 2: host has no value");
        fails("[Key]\nhost = a\nhost = b\n", "line 3: host given twice");
        fails("[Key]\nhost = a\nshape = round\n", "line 3: shape is panel or air");
        fails("[Key]\nhost = a\ncolour = red\n", "line 3: unknown setting colour");
    }

    #[test]
    fn load_names_the_file_in_errors() {
        let problem = load(Path::new("/nonexistent/lights.conf")).unwrap_err().to_string();
        assert!(problem.contains("/nonexistent/lights.conf"), "{}", problem);
    }

    #[test]
    fn example_file_parses() {
        let text = include_str!("../lights.conf.example");
        let config = parse(text).unwrap();
        assert!(!config.lights.is_empty());
        assert!(!config.presets.is_empty());
    }

    const LIGHTS: &str = "[MK.2]\nhost = a\n[Key]\nhost = b\n[Air]\nhost = c\n";

    fn on(brightness: Option<i32>, temperature: Option<i32>) -> Setting {
        Setting { on: true, brightness, temperature }
    }

    const OFF: Setting = Setting { on: false, brightness: None, temperature: None };

    #[test]
    fn parse_reads_presets_in_order() {
        let text = format!(
            "{}[preset Full]\nAir = 57% 192\nKey = 57% 192\nMK.2 = 20% 229\n\n[preset Soft]\nAir = 14% 344\nMK.2 = off\n",
            LIGHTS
        );
        let config = parse(&text).unwrap();
        assert_eq!(config.lights.len(), 3);
        assert_eq!(
            config.presets,
            [
                Preset {
                    name: "Full".into(),
                    settings: vec![
                        ("Air".into(), on(Some(57), Some(192))),
                        ("Key".into(), on(Some(57), Some(192))),
                        ("MK.2".into(), on(Some(20), Some(229))),
                    ],
                    colour: None,
                },
                Preset {
                    name: "Soft".into(),
                    settings: vec![("Air".into(), on(Some(14), Some(344))), ("MK.2".into(), OFF)],
                    colour: None,
                },
            ]
        );
        assert_eq!(config.presets[1].setting("Key"), None);
        assert_eq!(config.presets[1].setting("MK.2"), Some(OFF));
    }

    #[test]
    fn presets_may_come_before_the_lights() {
        let text = format!("[preset Dim]\nKey = 5%\n{}", LIGHTS);
        let config = parse(&text).unwrap();
        assert_eq!(config.presets[0].setting("Key"), Some(on(Some(5), None)));
        assert_eq!(config.lights.len(), 3);
    }

    #[test]
    fn setting_forms() {
        assert_eq!(setting("off"), Ok(OFF));
        assert_eq!(setting("on"), Ok(on(None, None)));
        assert_eq!(setting("40%"), Ok(on(Some(40), None)));
        assert_eq!(setting("213"), Ok(on(None, Some(213))));
        assert_eq!(setting("on 40% 213"), Ok(on(Some(40), Some(213))));
        assert_eq!(setting("213   40%"), Ok(on(Some(40), Some(213))));
        assert_eq!(setting("0%"), Ok(on(Some(0), None)));
    }

    #[test]
    fn setting_takes_kelvin() {
        assert_eq!(setting("5000K"), Ok(on(None, Some(200))));
        assert_eq!(setting("4700k"), Ok(on(None, Some(213))));
        // The ends of the range round just outside it, and are pulled back in.
        assert_eq!(setting("2900K"), Ok(on(None, Some(TEMPERATURE_MAX))));
        assert_eq!(setting("7000K"), Ok(on(None, Some(TEMPERATURE_MIN))));
    }

    #[test]
    fn setting_rejects_mistakes() {
        for (value, expected) in [
            ("off 20%", "off takes nothing else"),
            ("101%", "outside 0-100%"),
            ("-1%", "outside 0-100%"),
            ("x%", "not a brightness"),
            ("2000K", "outside 2900K-7000K"),
            ("8000K", "outside 2900K-7000K"),
            ("warmK", "not a temperature"),
            ("3100", "outside 143-344 mireds"),
            ("100", "outside 143-344 mireds"),
            ("bright", "expected off, on"),
            ("20% 30%", "brightness given twice"),
            ("200 5000K", "temperature given twice"),
        ] {
            let problem = setting(value).unwrap_err();
            assert!(problem.contains(expected), "{:?} gave {:?}", value, problem);
        }
    }

    #[test]
    fn parse_rejects_preset_mistakes() {
        fails(&format!("{}[preset ]\n", LIGHTS), "line 7: preset has no name");
        fails(&format!("{}[preset A]\nKey = on\n[preset A]\nKey = off\n", LIGHTS), "line 9: [preset A] appears twice");
        fails(&format!("{}[preset A]\n", LIGHTS), "line 7: [preset A] sets no lights");
        fails(&format!("{}[preset A]\nLamp = on\n", LIGHTS), "line 8: Lamp is not one of the lights");
        fails(&format!("{}[preset A]\nKey = on\nKey = off\n", LIGHTS), "line 9: Key given twice");
        fails(&format!("{}[preset A]\nKey = 120%\n", LIGHTS), "line 8: Key: brightness 120% is outside");
        fails(&format!("{}[preset A]\nKey\n", LIGHTS), "line 8: expected setting = value");
        fails(&format!("{}[preset A]\nKey =\n", LIGHTS), "line 8: Key has no value");

        let nine: String = (1..=9).map(|n| format!("[preset P{}]\nKey = on\n", n)).collect();
        fails(&format!("{}{}", LIGHTS, nine), "9 presets configured, but there is only room for 8");
        let eight: String = (1..=8).map(|n| format!("[preset P{}]\nKey = on\n", n)).collect();
        assert_eq!(parse(&format!("{}{}", LIGHTS, eight)).unwrap().presets.len(), 8);
    }

    #[test]
    fn presets_take_a_colour() {
        let text = format!("{}[preset A]\ncolour = Blue\nKey = on\n[preset B]\nKey = off\ncolor = #a0B1c2\n", LIGHTS);
        let config = parse(&text).unwrap();
        assert_eq!(config.presets[0].colour, Some([0x3a, 0x67, 0xa1]));
        assert_eq!(config.presets[0].settings.len(), 1);
        assert_eq!(config.presets[1].colour, Some([0xa0, 0xb1, 0xc2]));
    }

    #[test]
    fn colour_forms() {
        assert_eq!(colour("grey"), colour("gray"));
        assert_eq!(colour("GREEN"), Some([0x3d, 0x8b, 0x5a]));
        assert_eq!(colour("#000000"), Some([0, 0, 0]));
        for wrong in ["pink", "#12345", "#1234567", "#12345g", "123456", ""] {
            assert_eq!(colour(wrong), None, "{:?}", wrong);
        }
    }

    #[test]
    fn parse_rejects_colour_mistakes() {
        fails(&format!("{}[preset A]\ncolour = pink\nKey = on\n", LIGHTS), "line 8: colour is one of blue");
        fails(&format!("{}[preset A]\ncolour = blue\ncolour = red\nKey = on\n", LIGHTS), "line 9: colour given twice");
        fails(&format!("{}[preset A]\ncolour = blue\n", LIGHTS), "[preset A] sets no lights");
    }

    #[test]
    fn light_settings_after_a_preset_belong_to_the_next_light() {
        let text = "[preset A]\nKey = on\n[Key]\nhost = b\nshape = air\n";
        let config = parse(text).unwrap();
        assert_eq!(config.lights[0].shape, Shape::Air);
    }
}
