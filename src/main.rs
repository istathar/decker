use std::error::Error;
use std::time::Duration;

use clap::{Arg, ArgAction, Command};
use elgato_streamdeck::info::Kind;
use elgato_streamdeck::{list_devices, new_hidapi, StreamDeck};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
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
        .get_matches();

    let listing = *matches.get_one::<bool>("list").unwrap();
    let brightness = *matches.get_one::<u8>("brightness").unwrap();

    let result = match listing {
        true => list(),
        false => run(brightness),
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

fn run(brightness: u8) -> Result<(), Box<dyn Error>> {
    let hidapi = new_hidapi()?;
    let (kind, serial) = list_devices(&hidapi)
        .into_iter()
        .find(|(kind, _)| matches!(kind, Kind::Plus | Kind::PlusXl))
        .ok_or("No Stream Deck Plus found. Is another controller application still running?)")?;

    let deck = StreamDeck::connect(&hidapi, kind, &serial)?;
    deck.reset()?;
    deck.set_brightness(brightness)?;

    println!("decker: {:?} serial {}, firmware {}", kind, serial, deck.firmware_version()?);

    let deck = std::sync::Arc::new(deck);
    let reader = deck.get_reader();

    loop {
        for update in reader.read(Some(Duration::from_millis(100)))? {
            println!("{:?}", update);
        }
    }
}
