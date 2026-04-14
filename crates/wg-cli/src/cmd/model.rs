//! `wg model` — model management commands.
//!
//! Provides status, listing, and download helpers for the configured
//! embedding/semantic model.

use bpaf::*;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};

use crate::cmd::Command;
use wg_core::{Config, WgError};

#[derive(Debug, Clone)]
pub enum ModelSub {
    List,
    Status { name: String },
    Download { name: String },
}

pub fn model_command() -> impl Parser<Command> {
    let list = pure(ModelSub::List)
        .to_options()
        .command("list")
        .help("List model artifacts in the configured download/cache directories");

    let name = positional::<String>("NAME").help("Model name or Hugging Face repo id");
    let status = construct!(ModelSub::Status { name })
        .to_options()
        .command("status")
        .help("Show model download/status information");

    let name = positional::<String>("NAME").help("Model name or Hugging Face repo id");
    let download = construct!(ModelSub::Download { name })
        .to_options()
        .command("download")
        .help("Download a model using Hugging Face tooling");

    construct!([list, status, download])
        .map(Command::Model)
        .to_options()
        .command("model")
        .help("Model management")
}

pub fn run_model(config: Config, sub: ModelSub) -> Result<String, WgError> {
    match sub {
        ModelSub::List => model_list(&config),
        ModelSub::Status { name } => model_status(&config, &name),
        ModelSub::Download { name } => model_download(&config, &name),
    }
}

fn model_list(config: &Config) -> Result<String, WgError> {
    let download_dir = expand_path(&config.model.download_dir);
    let cache_dir = expand_path(&config.model.cache_dir);

    let mut out = String::new();
    out.push_str(&format!("Configured model: {}\n", config.model.name));
    out.push_str(&format!("Download dir: {}\n", download_dir.display()));
    out.push_str(&format!("Cache dir: {}\n", cache_dir.display()));

    let downloads = discover_model_dirs(&download_dir)?;
    let cached = discover_model_dirs(&cache_dir)?;

    out.push_str("\nDownloaded models:\n");
    if downloads.is_empty() {
        out.push_str("  (none found)\n");
    } else {
        for entry in downloads {
            let (files, bytes) = dir_stats(&entry.path)?;
            out.push_str(&format!(
                "  - {}  [{} files, {}]\n",
                entry.display_name(),
                files,
                format_bytes(bytes)
            ));
        }
    }

    out.push_str("\nCache entries:\n");
    if cached.is_empty() {
        out.push_str("  (none found)\n");
    } else {
        for entry in cached {
            let (files, bytes) = dir_stats(&entry.path)?;
            out.push_str(&format!(
                "  - {}  [{} files, {}]\n",
                entry.display_name(),
                files,
                format_bytes(bytes)
            ));
        }
    }

    Ok(out.trim_end().to_string())
}

fn model_status(config: &Config, name: &str) -> Result<String, WgError> {
    let download_dir = expand_path(&config.model.download_dir);
    let cache_dir = expand_path(&config.model.cache_dir);
    let download_path = download_dir.join(name);

    let local_present = has_model_artifacts(&download_path);
    let cache_present = discover_model_dirs(&cache_dir)?
        .into_iter()
        .any(|entry| model_name_matches(name, &entry.path, &cache_dir));

    let mut out = String::new();
    out.push_str(&format!("Model: {}\n", name));
    out.push_str(&format!("Configured download dir: {}\n", download_dir.display()));
    out.push_str(&format!("Configured cache dir: {}\n", cache_dir.display()));
    out.push_str(&format!("Download path: {}\n", download_path.display()));
    out.push_str(&format!("Downloaded: {}\n", if local_present { "yes" } else { "no" }));
    out.push_str(&format!("Cached: {}\n", if cache_present { "yes" } else { "no" }));

    if local_present {
        let (files, bytes) = dir_stats(&download_path)?;
        out.push_str(&format!("Files: {}\n", files));
        out.push_str(&format!("Size: {}\n", format_bytes(bytes)));
    }

    if !local_present && !cache_present {
        out.push_str("Hint: run `wg model download <name>` to fetch it.");
    }

    Ok(out.trim_end().to_string())
}

