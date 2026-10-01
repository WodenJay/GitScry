use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::{Builder, NamedTempFile};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PINNED_RUNTIME: &str = include_str!("../../config/onnxruntime.json");

/// The GitScry package version used to identify a packaged runtime.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_RUNTIME_FILES: usize = 32;
const MAX_RUNTIME_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePins {
    pub version: String,
    pub binding_version: String,
    pub api_version: u32,
    pub targets: BTreeMap<String, RuntimeTargetPins>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTargetPins {
    pub archive_url: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub archive_format: String,
    pub library: String,
    pub files: Vec<PinnedFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedFile {
    pub archive_path: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeManifest {
    pub schema_version: u32,
    pub app_version: String,
    pub target: String,
    pub runtime_id: String,
    pub runtime_version: String,
    pub binding_version: String,
    pub api_version: u32,
    pub library: String,
    pub library_sha256: String,
    pub upstream_archive_sha256: String,
    pub files: Vec<RuntimeFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug)]
pub struct RuntimeArtifact {
    pub library_path: PathBuf,
    pub runtime_id: String,
    pub runtime_version: String,
    pub binding_version: String,
    pub api_version: u32,
    pub library_sha256: String,
}

impl RuntimePins {
    pub fn pinned() -> Result<Self, String> {
        Self::parse(PINNED_RUNTIME)
    }

    pub fn parse(contents: &str) -> Result<Self, String> {
        serde_json::from_str(contents)
            .map_err(|error| format!("invalid ONNX Runtime pins: {error}"))
    }
}

impl RuntimeManifest {
    pub fn from_pins(app_version: &str, target: &str, pins: &RuntimePins) -> Result<Self, String> {
        let target_pins = pins
            .targets
            .get(target)
            .ok_or_else(|| format!("no pinned ONNX Runtime for target {target}"))?;
        let library = target_pins
            .files
            .iter()
            .find(|file| file.name == target_pins.library)
            .ok_or_else(|| format!("pinned ONNX Runtime library is absent for {target}"))?;
        let manifest = Self {
            schema_version: 1,
            app_version: app_version.to_owned(),
            target: target.to_owned(),
            runtime_id: runtime_id(&target_pins.archive_sha256),
            runtime_version: pins.version.clone(),
            binding_version: pins.binding_version.clone(),
            api_version: pins.api_version,
            library: target_pins.library.clone(),
            library_sha256: library.sha256.clone(),
            upstream_archive_sha256: target_pins.archive_sha256.clone(),
            files: target_pins
                .files
                .iter()
                .map(|file| RuntimeFile {
                    name: file.name.clone(),
                    bytes: file.bytes,
                    sha256: file.sha256.clone(),
                })
                .collect(),
        };
        manifest.validate_pinned(app_version, target, pins)?;
        Ok(manifest)
    }

    pub fn validate_pinned(
        &self,
        app_version: &str,
        target: &str,
        pins: &RuntimePins,
    ) -> Result<(), String> {
        self.validate_shape(app_version, target)?;
        let target_pins = pins
            .targets
            .get(target)
            .ok_or_else(|| format!("no pinned ONNX Runtime for target {target}"))?;
        let library = target_pins
            .files
            .iter()
            .find(|file| file.name == target_pins.library)
            .ok_or_else(|| format!("pinned ONNX Runtime library is absent for {target}"))?;

        if self.runtime_version != pins.version
            || self.binding_version != pins.binding_version
            || self.api_version != pins.api_version
            || self.library != target_pins.library
            || self.library_sha256 != library.sha256
            || self.upstream_archive_sha256 != target_pins.archive_sha256
            || self.runtime_id != runtime_id(&target_pins.archive_sha256)
        {
            return Err("packaged ONNX Runtime identity does not match the pinned runtime".into());
        }

        if self.files.len() != target_pins.files.len()
            || target_pins.files.iter().any(|pinned| {
                !self.files.iter().any(|file| {
                    file.name == pinned.name
                        && file.bytes == pinned.bytes
                        && file.sha256 == pinned.sha256
                })
            })
        {
            return Err("packaged ONNX Runtime file list does not match the pinned runtime".into());
        }
        Ok(())
    }

    fn validate_shape(&self, app_version: &str, target: &str) -> Result<(), String> {
        if self.schema_version != 1
            || self.app_version != app_version
            || self.target != target
            || semver::Version::parse(&self.app_version).is_err()
            || semver::Version::parse(&self.runtime_version).is_err()
            || semver::Version::parse(&self.binding_version).is_err()
            || self.api_version == 0
            || !is_sha256(&self.upstream_archive_sha256)
            || !is_sha256(&self.library_sha256)
            || !is_safe_filename(&self.library)
            || self.files.is_empty()
            || self.files.len() > MAX_RUNTIME_FILES
        {
            return Err("packaged ONNX Runtime manifest has invalid identity or metadata".into());
        }
        if self.runtime_id != runtime_id(&self.upstream_archive_sha256) {
            return Err(
                "packaged ONNX Runtime ID does not match its upstream archive digest".into(),
            );
        }

        let mut names = std::collections::BTreeSet::new();
        let mut total_bytes = 0_u64;
        let mut library_found = false;
        for file in &self.files {
            if !is_safe_filename(&file.name)
                || !is_sha256(&file.sha256)
                || file.bytes == 0
                || !names.insert(file.name.as_str())
            {
                return Err(
                    "packaged ONNX Runtime manifest contains an invalid file record".into(),
                );
            }
            total_bytes = total_bytes
                .checked_add(file.bytes)
                .ok_or_else(|| "packaged ONNX Runtime file sizes overflow".to_owned())?;
            if file.name == self.library {
                library_found = file.sha256 == self.library_sha256;
            }
        }
        if total_bytes > MAX_RUNTIME_BYTES || !library_found {
            return Err("packaged ONNX Runtime manifest has an invalid library or size".into());
        }
        Ok(())
    }
}

pub fn resolve_installation(
    install_dir: &Path,
    app_version: &str,
    target: &str,
    pins: &RuntimePins,
) -> Result<RuntimeArtifact, String> {
    let runtime_root = install_dir.join("runtime");
    let manifests_dir = runtime_root.join("manifests");
    require_directory(&runtime_root)?;
    require_directory(&manifests_dir)?;
    let manifest_path = manifests_dir.join(format!("{app_version}.json"));
    require_regular_file(&manifest_path)?;
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| format!("could not read packaged ONNX Runtime manifest: {error}"))?;
    let manifest: RuntimeManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid packaged ONNX Runtime manifest: {error}"))?;
    manifest.validate_pinned(app_version, target, pins)?;

    let runtime_dir = runtime_root.join(&manifest.runtime_id);
    require_directory(&runtime_dir)?;
    verify_runtime_files(&runtime_dir, &manifest)?;
    Ok(RuntimeArtifact {
        library_path: runtime_dir.join(&manifest.library),
        runtime_id: manifest.runtime_id,
        runtime_version: manifest.runtime_version,
        binding_version: manifest.binding_version,
        api_version: manifest.api_version,
        library_sha256: manifest.library_sha256,
    })
}

#[doc(hidden)]
pub fn install_verified_package(
    source_install_dir: &Path,
    destination_install_dir: &Path,
    app_version: &str,
    target: &str,
) -> Result<(), String> {
    // Keep package verification independent of this release's pins: an updater may carry older pins.
    // The new executable enforces its exact pins before loading.
    let source_runtime_root = source_install_dir.join("runtime");
    require_directory(&source_runtime_root)?;
    let source_manifest = source_runtime_root
        .join("manifests")
        .join(format!("{app_version}.json"));
    require_regular_file(&source_manifest)?;
    let manifest_bytes = fs::read(&source_manifest).map_err(|error| {
        format!(
            "could not read staged ONNX Runtime manifest {}: {error}",
            source_manifest.display()
        )
    })?;
    let manifest: RuntimeManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid staged ONNX Runtime manifest: {error}"))?;
    manifest.validate_shape(app_version, target)?;
    let source_runtime = source_runtime_root.join(&manifest.runtime_id);
    require_directory(&source_runtime)?;
    verify_runtime_files(&source_runtime, &manifest)?;

    let destination_runtime_root = destination_install_dir.join("runtime");
    fs::create_dir_all(&destination_runtime_root)
        .map_err(|error| format!("could not create ONNX Runtime install directory: {error}"))?;
    require_directory(&destination_runtime_root)?;
    let destination_runtime = destination_runtime_root.join(&manifest.runtime_id);
    if destination_runtime.exists() {
        require_directory(&destination_runtime)?;
        verify_runtime_files(&destination_runtime, &manifest)?;
    } else {
        let staging = Builder::new()
            .prefix(".onnxruntime-stage-")
            .tempdir_in(&destination_runtime_root)
            .map_err(|error| format!("could not stage ONNX Runtime files: {error}"))?;
        copy_runtime_files(&source_runtime, staging.path(), &manifest)?;
        verify_runtime_files(staging.path(), &manifest)?;
        fs::rename(staging.path(), &destination_runtime)
            .map_err(|error| format!("could not publish ONNX Runtime files: {error}"))?;
    }

    let destination_manifests = destination_runtime_root.join("manifests");
    fs::create_dir_all(&destination_manifests)
        .map_err(|error| format!("could not create ONNX Runtime manifest directory: {error}"))?;
    require_directory(&destination_manifests)?;
    let destination_manifest = destination_manifests.join(format!("{app_version}.json"));
    if destination_manifest.exists() {
        require_regular_file(&destination_manifest)?;
        if fs::read(&destination_manifest)
            .map_err(|error| format!("could not read installed ONNX Runtime manifest: {error}"))?
            != manifest_bytes
        {
            return Err("installed ONNX Runtime manifest differs from the verified package".into());
        }
    } else {
        let mut temporary = NamedTempFile::new_in(&destination_manifests)
            .map_err(|error| format!("could not stage ONNX Runtime manifest: {error}"))?;
        temporary
            .write_all(&manifest_bytes)
            .map_err(|error| format!("could not write ONNX Runtime manifest: {error}"))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| format!("could not flush ONNX Runtime manifest: {error}"))?;
        temporary
            .persist(&destination_manifest)
            .map_err(|error| format!("could not publish ONNX Runtime manifest: {}", error.error))?;
    }

    require_directory(&destination_runtime)?;
    verify_runtime_files(&destination_runtime, &manifest)
}

