//! Regression test against a real openSUSE Leap 16.0 kernel + firmware image.
//!
//! This test is intentionally slow (it downloads and extracts several hundred MB of RPMs)
//! and is therefore **opt-in**. It is meant to be run before cutting a new release to verify
//! that the cleanup output has not changed compared to the stored reference fixtures.
//!
//! # Running the test
//!
//! ```bash
//! # First time: download packages and generate the reference fixtures
//! IMAGE_JANITOR_LEAP16_TEST=1 IMAGE_JANITOR_LEAP16_GENERATE=1 cargo test leap16 -- --nocapture
//!
//! # Subsequent runs: compare against the stored fixtures
//! IMAGE_JANITOR_LEAP16_TEST=1 cargo test leap16 -- --nocapture
//! ```
//!
//! # Environment variables
//!
//! | Variable | Effect |
//! |---|---|
//! | `IMAGE_JANITOR_LEAP16_TEST` | Must be set (any value) to enable this test. |
//! | `IMAGE_JANITOR_LEAP16_GENERATE` | When set, write new fixture files instead of comparing. |
//! | `IMAGE_JANITOR_LEAP16_DIR` | Override the directory used to cache downloaded/extracted packages. Defaults to `/tmp/image-janitor-leap16`. |
//! | `IMAGE_JANITOR_LEAP16_REFRESH` | When set, delete and re-download/re-extract the packages even if they are already cached. |
//! | `IMAGE_JANITOR_LEAP16_MIRROR` | Override the base URL of the Leap 16.0 `oss` repository. |

use image_janitor::{command::SystemCommandRunner, driver, firmware};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Fixture paths (relative to the workspace root, committed to the repo)
// ---------------------------------------------------------------------------

const FIXTURE_DRIVERS: &str = "tests/fixtures/leap16_drivers_deleted.txt";
const FIXTURE_FIRMWARE: &str = "tests/fixtures/leap16_firmware_deleted.txt";

// ---------------------------------------------------------------------------
// Default repository URL
// ---------------------------------------------------------------------------

const DEFAULT_MIRROR: &str = "https://download.opensuse.org/distribution/leap/16.0/repo/oss/";

// ---------------------------------------------------------------------------
// Helper: collect all regular file paths under `dir`, returned as sorted
// relative paths (relative to `dir`).
// ---------------------------------------------------------------------------

fn collect_files(dir: &Path) -> Vec<String> {
    let mut result = Vec::new();
    collect_files_recursive(dir, dir, &mut result);
    result.sort();
    result
}

fn collect_files_recursive(base: &Path, current: &Path, out: &mut Vec<String>) {
    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Use symlink_metadata so we visit symlinks as entries rather than
        // following them (broken symlinks have no regular metadata).
        let meta = match path.symlink_metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            collect_files_recursive(base, &path, out);
        } else {
            // Include both regular files and symlinks.
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: recursively copy a directory tree (files + symlinks).
// ---------------------------------------------------------------------------

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dst dir");
    for entry in fs::read_dir(src).expect("read src dir").flatten() {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let meta = src_path
            .symlink_metadata()
            .expect("symlink_metadata on src entry");
        if meta.is_dir() {
            copy_dir_all(&src_path, &dst_path);
        } else if meta.file_type().is_symlink() {
            let target = fs::read_link(&src_path).expect("read_link");
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dst_path).expect("create symlink in copy");
        } else {
            fs::copy(&src_path, &dst_path).expect("copy file");
        }
    }
}

// ---------------------------------------------------------------------------
// Download and extract Leap 16.0 kernel + firmware RPMs into `dir`.
// ---------------------------------------------------------------------------

