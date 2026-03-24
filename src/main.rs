use getsmart::{GetSmartError, get_smart, list_devices};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), GetSmartError> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("list") => {
            let devices = list_devices()?;
            println!("{}", serde_json::to_string_pretty(&devices).unwrap());
        }
        Some("read") => {
            let device_id = args.next().ok_or_else(|| {
                GetSmartError::InvalidArgument("usage: cargo run -- read <device_id>".to_owned())
            })?;
            let report = get_smart(&device_id)?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }
        _ => {
            println!("usage:");
            println!("  cargo run -- list");
            println!("  cargo run -- read <device_id>");
        }
    }

    Ok(())
}
