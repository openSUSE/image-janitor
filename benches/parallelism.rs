use image_janitor::command::CommandRunner;
use image_janitor::driver;
use image_janitor::firmware;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

struct BenchmarkRunner;
impl CommandRunner for BenchmarkRunner {
    fn run(
        &self,
        command: &str,
        args: &[&str],
    ) -> Result<String, image_janitor::error::JanitorError> {
        let output = Command::new(command).args(args).output().map_err(|e| {
            image_janitor::error::JanitorError::Command(format!("Failed to run command: {}", e))
        })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(image_janitor::error::JanitorError::Command(format!(
                "Command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
        }
    }
}

fn download_and_extract(dir: &Path) {
    let repo_base = env::var("IMAGE_JANITOR_BENCH_MIRROR")
        .unwrap_or_else(|_| "https://download.opensuse.org/tumbleweed/repo/oss/".to_string());
    let repo_base = if repo_base.ends_with('/') {
        repo_base
    } else {
        format!("{}/", repo_base)
    };
    let x86_64_repo = format!("{}x86_64/", repo_base);
    let noarch_repo = format!("{}noarch/", repo_base);

    let client = reqwest::blocking::Client::builder()
        .user_agent("image-janitor-benchmark")
        .redirect(reqwest::redirect::Policy::default())
        .build()
        .expect("Failed to build reqwest client");

    println!("Fetching list of kernel packages...");
    let response = client
        .get(&x86_64_repo)
        .send()
        .expect("Failed to fetch kernel repo list");
    let html = response
        .text()
        .expect("Failed to read kernel repo list response");

    let kernel_re =
        regex::Regex::new(r#"href="./(kernel-default-[0-9][^"]*\.x86_64\.rpm)""#).unwrap();
    let mut kernel_rpms: Vec<_> = kernel_re
        .captures_iter(&html)
        .map(|c| c[1].to_string())
        .collect();
    kernel_rpms.sort();
    let kernel_rpm = kernel_rpms.last().expect("No kernel-default package found");

    let mut tasks = vec![(format!("{}{}", x86_64_repo, kernel_rpm), kernel_rpm.clone())];

    println!("Fetching list of firmware packages...");
    let response = client
        .get(&noarch_repo)
        .send()
        .expect("Failed to fetch firmware repo list");
    let html = response
        .text()
        .expect("Failed to read firmware repo list response");

    let re = regex::Regex::new(r#"href="./(kernel-firmware-[a-z0-9-]*-[0-9][^"]*\.rpm)""#).unwrap();

    for cap in re.captures_iter(&html) {
        let fw_rpm = &cap[1];
        if fw_rpm.starts_with("kernel-firmware-all") {
            continue; // Skip the meta-package
        }
        tasks.push((format!("{}{}", noarch_repo, fw_rpm), fw_rpm.to_string()));
    }

    println!("Processing {} packages in parallel...", tasks.len());

    let multi = MultiProgress::new();
    let style = ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta}) {msg}")
        .unwrap()
        .progress_chars("#>-");

    tasks.into_par_iter().for_each(|(url, rpm)| {
        let rpm_path = dir.join(&rpm);
        let pb = multi.add(ProgressBar::new(0));
        pb.set_style(style.clone());
        pb.set_message(rpm.clone());

        if !rpm_path.exists() {
            let mut response = client.get(&url).send().expect("Failed to download RPM");
            if !response.status().is_success() {
                panic!("Failed to download {}: {}", url, response.status());
            }
            if let Some(len) = response.content_length() {
                pb.set_length(len);
            }

            let mut file = fs::File::create(&rpm_path).expect("Failed to create RPM file");
            let mut buffer = [0; 8192];
            loop {
                let n = response
                    .read(&mut buffer)
                    .expect("Failed to read from response");
                if n == 0 {
                    break;
                }
                file.write_all(&buffer[..n])
                    .expect("Failed to write to RPM file");
                pb.inc(n as u64);
            }
        } else {
            pb.set_message(format!("{} (cached)", rpm));
            let meta = fs::metadata(&rpm_path).expect("Failed to get RPM metadata");
            pb.set_length(meta.len());
            pb.set_position(meta.len());
        }

        pb.set_message(format!("Extracting {}...", rpm));
        let mut rpm2cpio = Command::new("rpm2cpio")
            .arg(&rpm_path)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("Failed to spawn rpm2cpio for {}: {}", rpm, e));

        let cpio = Command::new("cpio")
            .arg("-idm")
            .stdin(rpm2cpio.stdout.take().unwrap())
            .current_dir(dir)
            .status()
            .unwrap_or_else(|e| panic!("Failed to run cpio for {}: {}", rpm, e));

        let rpm2cpio_status = rpm2cpio.wait().expect("Failed to wait for rpm2cpio");
        assert!(rpm2cpio_status.success(), "rpm2cpio failed for {}", rpm);
        assert!(cpio.success(), "cpio failed for {}", rpm);

        pb.finish_with_message(format!("Done: {}", rpm));
    });
}

fn main() {
    // Initialize logger to see info messages
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let runner = BenchmarkRunner;

    let bench_dir = env::var("IMAGE_JANITOR_BENCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut p = env::temp_dir();
            p.push("image-janitor-bench");
            p
        });