fn download_and_extract(dir: &Path) {
    let mirror =
        env::var("IMAGE_JANITOR_LEAP16_MIRROR").unwrap_or_else(|_| DEFAULT_MIRROR.to_string());
    let mirror = if mirror.ends_with('/') {
        mirror
    } else {
        format!("{}/", mirror)
    };
    let x86_64_repo = format!("{}x86_64/", mirror);
    let noarch_repo = format!("{}noarch/", mirror);

    let client = ureq::Agent::config_builder()
        .user_agent("image-janitor-leap16-test")
        .build()
        .new_agent();

    // ---- kernel-default ----
    println!("  Fetching list of kernel packages from {}...", x86_64_repo);
    let html = client
        .get(&x86_64_repo)
        .call()
        .expect("fetch kernel repo listing")
        .body_mut()
        .read_to_string()
        .expect("read kernel repo listing body");

    let kernel_re =
        regex::Regex::new(r#"href="\./([^"]*kernel-default-[0-9][^"]*\.x86_64\.rpm)""#).unwrap();
    let mut kernel_rpms: Vec<String> = kernel_re
        .captures_iter(&html)
        .map(|c| c[1].to_string())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    kernel_rpms.sort();
    let kernel_rpm = kernel_rpms
        .last()
        .expect("no kernel-default package found in repository listing")
        .clone();

    // ---- kernel-firmware-* ----
    println!(
        "  Fetching list of firmware packages from {}...",
        noarch_repo
    );
    let html = client
        .get(&noarch_repo)
        .call()
        .expect("fetch firmware repo listing")
        .body_mut()
        .read_to_string()
        .expect("read firmware repo listing body");

    let fw_re = regex::Regex::new(r#"href="\./([^"]*kernel-firmware-[a-z0-9-]*-[0-9][^"]*\.rpm)""#)
        .unwrap();
    let mut fw_rpms: Vec<String> = fw_re
        .captures_iter(&html)
        .map(|c| c[1].to_string())
        .filter(|name| !name.starts_with("kernel-firmware-all"))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    fw_rpms.sort();

    let mut tasks: Vec<(String, String)> =
        vec![(format!("{}{}", x86_64_repo, kernel_rpm), kernel_rpm.clone())];
    for fw in fw_rpms {
        tasks.push((format!("{}{}", noarch_repo, fw), fw));
    }

    println!("  Downloading and extracting {} RPM(s)...", tasks.len());
    for (url, rpm) in &tasks {
        let rpm_path = dir.join(rpm);
        if !rpm_path.exists() {
            println!("    Downloading {}...", rpm);
            let mut response = client
                .get(url)
                .call()
                .unwrap_or_else(|e| panic!("failed to download {}: {}", url, e));
            let mut file = fs::File::create(&rpm_path)
                .unwrap_or_else(|e| panic!("create {}: {}", rpm_path.display(), e));
            let mut buf = [0u8; 65536];
            let mut reader = response.body_mut().as_reader();
            loop {
                let n = reader.read(&mut buf).expect("read response");
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n]).expect("write rpm");
            }
        } else {
            println!("    Using cached {}.", rpm);
        }

        println!("    Extracting {}...", rpm);
        let mut rpm2cpio = Command::new("rpm2cpio")
            .arg(&rpm_path)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn rpm2cpio for {}: {}", rpm, e));

        let cpio_status = Command::new("cpio")
            .args(["-idm", "--quiet"])
            .stdin(rpm2cpio.stdout.take().unwrap())
            .current_dir(dir)
            .status()
            .unwrap_or_else(|e| panic!("run cpio for {}: {}", rpm, e));

        let rpm2cpio_status = rpm2cpio.wait().expect("wait rpm2cpio");
        assert!(rpm2cpio_status.success(), "rpm2cpio failed for {}", rpm);
        assert!(cpio_status.success(), "cpio failed for {}", rpm);
    }
    println!("  Extraction complete.");
}

// ---------------------------------------------------------------------------
// Compute the sorted list of files that cleanup_drivers would delete.
// We do this by copying the module dir, running with delete=true, then
// diffing the file sets before and after.
// ---------------------------------------------------------------------------

