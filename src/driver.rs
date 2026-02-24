use crate::command::CommandRunner;
use crate::config;
use crate::error::JanitorError;
use crate::util;
use log::{debug, info, warn};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Driver {
    name: String,
    path: PathBuf,
    deps: Vec<String>,
}

impl Driver {
    fn from_file(path: &Path, runner: &dyn CommandRunner) -> Result<Self, JanitorError> {
        let deps_str = match runner.run(
            "/usr/sbin/modinfo",
            &["-F", "depends", path.to_str().unwrap()],
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!("modinfo for {} failed: {}", path.display(), e);
                String::new()
            }
        };

        let deps = deps_str
            .trim()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        let name = path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .split('.')
            .next()
            .unwrap()
            .to_string();

        Ok(Driver {
            name,
            path: path.to_path_buf(),
            deps,
        })
    }
}

pub fn cleanup_drivers(
    config_paths: &[&str],
    module_dir: &Path,
    delete: bool,
    no_parallel: bool,
    runner: &(dyn CommandRunner + Sync),
) -> Result<(), JanitorError> {
    let (to_keep_re, to_delete_re) = config::read_config(config_paths, runner)?;
    let kernel_dir = util::find_kernel_dir(module_dir)?;
    info!("Scanning kernel modules in {}", kernel_dir.display());

    let mut module_paths = Vec::new();
    for path in util::walk_dir(&kernel_dir)? {
        if path.is_file()
            && (path.extension().is_some_and(|e| e == "ko")
                || path.to_str().is_some_and(|s| s.ends_with(".ko.xz"))
                || path.to_str().is_some_and(|s| s.ends_with(".ko.zst")))
        {
            module_paths.push(path.to_path_buf());
        }
    }

    if no_parallel {
        info!("Using 1 thread for scanning kernel modules");
    } else {
        info!("Using multiple threads for scanning kernel modules");
    }

    let driver_map: HashMap<String, Driver> = if no_parallel {
        module_paths
            .into_iter()
            .map(|path| {
                let driver = Driver::from_file(&path, runner)?;
                Ok((driver.name.clone(), driver))
            })
            .collect::<Result<HashMap<_, _>, JanitorError>>()?
    } else {
        module_paths
            .into_par_iter()
            .map(|path| {
                let driver = Driver::from_file(&path, runner)?;
                Ok((driver.name.clone(), driver))
            })
            .collect::<Result<HashMap<_, _>, JanitorError>>()?
    };

    let mut to_keep: HashSet<Driver> = HashSet::new();

    for driver in driver_map.values() {
        let kernel_path = driver
            .path
            .strip_prefix(&kernel_dir)
            .unwrap()
            .to_str()
            .ok_or_else(|| JanitorError::InvalidPath(driver.path.clone()))?;

        if to_delete_re.iter().any(|r| r.is_match(kernel_path)) {
            debug!("Marked for deletion by config: {}", driver.path.display());
        } else if to_keep_re.iter().any(|r| r.is_match(kernel_path)) {
            debug!("Marked for keeping by config: {}", driver.path.display());
            to_keep.insert(driver.clone());
        }
    }

    info!("Checking driver dependencies...");
    let mut worklist: Vec<Driver> = to_keep.iter().cloned().collect();
    while let Some(driver) = worklist.pop() {
        for dep_name in &driver.deps {
            if let Some(dep_driver) = driver_map.get(dep_name) {
                // If the dependency was not already in to_keep, add it and
                // put it on the worklist to process its dependencies.
                if to_keep.insert(dep_driver.clone()) {
                    debug!("Keep dependant driver {}", dep_driver.path.display());
                    worklist.push(dep_driver.clone());
                }
            }
        }
    }

    let to_delete: Vec<_> = driver_map
        .values()
        .filter(|d| !to_keep.contains(d))
        .collect();

    info!("Found {} drivers to delete", to_delete.len());
    debug!(
        "Drivers to delete: {:?}",
        to_delete.iter().map(|d| &d.path).collect::<Vec<_>>()
    );

    if delete {
        let mut deleted_size = 0;
        for driver in to_delete {
            debug!("Deleting {}", driver.path.display());
            deleted_size += fs::metadata(&driver.path)?.len();
            fs::remove_file(&driver.path)?;
        }
        info!(
            "Deleted drivers: {} bytes ({} MiB)",
            deleted_size,
            deleted_size >> 20
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandRunner;
    use std::collections::HashMap;
    use tempfile::tempdir;

    struct MockCommandRunner {
        responses: HashMap<String, String>,
    }

    impl CommandRunner for MockCommandRunner {
        fn run(&self, command: &str, args: &[&str]) -> Result<String, JanitorError> {
            let key = if args.is_empty() {
                command.to_string()
            } else {
                format!("{} {}", command, args.join(" "))
            };
            self.responses
                .get(&key)
                .cloned()
                .ok_or(JanitorError::Command(format!("Not mocked: {}", key)))
        }
    }

    #[test]
    fn test_cleanup_drivers() {
        let temp_dir = tempdir().unwrap();
        let module_dir = temp_dir.path();
        let kernel_dir = module_dir.join("6.1.0-test");
        fs::create_dir_all(&kernel_dir).unwrap();

        let mod_a_path = kernel_dir.join("a.ko");
        let mod_b_path = kernel_dir.join("b.ko");
        let mod_c_path = kernel_dir.join("c.ko");
        let mod_d_path = kernel_dir.join("d.ko");

        fs::write(&mod_a_path, "").unwrap();
        fs::write(&mod_b_path, "").unwrap();
        fs::write(&mod_c_path, "").unwrap();
        fs::write(&mod_d_path, "").unwrap();

        let config_path = temp_dir.path().join("test.conf");
        fs::write(&config_path, "a.ko").unwrap();

        let mut responses = HashMap::new();
        responses.insert(
            format!("/usr/sbin/modinfo -F depends {}", mod_a_path.display()),
            "b".to_string(),
        );
        responses.insert(
            format!("/usr/sbin/modinfo -F depends {}", mod_b_path.display()),
            "c".to_string(),
        );
        responses.insert(
            format!("/usr/sbin/modinfo -F depends {}", mod_c_path.display()),
            "".to_string(),
        );
        responses.insert(
            format!("/usr/sbin/modinfo -F depends {}", mod_d_path.display()),
            "".to_string(),
        );
        responses.insert("arch".to_string(), "x86_64".to_string());

        let runner = MockCommandRunner { responses };

        // Test dry run
        cleanup_drivers(
            &[config_path.to_str().unwrap()],
            module_dir,
            false,
            false,
            &runner,
        )
        .unwrap();
        assert!(mod_a_path.exists());
        assert!(mod_b_path.exists());
        assert!(mod_c_path.exists());
        assert!(mod_d_path.exists());

        // Test delete
        cleanup_drivers(
            &[config_path.to_str().unwrap()],
            module_dir,
            true,
            false,
            &runner,
        )
        .unwrap();
        assert!(mod_a_path.exists());
        assert!(mod_b_path.exists());
        assert!(mod_c_path.exists());
        assert!(!mod_d_path.exists());
    }

    #[test]
    fn test_cleanup_drivers_large_parallel() {
        let temp_dir = tempdir().unwrap();
        let module_dir = temp_dir.path();
        let kernel_dir = module_dir.join("6.1.0-test");
        fs::create_dir_all(&kernel_dir).unwrap();

        let mut responses = HashMap::new();
        let mut modules = Vec::new();

        // Create 100 modules to ensure parallelism is used
        for i in 0..100 {
            let mod_path = kernel_dir.join(format!("mod{}.ko", i));
            fs::write(&mod_path, "").unwrap();
            modules.push(mod_path.clone());

            // mod(i) depends on mod(i+1) if i < 99
            let deps = if i < 99 {
                format!("mod{}", i + 1)
            } else {
                "".to_string()
            };

            responses.insert(
                format!("/usr/sbin/modinfo -F depends {}", mod_path.display()),
                deps,
            );
        }
        responses.insert("arch".to_string(), "x86_64".to_string());

        let config_path = temp_dir.path().join("test.conf");
        // Only keep mod0. Because of dependencies, everything from mod0 to mod99 should be kept.
        fs::write(&config_path, "mod0.ko").unwrap();

        let runner = MockCommandRunner { responses };

        // Test delete - should keep all modules because mod0 depends on mod1, mod1 on mod2, ..., mod99 on nothing
        cleanup_drivers(
            &[config_path.to_str().unwrap()],
            module_dir,
            true,
            false,
            &runner,
        )
        .unwrap();

        for mod_path in modules {
            assert!(
                mod_path.exists(),
                "Module {} should have been kept",
                mod_path.display()
            );
        }

        // Now test deleting everything by having an empty config
        let config_path_empty = temp_dir.path().join("empty.conf");
        fs::write(&config_path_empty, "nothing.ko").unwrap();

        cleanup_drivers(
            &[config_path_empty.to_str().unwrap()],
            module_dir,
            true,
            false,
            &runner,
        )
        .unwrap();

        for i in 0..100 {
            let mod_path = kernel_dir.join(format!("mod{}.ko", i));
            assert!(
                !mod_path.exists(),
                "Module {} should have been deleted",
                mod_path.display()
            );
        }
    }

    #[test]
    fn test_cleanup_drivers_no_parallel() {
        let temp_dir = tempdir().unwrap();
        let module_dir = temp_dir.path();
        let kernel_dir = module_dir.join("6.1.0-test");
        fs::create_dir_all(&kernel_dir).unwrap();

        let mod_a_path = kernel_dir.join("a.ko");
        fs::write(&mod_a_path, "").unwrap();

        let config_path = temp_dir.path().join("test.conf");
        fs::write(&config_path, "a.ko").unwrap();

        let mut responses = HashMap::new();
        responses.insert(
            format!("/usr/sbin/modinfo -F depends {}", mod_a_path.display()),
            "".to_string(),
        );
        responses.insert("arch".to_string(), "x86_64".to_string());

        let runner = MockCommandRunner { responses };

        cleanup_drivers(
            &[config_path.to_str().unwrap()],
            module_dir,
            true,
            true, // no_parallel = true
            &runner,
        )
        .unwrap();

        assert!(mod_a_path.exists());
    }
}