fn copy_runtime_files(
    source_dir: &Path,
    destination_dir: &Path,
    manifest: &RuntimeManifest,
) -> Result<(), String> {
    for file in &manifest.files {
        let source = source_dir.join(&file.name);
        require_regular_file(&source)?;
        let destination = destination_dir.join(&file.name);
        let mut input = File::open(&source).map_err(|error| {
            format!(
                "could not open staged runtime file {}: {error}",
                source.display()
            )
        })?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|error| {
                format!(
                    "could not create staged runtime file {}: {error}",
                    destination.display()
                )
            })?;
        std::io::copy(&mut input, &mut output).map_err(|error| {
            format!(
                "could not copy staged runtime file {}: {error}",
                source.display()
            )
        })?;
        output.sync_all().map_err(|error| {
            format!(
                "could not flush staged runtime file {}: {error}",
                destination.display()
            )
        })?;
    }
    Ok(())
}

fn verify_runtime_files(directory: &Path, manifest: &RuntimeManifest) -> Result<(), String> {
    for file in &manifest.files {
        let path = directory.join(&file.name);
        require_regular_file(&path)?;
        let metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "could not inspect packaged runtime file {}: {error}",
                path.display()
            )
        })?;
        if metadata.len() != file.bytes || sha256_file(&path)? != file.sha256 {
            return Err(format!(
                "packaged ONNX Runtime file failed verification: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn require_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "packaged ONNX Runtime directory is missing at {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "packaged ONNX Runtime path is not a directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "packaged ONNX Runtime file is missing at {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "packaged ONNX Runtime path is not a regular file: {}",
            path.display()
        ));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| {
        format!(
            "could not open packaged runtime file {}: {error}",
            path.display()
        )
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            format!(
                "could not hash packaged runtime file {}: {error}",
                path.display()
            )
        })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(encode_hex(&digest.finalize()))
}

fn runtime_id(archive_sha256: &str) -> String {
    let short_digest = archive_sha256.get(..24).unwrap_or(archive_sha256);
    format!("ort-{short_digest}")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_safe_filename(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\', ':', '\0'])
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::fs;

    use tempfile::tempdir;

    use super::{
        PinnedFile, RuntimeManifest, RuntimePins, RuntimeTargetPins, encode_hex,
        install_verified_package, resolve_installation,
    };

    const RUNTIME_SHA256: &str = "d92c6a81b2ff50096bcda80885427d1f59a25b5f483f7055523504925d16ab23";

    fn pins() -> RuntimePins {
        RuntimePins {
            version: "1.23.2".into(),
            binding_version: "2.0.0-rc.11".into(),
            api_version: 23,
            targets: BTreeMap::from([(
                "x86_64-unknown-linux-gnu".into(),
                RuntimeTargetPins {
                    archive_url: "https://example.invalid/runtime.tgz".into(),
                    archive_sha256: RUNTIME_SHA256.into(),
                    archive_bytes: 7,
                    archive_format: "tar".into(),
                    library: "libonnxruntime.so".into(),
                    files: vec![PinnedFile {
                        archive_path: "runtime/libonnxruntime.so".into(),
                        name: "libonnxruntime.so".into(),
                        bytes: 7,
                        sha256: RUNTIME_SHA256.into(),
                    }],
                },
            )]),
        }
    }

    fn write_staged_package(
        root: &std::path::Path,
        app_version: &str,
        manifest: &RuntimeManifest,
        runtime_bytes: &[u8],
    ) {
        let runtime_dir = root.join("runtime").join(&manifest.runtime_id);
        let manifest_dir = root.join("runtime/manifests");
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(runtime_dir.join(&manifest.library), runtime_bytes).unwrap();
        fs::write(
            manifest_dir.join(format!("{app_version}.json")),
            serde_json::to_vec(manifest).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn resolves_verified_runtime_from_installation_directory() {
        let temporary = tempdir().unwrap();
        let install_dir = temporary.path().join("GitScry package with spaces");
        let pins = pins();
        let manifest =
            RuntimeManifest::from_pins("0.4.1", "x86_64-unknown-linux-gnu", &pins).unwrap();
        let runtime_dir = install_dir.join("runtime").join(&manifest.runtime_id);
        let manifest_dir = install_dir.join("runtime").join("manifests");
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(install_dir.join("gitscry"), b"executable").unwrap();
        fs::write(runtime_dir.join("libonnxruntime.so"), b"runtime").unwrap();
        fs::write(
            manifest_dir.join("0.4.1.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let artifact =
            resolve_installation(&install_dir, "0.4.1", "x86_64-unknown-linux-gnu", &pins).unwrap();

        assert_eq!(artifact.library_path, runtime_dir.join("libonnxruntime.so"));
    }

    #[test]
    fn rejects_a_runtime_file_that_does_not_match_its_manifest() {
        let temporary = tempdir().unwrap();
        let install_dir = temporary.path().join("install");
        let pins = pins();
        let manifest =
            RuntimeManifest::from_pins("0.4.1", "x86_64-unknown-linux-gnu", &pins).unwrap();
        let runtime_dir = install_dir.join("runtime").join(&manifest.runtime_id);
        let manifest_dir = install_dir.join("runtime").join("manifests");
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(runtime_dir.join("libonnxruntime.so"), b"tampered").unwrap();
        fs::write(
            manifest_dir.join("0.4.1.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = resolve_installation(&install_dir, "0.4.1", "x86_64-unknown-linux-gnu", &pins)
            .unwrap_err();

        assert!(error.contains("failed verification"));
    }

    #[test]
    fn installs_verified_runtime_as_a_complete_versioned_package() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("staged-package");
        let destination = temporary.path().join("installed-package");
        let pins = pins();
        let manifest =
            RuntimeManifest::from_pins("0.4.1", "x86_64-unknown-linux-gnu", &pins).unwrap();
        let source_runtime = source.join("runtime").join(&manifest.runtime_id);
        let source_manifests = source.join("runtime").join("manifests");
        fs::create_dir_all(&source_runtime).unwrap();
        fs::create_dir_all(&source_manifests).unwrap();
        fs::write(source_runtime.join("libonnxruntime.so"), b"runtime").unwrap();
        fs::write(
            source_manifests.join("0.4.1.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        install_verified_package(&source, &destination, "0.4.1", "x86_64-unknown-linux-gnu")
            .unwrap();
        install_verified_package(&source, &destination, "0.4.1", "x86_64-unknown-linux-gnu")
            .unwrap();

        let installed_runtime = destination.join("runtime").join(&manifest.runtime_id);
        assert_eq!(
            fs::read(installed_runtime.join("libonnxruntime.so")).unwrap(),
            b"runtime"
        );
        assert_eq!(
            resolve_installation(&destination, "0.4.1", "x86_64-unknown-linux-gnu", &pins,)
                .unwrap()
                .library_path,
            installed_runtime.join("libonnxruntime.so")
        );
    }
    #[test]
    fn installing_a_new_runtime_preserves_the_previous_version() {
        let temporary = tempdir().unwrap();
        let destination = temporary.path().join("installed-package");
        let old_pins = pins();
        let old_version = "0.4.1";
        let old_manifest =
            RuntimeManifest::from_pins(old_version, "x86_64-unknown-linux-gnu", &old_pins).unwrap();
        let old_source = temporary.path().join("old-package");
        write_staged_package(&old_source, old_version, &old_manifest, b"runtime");
        install_verified_package(
            &old_source,
            &destination,
            old_version,
            "x86_64-unknown-linux-gnu",
        )
        .unwrap();

        let new_version = "0.4.2";
        let new_runtime = b"runtime-v2";
        let mut new_pins = pins();
        new_pins.version = "1.23.3".into();
        let target_pins = new_pins
            .targets
            .get_mut("x86_64-unknown-linux-gnu")
            .unwrap();
        target_pins.archive_sha256 = encode_hex(&Sha256::digest(b"new archive"));
        target_pins.archive_bytes = b"new archive".len() as u64;
        target_pins.files[0].bytes = new_runtime.len() as u64;
        target_pins.files[0].sha256 = encode_hex(&Sha256::digest(new_runtime));
        let new_manifest =
            RuntimeManifest::from_pins(new_version, "x86_64-unknown-linux-gnu", &new_pins).unwrap();
        let new_source = temporary.path().join("new-package");
        write_staged_package(&new_source, new_version, &new_manifest, new_runtime);
        install_verified_package(
            &new_source,
            &destination,
            new_version,
            "x86_64-unknown-linux-gnu",
        )
        .unwrap();

        let old_runtime = resolve_installation(
            &destination,
            old_version,
            "x86_64-unknown-linux-gnu",
            &old_pins,
        )
        .unwrap();
        let new_runtime = resolve_installation(
            &destination,
            new_version,
            "x86_64-unknown-linux-gnu",
            &new_pins,
        )
        .unwrap();
        assert_ne!(old_runtime.library_path, new_runtime.library_path);
        assert_eq!(fs::read(old_runtime.library_path).unwrap(), b"runtime");
        assert_eq!(fs::read(new_runtime.library_path).unwrap(), b"runtime-v2");
    }
}
