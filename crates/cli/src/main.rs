//! Headless CLI for paranoia-enc-decryptor.
//!
//! Primarily used for verification, scripting and for environments without a desktop. The GUI is
//! the primary interface; this binary exposes the exact same core library.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use paranoia_enc_core::{
    clear_remembered_password, config_file_path, default_output_dir, inspect_file, parse_argon2_params,
    run_batch, BatchOptions, ConflictStrategy,
};

#[derive(Parser, Debug)]
#[command(
    name = "paranoia-enc-cli",
    version,
    about = "Decrypt Paranoia File Encryption v4 (.enc) AES-256 files"
)]
struct Cli {
    /// .enc files to decrypt (batch).
    #[arg(value_name = "FILES")]
    files: Vec<PathBuf>,

    /// Password for the files. Falls back to the PARANOIA_ENC_PASSWORD environment variable.
    #[arg(short = 'p', long)]
    password: Option<String>,

    /// Output directory (default: directory of the first input file).
    #[arg(short = 'o', long)]
    output_dir: Option<PathBuf>,

    /// What to do if an output file already exists.
    #[arg(long, value_enum, default_value_t = ConflictArg::Rename)]
    conflict: ConflictArg,

    /// Print machine-readable JSON instead of human-readable text.
    #[arg(long)]
    json: bool,

    /// Only inspect the plaintext header of the given files, do not decrypt.
    #[arg(long)]
    inspect: bool,

    /// Print the password store location and exit.
    #[arg(long)]
    config_path: bool,

    /// Delete the remembered password and exit.
    #[arg(long)]
    clear_password: bool,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ConflictArg {
    Overwrite,
    Rename,
    Error,
}

impl From<ConflictArg> for ConflictStrategy {
    fn from(value: ConflictArg) -> Self {
        match value {
            ConflictArg::Overwrite => ConflictStrategy::Overwrite,
            ConflictArg::Rename => ConflictStrategy::Rename,
            ConflictArg::Error => ConflictStrategy::Error,
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.config_path {
        match config_file_path() {
            Ok(path) => {
                println!("{}", path.display());
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::from(2);
            }
        }
    }

    if cli.clear_password {
        return match clear_remembered_password() {
            Ok(()) => {
                println!("remembered password cleared");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(2)
            }
        };
    }

    if cli.inspect {
        return inspect_mode(&cli);
    }

    decrypt_mode(&cli)
}

fn inspect_mode(cli: &Cli) -> ExitCode {
    if cli.files.is_empty() {
        eprintln!("error: no input files given");
        return ExitCode::from(2);
    }
    let mut failures = 0usize;
    let mut results = Vec::new();
    for file in &cli.files {
        match inspect_file(file) {
            Ok(header) => match parse_argon2_params(header.custom_params) {
                Ok(params) => {
                    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
                    if cli.json {
                        results.push(serde_json::json!({
                            "file": file,
                            "size": size,
                            "magic": String::from_utf8_lossy(&header.raw[..5]).to_string(),
                            "version": header.version,
                            "algorithm": header.algorithm,
                            "customParams": header.custom_params,
                            "salt": hex(&header.salt),
                            "argon2": {
                                "iterations": params.iterations,
                                "memoryKiB": params.memory_kib,
                                "parallelism": params.parallelism,
                                "outputLen": params.output_len,
                            },
                        }));
                    } else {
                        println!("file:           {}", file.display());
                        println!("size:           {size} bytes");
                        println!(
                            "magic:          {}",
                            String::from_utf8_lossy(&header.raw[..5])
                        );
                        println!("format version: {}", header.version);
                        println!(
                            "algorithm:      {} (AES-256)",
                            header.algorithm
                        );
                        println!(
                            "custom params:  0x{:02x} (Argon2id t={}, m={} KiB, p={})",
                            header.custom_params,
                            params.iterations,
                            params.memory_kib,
                            params.parallelism
                        );
                        println!("salt:           {}", hex(&header.salt));
                    }
                }
                Err(error) => {
                    failures += 1;
                    if cli.json {
                        results.push(serde_json::json!({
                            "file": file,
                            "errorKind": error.kind(),
                            "errorMessage": error.to_string(),
                        }));
                    } else {
                        eprintln!("FAIL {}: {} ({})", file.display(), error, error.kind());
                    }
                }
            },
            Err(error) => {
                failures += 1;
                if cli.json {
                    results.push(serde_json::json!({
                        "file": file,
                        "errorKind": error.kind(),
                        "errorMessage": error.to_string(),
                    }));
                } else {
                    eprintln!("FAIL {}: {} ({})", file.display(), error, error.kind());
                }
            }
        }
    }
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "files": results }))
                .unwrap_or_default()
        );
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn decrypt_mode(cli: &Cli) -> ExitCode {
    if cli.files.is_empty() {
        eprintln!("error: no input files given");
        return ExitCode::from(2);
    }
    let password = match cli
        .password
        .clone()
        .or_else(|| std::env::var("PARANOIA_ENC_PASSWORD").ok())
    {
        Some(password) => password,
        None => {
            eprintln!("error: no password given (use -p/--password or PARANOIA_ENC_PASSWORD)");
            return ExitCode::from(2);
        }
    };

    let output_dir = cli
        .output_dir
        .clone()
        .or_else(|| default_output_dir(&cli.files));

    let options = BatchOptions {
        files: cli.files.clone(),
        password,
        output_dir,
        conflict: cli.conflict.into(),
    };

    let show_progress = !cli.json && cli.files.len() == 1 && std::io::stderr().is_terminal();
    let mut last_percent = u8::MAX;
    let mut on_progress = |progress: paranoia_enc_core::BatchProgress| {
        if !show_progress {
            return;
        }
        let percent = if progress.overall_total == 0 {
            0
        } else {
            (progress.overall_processed.saturating_mul(100) / progress.overall_total) as u8
        };
        if percent != last_percent {
            last_percent = percent;
            eprintln!("\r{:3}%", percent);
        }
    };

    let summary = run_batch(&options, &mut on_progress);

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        for outcome in &summary.outcomes {
            match (&outcome.error_kind, &outcome.error_message) {
                (Some(kind), Some(message)) => {
                    println!("FAIL {}: {} ({})", outcome.input.display(), message, kind)
                }
                _ => {
                    let outputs = outcome
                        .outputs
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "OK   {} -> {} (original: {}, {} bytes)",
                        outcome.input.display(),
                        outputs,
                        outcome.original_name.as_deref().unwrap_or("?"),
                        outcome.bytes_written
                    );
                }
            }
        }
        println!(
            "successes: {}, failures: {}, output dir: {}",
            summary.successes,
            summary.failures,
            summary.output_dir.display()
        );
    }

    if summary.failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
