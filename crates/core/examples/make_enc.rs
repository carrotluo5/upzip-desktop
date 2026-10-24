//! Generate a Paranoia v4 `.enc` fixture from an existing file.
//!
//! Usage:
//!   cargo run -p unparanoia-core --features testkit --example make_enc -- \
//!       <input-file> <output.enc> <password> [customParamsHex]
//!
//! `customParamsHex` defaults to `00` (t=10, m=10240 KiB, p=4), which is fast enough for tests.
//! The wrapped file becomes the single entry of the inner ZIP; decrypting and extracting the
//! container therefore restores the input under its original file name.

use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage: make_enc <input-file> <output.enc> <password> [customParamsHex, default 00]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 || args.len() > 5 {
        return usage();
    }
    let input = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    let password = &args[3];

    let custom_params = if args.len() == 5 {
        let raw = args[4].trim_start_matches("0x").trim_start_matches("0X");
        match u8::from_str_radix(raw, 16) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("invalid customParamsHex {:?}: {error}", args[4]);
                return usage();
            }
        }
    } else {
        0x00
    };

    match unparanoia_core::testkit::write_enc_file(&input, &output, password, custom_params) {
        Ok(()) => {
            let size = std::fs::metadata(&output).map(|meta| meta.len()).unwrap_or(0);
            println!("wrote {} ({} bytes, customParams=0x{custom_params:02x})", output.display(), size);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