    if env::var("IMAGE_JANITOR_BENCH_EXTRACT").is_ok() {
        if bench_dir.exists() {
            println!("Cleaning up benchmark directory {}...", bench_dir.display());
            for entry in fs::read_dir(&bench_dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    let _ = fs::remove_dir_all(path);
                } else if path.is_file() {
                    if path.extension().is_some_and(|ext| ext == "rpm") {
                        if env::var("IMAGE_JANITOR_BENCH_NO_CLEAN").is_err() {
                            let _ = fs::remove_file(path);
                        }
                    } else {
                        let _ = fs::remove_file(path);
                    }
                }
            }
        }
        fs::create_dir_all(&bench_dir).unwrap();
        download_and_extract(&bench_dir);
    }

    if !bench_dir.exists() {
        println!(
            "Benchmark data not found in {}. Skipping benchmarks.",
            bench_dir.display()
        );
        println!("Set IMAGE_JANITOR_BENCH_EXTRACT=1 to download and extract openSUSE Tumbleweed packages.");
        return;
    }

    let module_dir = bench_dir.join("usr/lib/modules");
    let firmware_dir = bench_dir.join("usr/lib/firmware");

    if !module_dir.exists() || !firmware_dir.exists() {
        println!(
            "Module or firmware directory not found in {}. Skipping benchmarks.",
            bench_dir.display()
        );
        return;
    }

    // Use real configuration files from the data directory.
    let config_paths = vec!["data/module.list", "data/module.list.extra"];

    println!("\nStarting Benchmarks...");
    println!("======================");

    // Warm up run (Parallel)
    println!("\n[Warm-up] Running Parallel execution (warming up disk cache)...");
    let _ = driver::cleanup_drivers(&config_paths, &module_dir, false, false, &runner);
    let _ = firmware::cleanup_firmware(&module_dir, &firmware_dir, false, false, &runner);

    // Run Non-Parallel
    println!("\n[1/2] Running Non-Parallel execution...");
    let start_np = Instant::now();
    driver::cleanup_drivers(&config_paths, &module_dir, false, true, &runner).unwrap();
    firmware::cleanup_firmware(&module_dir, &firmware_dir, false, true, &runner).unwrap();
    let duration_np = start_np.elapsed();

    // Run Parallel
    println!("\n[2/2] Running Parallel execution...");
    let start_p = Instant::now();
    driver::cleanup_drivers(&config_paths, &module_dir, false, false, &runner).unwrap();
    firmware::cleanup_firmware(&module_dir, &firmware_dir, false, false, &runner).unwrap();
    let duration_p = start_p.elapsed();

    println!("\nSummary Results");
    println!("===============");
    println!("Non-Parallel Total Time: {:.2?}", duration_np);
    println!("Parallel Total Time:     {:.2?}", duration_p);
    println!("-------------------------------");
    let speedup = duration_np.as_secs_f64() / duration_p.as_secs_f64();
    println!("Speedup factor:          {:.2}x", speedup);
}
