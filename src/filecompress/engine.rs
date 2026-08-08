use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;

use super::config::{CompressAlgo, CompressConfig, CompressFmt};

#[derive(Debug)]
pub struct CompressResult {
    pub output_paths: Vec<PathBuf>,
    pub cpu_secs: f64,
    pub input_total_bytes: u64,
    pub output_total_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CompressError {
    #[error("Timeout during compression")]
    Timeout,
    #[error("Failed to spawn subprocess: {0}")]
    SpawnFailed(String),
    #[error("Process failed with exit code {exit_code}: {stderr}")]
    ProcessFailed { exit_code: i32, stderr: String },
    #[error("No output files generated")]
    NoOutput,
    #[error("Cancelled by user")]
    Cancelled,
}

#[allow(clippy::too_many_arguments)]
pub async fn run_compress(
    work_dir: &Path,
    config: &CompressConfig,
    input_files: &[PathBuf],
    timeout: Duration,
    cores: &[i32],
    trace_id: u64,
    cancel: &Arc<AtomicBool>,
) -> Result<CompressResult, CompressError> {
    let input_total_bytes: u64 = input_files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();

    let archive_base = "archive";
    let (cmd_name, args) = build_command(archive_base, config, input_files, cores);

    log_ev!("filecompress", trace_id, "compress_spawn", "cmd" => &cmd_name, "args" => format!("{args:?}"));

    let rusage_before = get_children_cpu_time();

    let child = tokio::process::Command::new(&cmd_name)
        .args(&args)
        .current_dir(work_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CompressError::SpawnFailed(e.to_string()))?;
    let mut child = child;

    if !cores.is_empty() {
        if let Some(pid) = child.id() {
            pin_pid_to_cores(pid, cores, trace_id);
        }
    }

    // stderr در پس‌زمینه خوانده می‌شود؛ با `wait()` تنها، پر شدن لوله قفل می‌کند.
    let stderr_task = child.stderr.take().map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf).await;
            buf
        })
    });

    let t0 = Instant::now();
    // انصراف کاربر باید پروسه را بکشد، نه اینکه CPU تا آخر بسوزد و بعد
    // خروجی دور ریخته شود. هر ۵۰۰ms پرچم و مهلت کل چک می‌شوند.
    let status_res = loop {
        match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
            Ok(res) => break Some(res),
            Err(_) => {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    log_ev!("filecompress", trace_id, "compress_killed", "reason" => "cancelled", "wall_secs" => format!("{:.2}", t0.elapsed().as_secs_f64()));
                    return Err(CompressError::Cancelled);
                }
                if t0.elapsed() >= timeout {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    log_ev!("filecompress", trace_id, "compress_killed", "reason" => "timeout");
                    break None;
                }
            }
        }
    };

    let stderr_bytes = match stderr_task {
        Some(h) => h.await.unwrap_or_default(),
        None => Vec::new(),
    };

    let rusage_after = get_children_cpu_time();
    let wall_elapsed = t0.elapsed();

    let raw_cpu_secs = rusage_after - rusage_before;
    // Fallback if rusage difference is <= 0 or imprecise
    let cpu_secs = if raw_cpu_secs > 0.01 {
        raw_cpu_secs
    } else {
        wall_elapsed.as_secs_f64() * cores.len().max(1) as f64
    };

    let status = match status_res {
        Some(Ok(st)) => st,
        Some(Err(e)) => {
            return Err(CompressError::ProcessFailed {
                exit_code: -1,
                stderr: e.to_string(),
            });
        }
        None => {
            return Err(CompressError::Timeout);
        }
    };

    log_ev!(
        "filecompress",
        trace_id,
        "compress_done",
        "exit_code" => status.code().unwrap_or(-1),
        "cpu_secs" => format!("{cpu_secs:.2}"),
        "wall_secs" => format!("{:.2}", wall_elapsed.as_secs_f64())
    );

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
        return Err(CompressError::ProcessFailed {
            exit_code: status.code().unwrap_or(-1),
            stderr,
        });
    }

    let output_paths = collect_outputs(work_dir, archive_base, config);
    if output_paths.is_empty() {
        return Err(CompressError::NoOutput);
    }

    let output_total_bytes: u64 = output_paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();

    Ok(CompressResult {
        output_paths,
        cpu_secs,
        input_total_bytes,
        output_total_bytes,
    })
}