fn deleted_drivers(module_dir: &Path, work_root: &Path) -> Vec<String> {
    let copy_dir = work_root.join("modules_copy");
    if copy_dir.exists() {
        fs::remove_dir_all(&copy_dir).expect("remove modules copy");
    }
    copy_dir_all(module_dir, &copy_dir);

    let before: HashSet<String> = collect_files(&copy_dir).into_iter().collect();

    let runner = SystemCommandRunner;
    let config_paths = vec!["data/module.list", "data/module.list.extra"];
    driver::cleanup_drivers(&config_paths, &copy_dir, true, true, &runner)
        .expect("cleanup_drivers failed");

    let after: HashSet<String> = collect_files(&copy_dir).into_iter().collect();

    let mut deleted: Vec<String> = before.difference(&after).cloned().collect();
    deleted.sort();
    deleted
}

// ---------------------------------------------------------------------------
// Compute the sorted list of files that cleanup_firmware would delete.
// ---------------------------------------------------------------------------

fn deleted_firmware(module_dir: &Path, fw_dir: &Path, work_root: &Path) -> Vec<String> {
    let copy_fw = work_root.join("firmware_copy");
    if copy_fw.exists() {
        fs::remove_dir_all(&copy_fw).expect("remove firmware copy");
    }
    copy_dir_all(fw_dir, &copy_fw);

    let before: HashSet<String> = collect_files(&copy_fw).into_iter().collect();

    let runner = SystemCommandRunner;
    firmware::cleanup_firmware(module_dir, &copy_fw, true, true, &runner)
        .expect("cleanup_firmware failed");

    let after: HashSet<String> = collect_files(&copy_fw).into_iter().collect();

    let mut deleted: Vec<String> = before.difference(&after).cloned().collect();
    deleted.sort();
    deleted
}

// ---------------------------------------------------------------------------
// The test entry point
// ---------------------------------------------------------------------------

