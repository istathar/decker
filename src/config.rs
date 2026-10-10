use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightConfig {
    pub name: String,
    pub host: String,
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

pub fn load(path: &Path) -> Result<Vec<LightConfig>, Box<dyn Error>> {
    let text = std::fs::read_to_string(path)
        .map_err(|problem| format!("cannot read {}: {}", path.display(), problem))?;
    let lights = parse(&text).map_err(|problem| format!("{}: {}", path.display(), problem))?;
    Ok(lights)
}

/// One `[Name]` section per light, left to right:
///
/// ```text
/// [Key]
/// host = 192.0.2.20
/// shape = panel
/// ```
///
/// Blank lines and lines starting with `#` or `;` are ignored.
pub fn parse(text: &str) -> Result<Vec<LightConfig>, String> {
    let mut sections: Vec<Section> = Vec::new();

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
            if name.is_empty() {
                return Err(format!("line {}: light has no name", number));
            }
            if sections.iter().any(|section| section.name == name) {
                return Err(format!("line {}: [{}] appears twice", number, name));
            }
            sections.push(Section { name: name.to_string(), line: number, host: None, shape: None });
            continue;
        }

        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected setting = value", number))?;
        let (key, value) = (key.trim(), value.trim());

        if value.is_empty() {
            return Err(format!("line {}: {} has no value", number, key));
        }

        let section = sections
            .last_mut()
            .ok_or_else(|| format!("line {}: {} comes before any [light] section", number, key))?;

        match key {
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
    sections.into_iter().map(Section::finish).collect()
}

struct Section {
    name: String,
    line: usize,
    host: Option<String>,
    shape: Option<Shape>,
}

impl Section {
    fn finish(self) -> Result<LightConfig, String> {
        let host = self
            .host
            .ok_or_else(|| format!("line {}: [{}] needs a host", self.line, self.name))?;

        Ok(LightConfig { name: self.name, host, shape: self.shape.unwrap_or(Shape::Panel) })
    }
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

    fn light(name: &str, host: &str, shape: Shape) -> LightConfig {
        LightConfig { name: name.into(), host: host.into(), shape }
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
            parse(text).unwrap(),
            [
                light("MK.2", "192.0.2.93", Shape::Panel),
                light("Air", "192.0.2.81", Shape::Air),
            ]
        );
    }

    #[test]
    fn parse_tolerates_spacing_and_case() {
        let text = "[ Key ]\n  host=key.local  \nshape = PANEL\n";
        assert_eq!(parse(text).unwrap(), [light("Key", "key.local", Shape::Panel)]);
    }

    #[test]
    fn parse_requires_one_to_three_lights() {
        fails("", "no lights");
        fails("# nothing here\n", "no lights");
        let four: String = (1..=4).map(|n| format!("[L{}]\nhost = h{}\n", n, n)).collect();
        fails(&four, "only room for 3");
        let three: String = (1..=3).map(|n| format!("[L{}]\nhost = h{}\n", n, n)).collect();
        assert_eq!(parse(&three).unwrap().len(), 3);
    }

    #[test]
    fn parse_rejects_mistakes_with_line_numbers() {
        fails("[Key]\n", "line 1: [Key] needs a host");
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
        assert!(!parse(text).unwrap().is_empty());
    }
}