fn model_download(config: &Config, name: &str) -> Result<String, WgError> {
    let download_dir = expand_path(&config.model.download_dir);
    let cache_dir = expand_path(&config.model.cache_dir);
    let download_path = download_dir.join(name);

    if has_model_artifacts(&download_path) {
        return Ok(format!(
            "Model '{}' is already present at {}",
            name,
            download_path.display()
        ));
    }

    fs::create_dir_all(&download_dir).map_err(|e| model_io_error("create download dir", &download_dir, e))?;
    fs::create_dir_all(&cache_dir).map_err(|e| model_io_error("create cache dir", &cache_dir, e))?;

    let mut last_error: Option<WgError> = None;
    for bin in ["hf", "huggingface-cli"] {
        match run_huggingface_download(bin, name, &download_path, &cache_dir) {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let mut out = format!(
                        "Downloaded '{}' to {} using {}",
                        name,
                        download_path.display(),
                        bin
                    );
                    if !stdout.is_empty() {
                        out.push_str("\n");
                        out.push_str(&stdout);
                    }
                    if !stderr.is_empty() {
                        out.push_str("\n");
                        out.push_str(&stderr);
                    }
                    return Ok(out);
                }

                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let message = if stderr.is_empty() { stdout } else { stderr };
                let err = io::Error::new(
                    io::ErrorKind::Other,
                    format!("{} exited with status {}: {}", bin, output.status, message),
                );
                return Err(WgError::ModelDownloadFailed {
                    name: name.to_string(),
                    source: Box::new(err),
                });
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                last_error = Some(WgError::ModelDownloadFailed {
                    name: name.to_string(),
                    source: Box::new(e),
                });
                continue;
            }
            Err(e) => {
                return Err(WgError::ModelDownloadFailed {
                    name: name.to_string(),
                    source: Box::new(e),
                });
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        WgError::InvalidInput(
            "Neither `hf` nor `huggingface-cli` was found on PATH. Install one of them to download models.".to_string(),
        )
    }))
}

fn run_huggingface_download(
    bin: &str,
    name: &str,
    download_path: &Path,
    cache_dir: &Path,
) -> io::Result<std::process::Output> {
    ProcessCommand::new(bin)
        .arg("download")
        .arg(name)
        .arg("--local-dir")
        .arg(download_path)
        .env("HF_HOME", cache_dir)
        .env("HF_HUB_CACHE", cache_dir)
        .env("HF_DATASETS_CACHE", cache_dir)
        .stdin(Stdio::null())
        .output()
}

#[derive(Debug, Clone)]
struct ModelDirEntry {
    path: PathBuf,
}

impl ModelDirEntry {
    fn display_name(&self) -> String {
        self.path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/")
    }
}

fn discover_model_dirs(root: &Path) -> Result<Vec<ModelDirEntry>, WgError> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();

    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }

        if !is_model_marker(entry.file_name().to_string_lossy().as_ref()) {
            continue;
        }

        if let Some(parent) = entry.path().parent() {
            let parent = parent.to_path_buf();
            if seen.insert(parent.clone()) {
                entries.push(ModelDirEntry { path: parent });
            }
        }
    }

    entries.sort_by(|a, b| a.display_name().cmp(&b.display_name()));
    Ok(entries)
}

fn is_model_marker(file_name: &str) -> bool {
    matches!(
        file_name,
        "config.json"
            | "model.safetensors"
            | "pytorch_model.bin"
            | "tokenizer.json"
            | "tokenizer.model"
            | "sentencepiece.bpe.model"
    )
}

fn has_model_artifacts(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }

    discover_model_dirs(path)
        .map(|entries| !entries.is_empty())
        .unwrap_or(false)
}

fn model_name_matches(name: &str, path: &Path, root: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let rel_name = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/");

    let normalized = normalize_model_name(name);
    rel_name == name || rel_name == normalized || rel_name.contains(&normalized) || path.ends_with(name)
}

fn normalize_model_name(name: &str) -> String {
    name.replace('/', "--")
}

fn dir_stats(path: &Path) -> Result<(usize, u64), WgError> {
    if !path.exists() {
        return Ok((0, 0));
    }

    let mut files = 0usize;
    let mut bytes = 0u64;

    for entry in walkdir::WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            files += 1;
            bytes += entry
                .metadata()
                .map_err(|e| WgError::Internal(format!("failed to read metadata for {}: {}", entry.path().display(), e)))?
                .len();
        }
    }

    Ok((files, bytes))
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0usize;

    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

fn expand_path(raw: &str) -> PathBuf {
    if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(stripped);
        }
    } else if raw == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }

    PathBuf::from(raw)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn model_io_error(action: &str, path: &Path, source: io::Error) -> WgError {
    WgError::Internal(format!("failed to {} {}: {}", action, path.display(), source))
}