#[test]
fn leap16_regression() {
    if env::var("IMAGE_JANITOR_LEAP16_TEST").is_err() {
        println!("Skipping leap16_regression (set IMAGE_JANITOR_LEAP16_TEST=1 to enable).");
        return;
    }

    // Locate workspace root (Cargo sets CARGO_MANIFEST_DIR for the crate under test).
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_drivers = manifest_dir.join(FIXTURE_DRIVERS);
    let fixture_firmware = manifest_dir.join(FIXTURE_FIRMWARE);

    // Work directory for downloaded/extracted packages.
    let work_dir = env::var("IMAGE_JANITOR_LEAP16_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/image-janitor-leap16"));

    // Optionally refresh (wipe + re-download).
    if env::var("IMAGE_JANITOR_LEAP16_REFRESH").is_ok() && work_dir.exists() {
        println!("Refreshing: removing {}...", work_dir.display());
        fs::remove_dir_all(&work_dir).expect("remove work dir");
    }

    // Download and extract if needed.
    let module_dir = work_dir.join("usr/lib/modules");
    let fw_dir = work_dir.join("usr/lib/firmware");

    if !module_dir.exists() || !fw_dir.exists() {
        println!(
            "Package data not found in {}. Downloading and extracting...",
            work_dir.display()
        );
        fs::create_dir_all(&work_dir).expect("create work dir");
        download_and_extract(&work_dir);
    } else {
        println!("Using cached package data in {}.", work_dir.display());
    }

    assert!(
        module_dir.exists(),
        "module dir not found after extraction: {}",
        module_dir.display()
    );
    assert!(
        fw_dir.exists(),
        "firmware dir not found after extraction: {}",
        fw_dir.display()
    );

    // A scratch directory inside work_dir for the copies we mutate.
    let scratch = work_dir.join("_scratch");
    fs::create_dir_all(&scratch).expect("create scratch dir");

    // Compute deletion lists.
    println!("Computing driver deletions...");
    let drivers_deleted = deleted_drivers(&module_dir, &scratch);

    println!("Computing firmware deletions...");
    let firmware_deleted = deleted_firmware(&module_dir, &fw_dir, &scratch);

    let generate = env::var("IMAGE_JANITOR_LEAP16_GENERATE").is_ok();

    if generate {
        // Write reference fixtures.
        println!("Writing reference fixtures...");
        fs::create_dir_all(fixture_drivers.parent().unwrap()).expect("create fixtures dir");
        fs::write(&fixture_drivers, drivers_deleted.join("\n") + "\n")
            .expect("write drivers fixture");
        fs::write(&fixture_firmware, firmware_deleted.join("\n") + "\n")
            .expect("write firmware fixture");
        println!(
            "Wrote {} driver deletions to {}",
            drivers_deleted.len(),
            fixture_drivers.display()
        );
        println!(
            "Wrote {} firmware deletions to {}",
            firmware_deleted.len(),
            fixture_firmware.display()
        );
    } else {
        // Compare against stored fixtures.
        let expected_drivers = fs::read_to_string(&fixture_drivers).unwrap_or_else(|_| {
            panic!(
                "Fixture file not found: {}. Run with IMAGE_JANITOR_LEAP16_GENERATE=1 to create it.",
                fixture_drivers.display()
            )
        });
        let expected_firmware = fs::read_to_string(&fixture_firmware).unwrap_or_else(|_| {
            panic!(
                "Fixture file not found: {}. Run with IMAGE_JANITOR_LEAP16_GENERATE=1 to create it.",
                fixture_firmware.display()
            )
        });

        let expected_drivers: Vec<&str> =
            expected_drivers.lines().filter(|l| !l.is_empty()).collect();
        let expected_firmware: Vec<&str> = expected_firmware
            .lines()
            .filter(|l| !l.is_empty())
            .collect();

        // Compute diffs for useful error messages.
        let drivers_deleted_set: HashSet<&str> =
            drivers_deleted.iter().map(String::as_str).collect();
        let expected_drivers_set: HashSet<&str> = expected_drivers.iter().copied().collect();
        let firmware_deleted_set: HashSet<&str> =
            firmware_deleted.iter().map(String::as_str).collect();
        let expected_firmware_set: HashSet<&str> = expected_firmware.iter().copied().collect();

        let drivers_only_in_actual: Vec<_> = drivers_deleted_set
            .difference(&expected_drivers_set)
            .copied()
            .collect();
        let drivers_only_in_expected: Vec<_> = expected_drivers_set
            .difference(&drivers_deleted_set)
            .copied()
            .collect();
        let firmware_only_in_actual: Vec<_> = firmware_deleted_set
            .difference(&expected_firmware_set)
            .copied()
            .collect();
        let firmware_only_in_expected: Vec<_> = expected_firmware_set
            .difference(&firmware_deleted_set)
            .copied()
            .collect();

        let mut diff_lines = Vec::new();
        if !drivers_only_in_actual.is_empty() || !drivers_only_in_expected.is_empty() {
            diff_lines.push("=== Driver cleanup diff ===".to_string());
            let mut extra = drivers_only_in_actual.clone();
            extra.sort();
            for f in &extra {
                diff_lines.push(format!("+ {}", f));
            }
            let mut missing = drivers_only_in_expected.clone();
            missing.sort();
            for f in &missing {
                diff_lines.push(format!("- {}", f));
            }
        }
        if !firmware_only_in_actual.is_empty() || !firmware_only_in_expected.is_empty() {
            diff_lines.push("=== Firmware cleanup diff ===".to_string());
            let mut extra = firmware_only_in_actual.clone();
            extra.sort();
            for f in &extra {
                diff_lines.push(format!("+ {}", f));
            }
            let mut missing = firmware_only_in_expected.clone();
            missing.sort();
            for f in &missing {
                diff_lines.push(format!("- {}", f));
            }
        }

        if !diff_lines.is_empty() {
            panic!(
                "Cleanup output differs from reference fixtures.\n\
                 (+ = new deletion not in reference, - = deletion missing from output)\n\
                 {}\n\
                 Re-run with IMAGE_JANITOR_LEAP16_GENERATE=1 to update the fixtures.",
                diff_lines.join("\n")
            );
        }

        println!(
            "OK: driver deletions match ({} files), firmware deletions match ({} files).",
            drivers_deleted.len(),
            firmware_deleted.len()
        );
    }
}