fn build_command(
    archive_base: &str,
    config: &CompressConfig,
    input_files: &[PathBuf],
    cores: &[i32],
) -> (String, Vec<String>) {
    let threads = cores.len().max(1);
    let mut args = Vec::new();

    match config.fmt {
        CompressFmt::SevenZ => {
            let out_name = format!("{archive_base}.7z");
            args.push("a".to_string());
            args.push("-t7z".to_string());
            args.push(format!("-mx={}", config.level.clamp(1, 9)));
            args.push(format!("-mmt={threads}"));

            match config.algo {
                CompressAlgo::Lzma2 => args.push("-m0=lzma2".to_string()),
                CompressAlgo::Ppmd => args.push("-m0=ppmd".to_string()),
                CompressAlgo::Bzip2 => args.push("-m0=bzip2".to_string()),
            }

            if config.solid {
                args.push("-ms=on".to_string());
            } else {
                args.push("-ms=off".to_string());
            }

            if let Some(ref pass) = config.password {
                args.push(format!("-p{pass}"));
                if config.obfuscate {
                    args.push("-mhe=on".to_string());
                }
            }

            if let Some(split_mb) = config.split_mb {
                args.push(format!("-v{split_mb}m"));
            }

            args.push(out_name);
            for f in input_files {
                if let Some(name) = f.file_name().and_then(|n| n.to_str()) {
                    args.push(name.to_string());
                }
            }
            ("7z".to_string(), args)
        }
        CompressFmt::Zip => {
            let out_name = format!("{archive_base}.zip");
            args.push("a".to_string());
            args.push("-tzip".to_string());
            args.push(format!("-mx={}", config.level.clamp(1, 9)));
            args.push(format!("-mmt={threads}"));

            if let Some(ref pass) = config.password {
                args.push(format!("-p{pass}"));
            }

            if let Some(split_mb) = config.split_mb {
                args.push(format!("-v{split_mb}m"));
            }

            args.push(out_name);
            for f in input_files {
                if let Some(name) = f.file_name().and_then(|n| n.to_str()) {
                    args.push(name.to_string());
                }
            }
            ("7z".to_string(), args)
        }
        CompressFmt::Rar => {
            let out_name = format!("{archive_base}.rar");
            args.push("a".to_string());
            args.push(format!("-m{}", config.level.clamp(1, 9)));
            args.push(format!("-mt{threads}"));

            if config.solid {
                args.push("-s".to_string());
            } else {
                args.push("-s-".to_string());
            }

            if let Some(ref pass) = config.password {
                if config.obfuscate {
                    args.push(format!("-hp{pass}"));
                } else {
                    args.push(format!("-p{pass}"));
                }
            }

            if let Some(split_mb) = config.split_mb {
                args.push(format!("-v{split_mb}m"));
            }

            args.push(out_name);
            for f in input_files {
                if let Some(name) = f.file_name().and_then(|n| n.to_str()) {
                    args.push(name.to_string());
                }
            }
            ("rar".to_string(), args)
        }
        CompressFmt::Zstd => {
            // zstd آرشیوساز نیست، فقط یک استریم را فشرده می‌کند؛ پس tar بسته‌بندی
            // می‌کند و با `-I` خودش zstd را spawn می‌کند. نتیجه همان یک child است،
            // بدون shell و بدون لولهٔ دستی، پس حلقهٔ cancel و `getrusage` بالا
            // بدون تغییر کار می‌کنند.
            // ponytail: بدون `--long`. windowLog بزرگ‌تر از ۲۷ موقع باز کردن به
            // `--memory` نیاز دارد و کاربر با ابزار دیگر خطا می‌گیرد. لازم شد،
            // همراه راهنمای باز کردن اضافه شود.
            let out_name = format!("{archive_base}.tar.zst");
            args.push("-I".to_string());
            args.push(format!("zstd -{} -T{threads}", config.level.clamp(1, 19)));
            args.push("-cf".to_string());
            args.push(out_name);
            for f in input_files {
                if let Some(name) = f.file_name().and_then(|n| n.to_str()) {
                    args.push(name.to_string());
                }
            }
            ("tar".to_string(), args)
        }
    }
}

