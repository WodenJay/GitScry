use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path};
use std::process;

use flate2::read::GzDecoder;
use gitscry::runtime::{
    APP_VERSION, PinnedFile, RuntimeManifest, RuntimePins, RuntimeTargetPins,
    install_verified_package, resolve_installation,
};
use sha2::{Digest, Sha256};
use tempfile::Builder;
use zip::ZipArchive;

const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const USER_AGENT: &str = "GitScry-runtime-packager";

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("prepare-runtime") || args.next().is_some() {
        eprintln!("usage: cargo run --manifest-path xtask/Cargo.toml -- prepare-runtime");
        process::exit(2);
    }

    if let Err(error) = prepare_runtime() {
        eprintln!("failed to prepare ONNX Runtime: {error}");
        process::exit(1);
    }
}

fn prepare_runtime() -> Result<(), Box<dyn Error>> {
    let target = native_target().ok_or("no pinned ONNX Runtime package for this native target")?;
    let pins = RuntimePins::pinned()?;
    let target_pins = pins
        .targets
        .get(target)
        .ok_or("no pinned ONNX Runtime package for this native target")?;
    let archive = download_archive(target_pins)?;
    let files = extract_files(&archive, target_pins)?;
    let manifest = RuntimeManifest::from_pins(APP_VERSION, target, &pins)?;

    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask manifest has no project parent")?;
    let runtime_root = source_root.join("runtime");
    let staging = Builder::new()
        .prefix(".onnxruntime-package-")
        .tempdir_in(source_root)?;
    stage_runtime_package(staging.path(), &manifest, &files)?;
    install_verified_package(staging.path(), source_root, APP_VERSION, target)?;
    resolve_installation(source_root, APP_VERSION, target, &pins)?;
    for install_dir in ["debug", "debug/deps", "release"] {
        let install_dir = source_root.join("target").join(install_dir);
        install_verified_package(staging.path(), &install_dir, APP_VERSION, target)?;
        resolve_installation(&install_dir, APP_VERSION, target, &pins)?;
    }
    println!(
        "Verified GitScry {APP_VERSION} ONNX Runtime {} (binding {}, API {}) for {target}; archive SHA-256 {}, runtime ID {}, installed at {}",
        manifest.runtime_version,
        manifest.binding_version,
        manifest.api_version,
        manifest.upstream_archive_sha256,
        manifest.runtime_id,
        runtime_root.display()
    );
    Ok(())
}

