# Image Janitor

Image Janitor is a command-line tool for cleaning up unused kernel drivers and firmware from a Linux system. It helps to reduce the size of a Linux image by removing unnecessary files.

## Features

*   **Driver Cleanup**: Removes unused kernel drivers.
*   **Firmware Cleanup**: Removes unused firmware files.
*   **Parallelism**: Uses multiple threads for scanning kernel modules and firmware by default. Parallelism can be disabled using the `--no-parallel` flag.
*   **Configuration**: Uses configuration files to determine which files to keep and which to delete.
*   **Dependency Resolution**: Resolves dependencies between kernel modules to avoid breaking the system.

## Usage

### Global Options

*   `--verbose`, `-v`: Enable verbose logging.
*   `--no-parallel`: Disable parallelism.

### Driver Cleanup

To clean up unused kernel drivers, run the following command:

```bash
image-janitor driver-cleanup
```

By default, the command will perform a dry run and only show the files that would be deleted. To actually delete the files, use the `--delete` flag:

```bash
image-janitor driver-cleanup --delete
```

You can also specify the directory containing the kernel modules and the configuration files to use:

```bash
image-janitor driver-cleanup --module-dir /path/to/modules --config-files /path/to/config1,/path/to/config2
```

### Firmware Cleanup

To clean up unused firmware, run the following command:

```bash
image-janitor fw-cleanup
```

By default, the command will perform a dry run and only show the files that would be deleted. To actually delete the files, use the `--delete` flag:

```bash
image-janitor fw-cleanup --delete
```

You can also specify the directory containing the kernel modules and the firmware files:

```bash
image-janitor fw-cleanup --module-dir /path/to/modules --firmware-dir /path/to/firmware
```

## Building from Source

To build the project from source, you will need to have Rust installed. You can then clone the repository and build the project using Cargo:

```bash
git clone https://github.com/fcrozat/image-janitor.git
cd image-janitor
cargo build --release
```

The executable will be located in the `target/release` directory.

## Benchmarks

A benchmark for testing the performance of parallelism is available. It can use real kernel and firmware packages from openSUSE Tumbleweed.

To run the benchmark, you first need to download and extract the packages (requires `rpm2cpio` and `cpio`):

```bash
IMAGE_JANITOR_BENCH_EXTRACT=1 cargo bench --bench parallelism
```

Subsequent runs can omit the extraction step:

```bash
cargo bench --bench parallelism
```

By default, the benchmark data is stored in `/tmp/image-janitor-bench`. You can change this using the `IMAGE_JANITOR_BENCH_DIR` environment variable.

If you want to re-run the extraction without deleting previously extracted files (e.g. if you added more packages manually), you can set `IMAGE_JANITOR_BENCH_NO_CLEAN=1`.

You can also specify an alternative mirror for downloading the packages using the `IMAGE_JANITOR_BENCH_MIRROR` environment variable (it should point to the root of the Tumbleweed `oss` repository).

## Configuration

The configuration files use a simple format. Each line contains a regular expression that is matched against the path of a file. If the path matches a regular expression, the file is kept. If the path does not match any regular expression, the file is deleted.

You can also specify which files to delete by prefixing the regular expression with a `-`. For example, to delete all files in the `drivers/net` directory, you would add the following line to your configuration file:

```
-drivers/net/.*
```

The configuration files also support architecture-specific sections. For example, to specify that a driver should only be kept on x86_64 systems, you would add the following lines to your configuration file:

```
<x86_64>
drivers/net/ethernet/intel/.*
</x86_64>
```

Configuration files used for [Agama](https://agama-project.github.io/) installer are available in the `data` subdirectory.
