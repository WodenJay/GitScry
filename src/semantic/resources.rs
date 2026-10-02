use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::app::AppError;

const REPOSITORY: &str = "Qdrant/all-MiniLM-L6-v2-onnx";
pub(crate) const MODEL_SHA256: &str =
    "bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5";
pub(crate) const TOKENIZER_SHA256: &str =
    "59f410da6d9dad2025f0e53b6c45554a3b3a0a5c574927f201e99d0217c1a26b";
pub(crate) const REVISION: &str = "8f518e882455312b086101e60691f5e6e2f05c3c";
const RESOURCES: [Resource; 5] = [
    Resource {
        path: "model.onnx",
        bytes: 90_387_630,
        sha256: MODEL_SHA256,
    },
    Resource {
        path: "tokenizer.json",
        bytes: 711_661,
        sha256: TOKENIZER_SHA256,
    },
    Resource {
        path: "tokenizer_config.json",
        bytes: 1_412,
        sha256: "abda01c8c14c5151ae498aceb30db406d6b91242c394fd88d3b6fd5a63a101e6",
    },
    Resource {
        path: "special_tokens_map.json",
        bytes: 695,
        sha256: "5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a",
    },
    Resource {
        path: "config.json",
        bytes: 650,
        sha256: "1b4d8e2a3988377ed8b519a31d8d31025a25f1c5f8606998e8014111438efcd7",
    },
];

struct Resource {
    path: &'static str,
    bytes: u64,
    sha256: &'static str,
}

pub(crate) struct Assets {
    pub(crate) model: PathBuf,
    pub(crate) tokenizer: PathBuf,
}

pub(crate) fn ensure() -> Result<Assets, AppError> {
    let directory = model_directory()?;
    fs::create_dir_all(&directory).map_err(|error| {
        AppError::operational(format!(
            "error: creating semantic model directory `{}`: {error}",
            directory.display()
        ))
    })?;
    let _lock = acquire_download_lock(&directory)?;

    for resource in &RESOURCES {
        ensure_resource(&directory, resource)?;
    }

    Ok(Assets {
        model: directory.join("model.onnx"),
        tokenizer: directory.join("tokenizer.json"),
    })
}
pub(crate) fn existing() -> Result<Assets, AppError> {
    let directory = model_directory()?;
    for resource in &RESOURCES {
        let path = directory.join(resource.path);
        if !is_valid(&path, resource) {
            return Err(AppError::operational(format!(
                "error: pinned semantic model resource `{}` is missing or invalid at `{}`; run `gitscry index --semantic` while online to install or repair it",
                resource.path,
                path.display()
            )));
        }
    }
    Ok(Assets {
        model: directory.join("onnx/model.onnx"),
        tokenizer: directory.join("tokenizer.json"),
    })
}

pub(crate) fn ensure_tokenizer() -> Result<PathBuf, AppError> {
    let directory = model_directory()?;
    fs::create_dir_all(&directory).map_err(|error| {
        AppError::operational(format!(
            "error: creating semantic model directory `{}`: {error}",
            directory.display()
        ))
    })?;
    let _lock = acquire_download_lock(&directory)?;
    for resource in RESOURCES
        .iter()
        .filter(|resource| resource.path != "onnx/model.onnx")
    {
        ensure_resource(&directory, resource)?;
    }
    Ok(directory.join("tokenizer.json"))
}

fn model_directory() -> Result<PathBuf, AppError> {
    let base = if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA").map(PathBuf::from).or_else(|| {
            env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .map(|home| home.join("AppData").join("Local"))
        })
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library").join("Caches"))
    } else {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".cache"))
            })
    }
    .ok_or_else(|| {
        AppError::operational(
            "error: cannot locate the per-user cache directory for semantic model files",
        )
    })?;
    Ok(base.join("GitScry").join("semantic").join(REVISION))
}

fn acquire_download_lock(directory: &Path) -> Result<File, AppError> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("download.lock"))
        .map_err(|error| {
            AppError::operational(format!(
                "error: opening semantic model download lock: {error}"
            ))
        })?;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(25)),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(AppError::operational(format!(
                    "error: locking semantic model files: {error}"
                )));
            }
        }
    }
}

fn ensure_resource(directory: &Path, resource: &Resource) -> Result<(), AppError> {
    let destination = directory.join(resource.path);
    if is_valid(&destination, resource) {
        return Ok(());
    }
    download(&destination, resource)?;
    if !is_valid(&destination, resource) {
        return Err(AppError::operational(format!(
            "error: pinned semantic model resource `{}` failed verification at `{}`",
            resource.path,
            destination.display()
        )));
    }
    Ok(())
}

fn is_valid(path: &Path, resource: &Resource) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() != resource.bytes {
        return false;
    }
    hash_file(path).is_ok_and(|hash| hash == resource.sha256)
}

fn download(destination: &Path, resource: &Resource) -> Result<(), AppError> {
    let parent = destination.parent().ok_or_else(|| {
        AppError::operational("error: semantic model resource has no parent directory")
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        AppError::operational(format!(
            "error: creating semantic model resource directory `{}`: {error}",
            parent.display()
        ))
    })?;
    let url = format!(
        "https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{}",
        resource.path
    );
    let mut response = ureq::get(&url).call().map_err(|error| {
        AppError::operational(format!(
            "error: downloading semantic model resource `{}` from Hugging Face: {error}; retry while online",
            resource.path
        ))
    })?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
        AppError::operational(format!(
            "error: creating temporary semantic model file: {error}"
        ))
    })?;
    let mut body = response
        .body_mut()
        .as_reader()
        .take(resource.bytes.saturating_add(1));
    let received = io::copy(&mut body, temporary.as_file_mut()).map_err(|error| {
        AppError::operational(format!(
            "error: receiving semantic model resource `{}`: {error}",
            resource.path
        ))
    })?;
    if received != resource.bytes {
        return Err(AppError::operational(format!(
            "error: semantic model resource `{}` has {received} bytes; expected {}",
            resource.path, resource.bytes
        )));
    }
    temporary.as_file_mut().sync_all().map_err(|error| {
        AppError::operational(format!(
            "error: flushing semantic model resource `{}`: {error}",
            resource.path
        ))
    })?;
    let hash = hash_file(temporary.path()).map_err(|error| {
        AppError::operational(format!(
            "error: verifying semantic model resource `{}`: {error}",
            resource.path
        ))
    })?;
    if hash != resource.sha256 {
        return Err(AppError::operational(format!(
            "error: semantic model resource `{}` has SHA-256 {hash}; expected {}",
            resource.path, resource.sha256
        )));
    }

    if destination.exists() {
        fs::remove_file(destination).map_err(|error| {
            AppError::operational(format!(
                "error: replacing invalid semantic model resource `{}`: {error}",
                destination.display()
            ))
        })?;
    }
    temporary.persist(destination).map_err(|error| {
        AppError::operational(format!(
            "error: publishing semantic model resource `{}`: {}",
            destination.display(),
            error.error
        ))
    })?;
    Ok(())
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn encoder_fingerprint() -> String {
    let mut hasher = Sha256::new();
    hasher.update(
        b"gitscry-semantic-encoder-v2;title=64;paths=32;total=256;field-prefix-chars=4096;path-separator=newline;right-pad;token-types=zero;mask-mean-f32-l2;dimension=384",
    );
    hasher.update(REVISION.as_bytes());
    for resource in &RESOURCES {
        hasher.update(resource.path.as_bytes());
        hasher.update(resource.bytes.to_le_bytes());
        hasher.update(resource.sha256.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}