fn collect_outputs(work_dir: &Path, archive_base: &str, config: &CompressConfig) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(work_dir) else {
        return files;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_match = match config.fmt {
            CompressFmt::SevenZ => {
                fname == format!("{archive_base}.7z")
                    || (fname.starts_with(&format!("{archive_base}.7z."))
                        && fname != format!("{archive_base}.7z"))
            }
            CompressFmt::Zip => {
                fname == format!("{archive_base}.zip")
                    || fname.starts_with(&format!("{archive_base}.zip."))
                    || fname.starts_with(&format!("{archive_base}.z"))
            }
            CompressFmt::Rar => {
                fname == format!("{archive_base}.rar")
                    || fname.starts_with(&format!("{archive_base}.part"))
                    || fname.starts_with(&format!("{archive_base}.r"))
            }
            CompressFmt::Zstd => fname == format!("{archive_base}.tar.zst"),
        };

        if is_match {
            files.push(path);
        }
    }

    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    files
}

fn get_children_cpu_time() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut usage) } == 0 {
        let user = usage.ru_utime.tv_sec as f64 + usage.ru_utime.tv_usec as f64 / 1e6;
        let sys = usage.ru_stime.tv_sec as f64 + usage.ru_stime.tv_usec as f64 / 1e6;
        user + sys
    } else {
        0.0
    }
}

fn pin_pid_to_cores(pid: u32, cores: &[i32], trace_id: u64) {
    if cores.is_empty() {
        return;
    }
    let cores_str = cores
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let status = std::process::Command::new("taskset")
        .arg("-cp")
        .arg(&cores_str)
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status();
    log_ev!("filecompress", trace_id, "pin_pid", "pid" => pid, "cores" => &cores_str, "status" => status.is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command_7z() {
        let cfg = CompressConfig {
            fmt: CompressFmt::SevenZ,
            algo: CompressAlgo::Lzma2,
            level: 5,
            password: Some("secret".into()),
            split_mb: Some(100),
            obfuscate: true,
            solid: true,
        };
        let inputs = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];
        let (cmd, args) = build_command("archive", &cfg, &inputs, &[0, 1]);
        assert_eq!(cmd, "7z");
        assert!(args.contains(&"-t7z".to_string()));
        assert!(args.contains(&"-mx=5".to_string()));
        assert!(args.contains(&"-mmt=2".to_string()));
        assert!(args.contains(&"-m0=lzma2".to_string()));
        assert!(args.contains(&"-ms=on".to_string()));
        assert!(args.contains(&"-psecret".to_string()));
        assert!(args.contains(&"-mhe=on".to_string()));
        assert!(args.contains(&"-v100m".to_string()));
    }

    #[test]
    fn test_build_command_rar() {
        let cfg = CompressConfig {
            fmt: CompressFmt::Rar,
            algo: CompressAlgo::Lzma2,
            level: 3,
            password: Some("123".into()),
            split_mb: None,
            obfuscate: false,
            solid: false,
        };
        let inputs = vec![PathBuf::from("doc.pdf")];
        let (cmd, args) = build_command("archive", &cfg, &inputs, &[]);
        assert_eq!(cmd, "rar");
        assert!(args.contains(&"-m3".to_string()));
        assert!(args.contains(&"-p123".to_string()));
        assert!(args.contains(&"-s-".to_string()));
    }

    #[test]
    fn test_build_command_zstd() {
        // ورودی چندتایی و رمز/پارت روشن: خروجی باید tar باشد و آرگومان رمز یا
        // چندجلدی به هیچ شکل به فرمان نرود — zstd هیچ‌کدام را ندارد.
        let cfg = CompressConfig {
            fmt: CompressFmt::Zstd,
            algo: CompressAlgo::Lzma2,
            level: 19,
            password: Some("secret".into()),
            split_mb: Some(100),
            obfuscate: true,
            solid: true,
        };
        let inputs = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];
        let (cmd, args) = build_command("archive", &cfg, &inputs, &[0, 1, 2, 3]);
        assert_eq!(cmd, "tar");
        assert_eq!(
            args,
            vec![
                "-I",
                "zstd -19 -T4",
                "-cf",
                "archive.tar.zst",
                "a.txt",
                "b.txt"
            ]
        );
        assert!(!args.iter().any(|a| a.contains("secret")));
    }

    #[test]
    fn test_collect_outputs_zstd() {
        let dir = std::env::temp_dir().join(format!("fc_zstd_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["archive.tar.zst", "a.txt", "archive.tar"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let cfg = CompressConfig {
            fmt: CompressFmt::Zstd,
            ..CompressConfig::default()
        };
        let out = collect_outputs(&dir, "archive", &cfg);
        assert_eq!(out.len(), 1);
        assert!(out[0].ends_with("archive.tar.zst"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
