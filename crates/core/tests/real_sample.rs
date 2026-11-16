//! Optional integration test against a real Paranoia sample.
//!
//! The real samples are user data and never committed, so this test is ignored by default.
//! Run it explicitly with the environment variables pointing at a sample (and its expected
//! SHA-256) to reproduce the acceptance verification:
//!
//!   PARANOIA_SAMPLE_ENC=/path/to/3138.7z.002.enc
//!   PARANOIA_SAMPLE_SHA256=46abe32c...
//!   PARANOIA_SAMPLE_PASSWORD=123
//!   cargo test -p upzip-core --test real_sample -- --ignored --nocapture

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use upzip_core::{
    decrypt_file, run_batch, BatchOptions, ConflictStrategy, DecryptOptions,
};
use sha2::{Digest, Sha256};

fn sample_path() -> Option<PathBuf> {
    std::env::var_os("PARANOIA_SAMPLE_ENC").map(PathBuf::from)
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "upzip-enc-test-sample-{tag}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha256_of(path: &PathBuf) -> String {
    let data = fs::read(path).unwrap();
    let digest = Sha256::digest(&data);
    let mut out = String::new();
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[test]
#[ignore = "requires PARANOIA_SAMPLE_ENC / PARANOIA_SAMPLE_SHA256 environment variables"]
fn decrypts_real_sample_byte_identically() {
    let sample = match sample_path() {
        Some(path) => path,
        None => {
            eprintln!("PARANOIA_SAMPLE_ENC not set; skipping");
            return;
        }
    };
    let expected = std::env::var("PARANOIA_SAMPLE_SHA256").expect("PARANOIA_SAMPLE_SHA256");
    let password = std::env::var("PARANOIA_SAMPLE_PASSWORD").unwrap_or_else(|_| "123".to_string());
    let original_len = fs::metadata(&sample).unwrap().len();

    let out_dir = temp_dir("single");
    let options = DecryptOptions {
        output_dir: out_dir.clone(),
        conflict: ConflictStrategy::Rename,
    };
    let report = decrypt_file(&sample, &password, &options, &mut |_, _| {}).expect("decrypt");
    assert_eq!(report.outputs.len(), 1);
    let actual = sha256_of(&report.outputs[0]);
    println!(
        "sample={} output={} bytes_expected_sha256={} actual_sha256={}",
        sample.display(),
        report.outputs[0].display(),
        expected,
        actual
    );
    assert_eq!(actual, expected, "sha256 mismatch");
    // The input sample must never be modified.
    assert_eq!(fs::metadata(&sample).unwrap().len(), original_len);
    fs::remove_dir_all(&out_dir).unwrap();
}

#[test]
#[ignore = "requires PARANOIA_SAMPLE_ENC / PARANOIA_SAMPLE_SHA256 environment variables"]
fn batch_decrypts_two_copies_and_isolates_failures() {
    let sample = match sample_path() {
        Some(path) => path,
        None => {
            eprintln!("PARANOIA_SAMPLE_ENC not set; skipping");
            return;
        }
    };
    let password = std::env::var("PARANOIA_SAMPLE_PASSWORD").unwrap_or_else(|_| "123".to_string());
    let expected = std::env::var("PARANOIA_SAMPLE_SHA256").expect("PARANOIA_SAMPLE_SHA256");

    let work = temp_dir("batch");
    let input_dir = work.join("inputs");
    fs::create_dir_all(&input_dir).unwrap();
    let copy_a = input_dir.join("copy-a.enc");
    let copy_b = input_dir.join("copy-b.enc");
    fs::copy(&sample, &copy_a).unwrap();
    fs::copy(&sample, &copy_b).unwrap();
    // A third file that no longer decrypts with this password must fail without
    // aborting the batch. Flipping one ciphertext byte makes the plaintext
    // checkcode invalid, so the decryptor reports a wrong-password error.
    let bad = input_dir.join("bad.enc");
    fs::copy(&sample, &bad).unwrap();
    {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut file = fs::OpenOptions::new().read(true).write(true).open(&bad).unwrap();
        file.seek(SeekFrom::Start(40)).unwrap();
        let mut byte = [0u8; 1];
        file.read_exact(&mut byte).unwrap();
        byte[0] ^= 0xff;
        file.seek(SeekFrom::Start(40)).unwrap();
        file.write_all(&byte).unwrap();
    }

    let options = BatchOptions {
        files: vec![copy_a.clone(), bad.clone(), copy_b.clone()],
        password,
        output_dir: None,
        conflict: ConflictStrategy::Rename,
    };
    let mut last_overall = 0u64;
    let summary = run_batch(&options, &mut |progress| {
        assert!(progress.overall_processed >= last_overall);
        last_overall = progress.overall_processed;
    });
    assert_eq!(summary.successes, 2, "outcomes: {:?}", summary.outcomes);
    assert_eq!(summary.failures, 1);
    assert_eq!(summary.output_dir, input_dir);

    let mut hashes: Vec<String> = summary
        .outcomes
        .iter()
        .filter(|outcome| outcome.is_ok())
        .flat_map(|outcome| outcome.outputs.iter())
        .map(sha256_of)
        .collect();
    hashes.sort();
    assert_eq!(hashes, vec![expected.clone(), expected]);
    assert!(summary.outcomes[1].error_kind.is_some());
    fs::remove_dir_all(&work).unwrap();
}