fn native_target() -> Option<&'static str> {
    match (
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_env = "gnu"),
    ) {
        ("windows", "x86_64", _) => Some("x86_64-pc-windows-msvc"),
        ("linux", "x86_64", true) => Some("x86_64-unknown-linux-gnu"),
        ("macos", "aarch64", _) => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

fn download_archive(target: &RuntimeTargetPins) -> Result<Vec<u8>, Box<dyn Error>> {
    if target.archive_bytes > MAX_ARCHIVE_BYTES {
        return Err("pinned ONNX Runtime archive exceeds the download limit".into());
    }
    let mut response = ureq::get(&target.archive_url)
        .header("User-Agent", USER_AGENT)
        .call()?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_ARCHIVE_BYTES)
        .read_to_vec()?;
    if bytes.len() as u64 != target.archive_bytes {
        return Err(format!(
            "ONNX Runtime archive has {} bytes; expected {}",
            bytes.len(),
            target.archive_bytes
        )
        .into());
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != target.archive_sha256 {
        return Err(format!(
            "ONNX Runtime archive has SHA-256 {digest}; expected {}",
            target.archive_sha256
        )
        .into());
    }
    Ok(bytes)
}

fn extract_files(
    archive_bytes: &[u8],
    target: &RuntimeTargetPins,
) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let expected = target
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.archive_path.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut files = vec![None; target.files.len()];
    match target.archive_format.as_str() {
        "tar" => {
            let decoder = GzDecoder::new(Cursor::new(archive_bytes));
            let mut archive = tar::Archive::new(decoder);
            for entry in archive.entries()? {
                let mut entry = entry?;
                let archive_path = entry.path()?;
                let Some(path) = normalize_archive_path(&archive_path) else {
                    continue;
                };
                let Some(index) = expected.get(path.as_str()).copied() else {
                    continue;
                };
                if !entry.header().entry_type().is_file() {
                    return Err(format!(
                        "pinned ONNX Runtime member is not a regular file: {path}"
                    )
                    .into());
                }
                if entry.size() != target.files[index].bytes {
                    return Err(format!(
                        "pinned ONNX Runtime member has an unexpected size: {path}"
                    )
                    .into());
                }
                let mut contents = Vec::with_capacity(target.files[index].bytes as usize);
                entry.read_to_end(&mut contents)?;
                store_verified_file(&mut files, index, contents, &target.files[index])?;
            }
        }
        "zip" => {
            let mut archive = ZipArchive::new(Cursor::new(archive_bytes))?;
            for entry_index in 0..archive.len() {
                let mut entry = archive.by_index(entry_index)?;
                let Some(index) = expected.get(entry.name()).copied() else {
                    continue;
                };
                if !entry.is_file() {
                    return Err(format!(
                        "pinned ONNX Runtime member is not a regular file: {}",
                        entry.name()
                    )
                    .into());
                }
                if entry.size() != target.files[index].bytes {
                    return Err(format!(
                        "pinned ONNX Runtime member has an unexpected size: {}",
                        entry.name()
                    )
                    .into());
                }
                let mut contents = Vec::with_capacity(target.files[index].bytes as usize);
                entry.read_to_end(&mut contents)?;
                store_verified_file(&mut files, index, contents, &target.files[index])?;
            }
        }
        format => return Err(format!("unsupported pinned archive format: {format}").into()),
    }

    files
        .into_iter()
        .enumerate()
        .map(|(index, contents)| {
            contents.ok_or_else(|| {
                format!(
                    "ONNX Runtime archive is missing pinned member {}",
                    target.files[index].archive_path
                )
                .into()
            })
        })
        .collect()
}

fn normalize_archive_path(path: &Path) -> Option<String> {
    let mut normalized = String::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str(segment.to_str()?);
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

fn store_verified_file(
    files: &mut [Option<Vec<u8>>],
    index: usize,
    contents: Vec<u8>,
    expected: &PinnedFile,
) -> Result<(), Box<dyn Error>> {
    if files[index].is_some() {
        return Err(format!(
            "ONNX Runtime archive repeats pinned member {}",
            expected.archive_path
        )
        .into());
    }
    let digest = format!("{:x}", Sha256::digest(&contents));
    if contents.len() as u64 != expected.bytes || digest != expected.sha256 {
        return Err(format!(
            "ONNX Runtime member {} failed size or SHA-256 verification",
            expected.archive_path
        )
        .into());
    }
    files[index] = Some(contents);
    Ok(())
}

fn stage_runtime_package(
    package_root: &Path,
    manifest: &RuntimeManifest,
    files: &[Vec<u8>],
) -> Result<(), Box<dyn Error>> {
    if files.len() != manifest.files.len() {
        return Err("ONNX Runtime extraction returned an unexpected file count".into());
    }
    let runtime_root = package_root.join("runtime");
    let runtime_dir = runtime_root.join(&manifest.runtime_id);
    fs::create_dir_all(&runtime_dir)?;
    for (file, contents) in manifest.files.iter().zip(files) {
        fs::write(runtime_dir.join(&file.name), contents)?;
    }
    let manifests_dir = runtime_root.join("manifests");
    fs::create_dir_all(&manifests_dir)?;
    fs::write(
        manifests_dir.join(format!("{}.json", manifest.app_version)),
        serde_json::to_vec_pretty(manifest)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_archive_path;
    use std::path::Path;

    #[test]
    fn normalizes_tar_members_with_leading_current_directory() {
        let member = "onnxruntime-osx-arm64-1.23.2/lib/libonnxruntime.1.23.2.dylib";
        assert_eq!(
            normalize_archive_path(Path::new(&format!("./{member}"))).as_deref(),
            Some(member)
        );
    }

    #[test]
    fn rejects_archive_paths_that_escape_the_member_root() {
        assert_eq!(
            normalize_archive_path(Path::new("../libonnxruntime.dylib")),
            None
        );
        assert_eq!(
            normalize_archive_path(Path::new("/libonnxruntime.dylib")),
            None
        );
    }
}
