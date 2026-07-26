use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const PREFIX: &[u8] = b"NOBRO-E";

fn dependency_rlib() -> PathBuf {
    let deps = env::current_exe()
        .expect("current test executable")
        .parent()
        .expect("target deps directory")
        .to_path_buf();
    fs::read_dir(&deps)
        .expect("read target deps")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            name.starts_with("libnobro_admission-") && name.ends_with(".rlib")
        })
        .expect("compiled nobro_admission rlib beside integration test")
}

fn compile(case: &str) -> Output {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest.join("tests").join("ui").join(format!("{case}.rs"));
    let rlib = dependency_rlib();
    let deps = rlib.parent().expect("rlib parent");
    let output_dir = env::temp_dir().join(format!(
        "nobro-admission-semantic-{}-{case}",
        std::process::id()
    ));
    fs::create_dir_all(&output_dir).expect("create semantic diagnostic output directory");
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let output = Command::new(rustc)
        .arg(&source)
        .arg("--edition=2021")
        .arg("--crate-type=bin")
        .arg("--emit=metadata")
        .arg("--color=never")
        .arg("--crate-name")
        .arg(format!("nobro_semantic_{case}"))
        .arg("--extern")
        .arg(format!("nobro_admission={}", rlib.display()))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("--out-dir")
        .arg(&output_dir)
        .output()
        .expect("run rustc semantic fixture");
    let _ = fs::remove_dir_all(output_dir);
    output
}

fn stable_codes(output: &Output) -> BTreeSet<String> {
    let mut text = output.stdout.clone();
    text.extend_from_slice(&output.stderr);
    text.windows(PREFIX.len() + 3)
        .filter(|window| {
            window.starts_with(PREFIX) && window[PREFIX.len()..].iter().all(u8::is_ascii_digit)
        })
        .map(|window| String::from_utf8(window.to_vec()).expect("ASCII diagnostic id"))
        .collect()
}

fn detail(output: &Output) -> String {
    format!(
        "status={}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn expected_codes(range: impl Iterator<Item = u8>) -> BTreeSet<String> {
    range.map(|code| format!("NOBRO-E{code:03}")).collect()
}

#[test]
fn admission_compile_diagnostics_assert_stable_semantics_only() {
    let pass = compile("admission_ok");
    assert!(pass.status.success(), "{}", detail(&pass));
    assert!(stable_codes(&pass).is_empty(), "{}", detail(&pass));

    let invalid_deadline = compile("invalid_deadline");
    assert!(
        !invalid_deadline.status.success(),
        "fixture unexpectedly passed"
    );
    assert_eq!(
        stable_codes(&invalid_deadline),
        expected_codes(4..=4),
        "{}",
        detail(&invalid_deadline)
    );

    let all_codes = compile("all_codes");
    assert!(!all_codes.status.success(), "fixture unexpectedly passed");
    assert_eq!(
        stable_codes(&all_codes),
        expected_codes(1..=21),
        "{}",
        detail(&all_codes)
    );
}
