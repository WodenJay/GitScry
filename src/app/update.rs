mod package;

use super::{AppError, Outcome, UpdateStage};
use crate::runtime::install_verified_package;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::io;
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};
use tempfile::Builder;

const RELEASE_URL: &str = "https://api.github.com/repos/WodenJay/GitScry/releases/latest";
const USER_AGENT: &str = concat!("gitscry/", env!("CARGO_PKG_VERSION"));
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
#[derive(Clone, Debug)]
struct Asset {
    name: String,
    url: Option<String>,
}

#[derive(Clone, Debug)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

trait ReleaseSource {
    fn latest_release(&mut self) -> Result<Release, String>;
    fn download(&mut self, asset: &Asset) -> Result<Vec<u8>, String>;
}

pub(super) fn run(report: &mut dyn FnMut(UpdateStage)) -> Result<Outcome, AppError> {
    let target = Target::current().map_err(update_error)?;
    let executable = std::env::current_exe()
        .map_err(|error| update_error(format!("locating the running executable: {error}")))?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("CARGO_PKG_VERSION must be a valid semantic version");
    let mut source = GithubReleaseSource;

    run_with(&mut source, &current, &executable, target, report)
}

fn run_with<S: ReleaseSource>(
    source: &mut S,
    current: &Version,
    executable: &Path,
    target: Target,
    report: &mut dyn FnMut(UpdateStage),
) -> Result<Outcome, AppError> {
    let executable = resolve_executable(executable).map_err(update_error)?;
    let _lock = UpdateLock::acquire(&executable).map_err(update_error)?;

    report(UpdateStage::Checking);
    let release = source.latest_release().map_err(update_error)?;
    let latest = release_version(&release).map_err(update_error)?;

    match latest.cmp(current) {
        std::cmp::Ordering::Less => {
            return Err(update_error(format!(
                "refusing to downgrade GitScry from {current} to {latest}"
            )));
        }
        std::cmp::Ordering::Equal => {
            return Ok(Outcome {
                progress: Vec::new(),
                message: format!("GitScry {current} is already up to date."),
                warnings: Vec::new(),
                notices: Vec::new(),
                report: None,
                usage_report: None,
                github_links: None,
                clear_report: None,
                prune_report: None,
            });
        }
        std::cmp::Ordering::Greater => {}
    }

    let archive_name = target.archive_name();
    let checksum_name = format!("{archive_name}.sha256");
    let archive_asset = required_asset(&release, archive_name).map_err(update_error)?;
    let checksum_asset = required_asset(&release, &checksum_name).map_err(update_error)?;

    report(UpdateStage::Downloading);
    let archive = source.download(archive_asset).map_err(update_error)?;
    let checksum = source.download(checksum_asset).map_err(update_error)?;

    report(UpdateStage::Verifying);
    verify_checksum(&archive, &checksum, archive_name).map_err(update_error)?;

    report(UpdateStage::Installing);
    let warnings = install_archive(&archive, target, &executable, &latest.to_string())
        .map_err(update_error)?;
    Ok(Outcome {
        progress: Vec::new(),
        message: format!("Updated GitScry {current} → {latest}."),
        warnings,
        notices: Vec::new(),
        report: None,
        usage_report: None,
        github_links: None,
        clear_report: None,
        prune_report: None,
    })
}

fn update_error(error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!("error: {error}"))
}

fn resolve_executable(path: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "accessing the running executable `{}`: {error}",
            path.display()
        )
    })?;
    let resolved = if metadata.file_type().is_symlink() {
        fs::canonicalize(path).map_err(|error| {
            format!(
                "resolving the running executable link `{}`: {error}",
                path.display()
            )
        })?
    } else {
        path.to_path_buf()
    };

    if !fs::metadata(&resolved)
        .map_err(|error| format!("checking the running executable: {error}"))?
        .is_file()
    {
        return Err(format!(
            "running executable `{}` is not a file",
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn release_version(release: &Release) -> Result<Version, String> {
    if release.draft || release.prerelease {
        return Err("latest GitHub release is not a published stable release".to_owned());
    }

    let tag = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    let version = Version::parse(tag).map_err(|error| {
        format!(
            "GitHub release tag `{}` is not a valid version: {error}",
            release.tag_name
        )
    })?;
    if !version.pre.is_empty() {
        return Err(format!(
            "GitHub release tag `{}` is a prerelease",
            release.tag_name
        ));
    }
    Ok(version)
}

fn required_asset<'a>(release: &'a Release, name: &str) -> Result<&'a Asset, String> {
    let mut matches = release.assets.iter().filter(|asset| asset.name == name);
    let asset = matches
        .next()
        .ok_or_else(|| format!("GitHub release is missing required asset `{name}`"))?;
    if matches.next().is_some() {
        return Err(format!("GitHub release contains duplicate asset `{name}`"));
    }
    Ok(asset)
}

fn verify_checksum(archive: &[u8], checksum: &[u8], archive_name: &str) -> Result<(), String> {
    let expected = parse_checksum(checksum, archive_name)?;
    let actual = Sha256::digest(archive);
    let actual = hex_digest(&actual);
    if expected != actual {
        return Err(format!(
            "SHA-256 mismatch for `{archive_name}` (expected {expected}, got {actual})"
        ));
    }
    Ok(())
}

fn parse_checksum(checksum: &[u8], archive_name: &str) -> Result<String, String> {
    let text = std::str::from_utf8(checksum)
        .map_err(|error| format!("checksum asset is not UTF-8: {error}"))?;
    let mut candidates = Vec::new();

    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.is_empty() || !is_sha256(fields[0]) {
            continue;
        }
        let matches_asset = match fields.as_slice() {
            [_] => true,
            [_, name] => name.trim_start_matches('*') == archive_name,
            _ => false,
        };
        if matches_asset {
            candidates.push(fields[0].to_ascii_lowercase());
        }
    }

    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [digest] => Ok(digest.clone()),
        [] => Err(format!(
            "checksum asset does not contain a SHA-256 value for `{archive_name}`"
        )),
        _ => Err(format!(
            "checksum asset contains multiple SHA-256 values for `{archive_name}`"
        )),
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex_digest(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Copy)]
enum ArchiveFormat {
    TarXz,
    Zip,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Target {
    Aarch64AppleDarwin,
    X86_64UnknownLinuxGnu,
    X86_64PcWindowsMsvc,
}

impl Target {
    fn current() -> Result<Self, String> {
        #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
        {
            return Ok(Self::Aarch64AppleDarwin);
        }
        #[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
        {
            return Ok(Self::X86_64UnknownLinuxGnu);
        }
        #[cfg(all(target_arch = "x86_64", target_os = "windows", target_env = "msvc"))]
        {
            return Ok(Self::X86_64PcWindowsMsvc);
        }
        #[allow(unreachable_code)]
        Err(format!(
            "GitScry updates are not supported for target `{}`",
            runtime_target()
        ))
    }

    fn archive_name(self) -> &'static str {
        match self {
            Self::Aarch64AppleDarwin => "gitscry-aarch64-apple-darwin.tar.xz",
            Self::X86_64UnknownLinuxGnu => "gitscry-x86_64-unknown-linux-gnu.tar.xz",
            Self::X86_64PcWindowsMsvc => "gitscry-x86_64-pc-windows-msvc.zip",
        }
    }

    fn executable_name(self) -> &'static str {
        match self {
            Self::X86_64PcWindowsMsvc => "gitscry.exe",
            Self::Aarch64AppleDarwin | Self::X86_64UnknownLinuxGnu => "gitscry",
        }
    }

    fn triple(self) -> &'static str {
        match self {
            Self::Aarch64AppleDarwin => "aarch64-apple-darwin",
            Self::X86_64UnknownLinuxGnu => "x86_64-unknown-linux-gnu",
            Self::X86_64PcWindowsMsvc => "x86_64-pc-windows-msvc",
        }
    }

    fn archive_format(self) -> ArchiveFormat {
        match self {
            Self::Aarch64AppleDarwin | Self::X86_64UnknownLinuxGnu => ArchiveFormat::TarXz,
            Self::X86_64PcWindowsMsvc => ArchiveFormat::Zip,
        }
    }
}

#[allow(dead_code)]
fn runtime_target() -> String {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "macos") {
        format!("{arch}-apple-darwin")
    } else if cfg!(target_os = "windows") {
        let environment = if cfg!(target_env = "msvc") {
            "msvc"
        } else if cfg!(target_env = "gnu") {
            "gnu"
        } else {
            "unknown"
        };
        format!("{arch}-pc-windows-{environment}")
    } else if cfg!(target_os = "linux") {
        let environment = if cfg!(target_env = "musl") {
            "musl"
        } else if cfg!(target_env = "gnu") {
            "gnu"
        } else {
            "unknown"
        };
        format!("{arch}-unknown-linux-{environment}")
    } else {
        format!("{arch}-unknown-{}", std::env::consts::OS)
    }
}

fn install_archive(
    archive: &[u8],
    target: Target,
    executable: &Path,
    app_version: &str,
) -> Result<Vec<String>, String> {
    install_archive_with_publish(archive, target, executable, app_version, replace_executable)
}

fn install_archive_with_publish(
    archive: &[u8],
    target: Target,
    executable: &Path,
    app_version: &str,
    publish_executable: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let parent = executable
        .parent()
        .ok_or_else(|| "running executable has no parent directory".to_owned())?;
    let temporary = Builder::new()
        .prefix(".gitscry-update-")
        .tempdir_in(parent)
        .map_err(|error| format!("creating private update storage: {error}"))?;
    let staged = temporary.path().join(target.executable_name());

    package::extract(archive, target, temporary.path())?;
    copy_executable_permissions(executable, &staged)?;
    // Versioned manifests and content-addressed runtime IDs keep the old executable usable until it is replaced.
    install_verified_package(temporary.path(), parent, app_version, target.triple())
        .map_err(|error| format!("verifying and installing ONNX Runtime: {error}"))?;
    #[cfg(windows)]
    fs::remove_dir_all(temporary.path().join("runtime"))
        .map_err(|error| format!("removing the staged ONNX Runtime package: {error}"))?;
    publish_executable(&staged, executable)?;

    if !fs::metadata(executable)
        .map_err(|error| format!("checking the installed executable: {error}"))?
        .is_file()
    {
        return Err("replacement did not leave an executable at the running path".to_owned());
    }
    #[cfg(windows)]
    let warnings = cleanup_windows_update(temporary);
    #[cfg(not(windows))]
    let warnings = Vec::new();
    Ok(warnings)
}

fn copy_executable_permissions(source: &Path, staged: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(source)
            .map_err(|error| format!("reading executable permissions: {error}"))?
            .permissions()
            .mode();
        let mut permissions = fs::metadata(staged)
            .map_err(|error| format!("reading staged permissions: {error}"))?
            .permissions();
        permissions.set_mode(mode | 0o111);
        fs::set_permissions(staged, permissions)
            .map_err(|error| format!("setting executable permissions: {error}"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (source, staged);
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_executable(staged: &Path, executable: &Path) -> Result<(), String> {
    fs::rename(staged, executable)
        .map_err(|error| format!("replacing the running executable: {error}"))
}

#[cfg(windows)]
fn replace_executable(staged: &Path, executable: &Path) -> Result<(), String> {
    let backup = staged
        .parent()
        .ok_or_else(|| "staged executable has no parent directory".to_owned())?
        .join("previous-executable");
    fs::rename(executable, &backup)
        .map_err(|error| format!("moving the old executable into private storage: {error}"))?;

    if let Err(error) = fs::rename(staged, executable) {
        let restore = fs::rename(&backup, executable);
        return Err(match restore {
            Ok(()) => format!("installing the new executable: {error}"),
            Err(restore_error) => format!(
                "installing the new executable: {error}; restoring the old executable: {restore_error}"
            ),
        });
    }

    if !executable.is_file() {
        let rollback = fs::rename(executable, staged);
        let restore = fs::rename(&backup, executable);
        return Err(format!(
            "replacement did not leave an executable at the running path (rollback: {rollback:?}, restore: {restore:?})"
        ));
    }

    Ok(())
}

#[cfg(windows)]
fn cleanup_windows_update(temporary: tempfile::TempDir) -> Vec<String> {
    let backup = temporary.path().join("previous-executable");
    if fs::remove_file(&backup).is_ok() {
        return Vec::new();
    }

    // The old executable is still mapped by this process. Retain its private
    // directory for the helper rather than letting TempDir try to delete it.
    let directory = temporary.keep();
    match spawn_windows_cleanup(&directory) {
        Ok(()) => Vec::new(),
        Err(error) => vec![format!(
            "Update installed, but could not start old executable cleanup: {error}. Remove `{}` after GitScry exits.",
            directory.display()
        )],
    }
}

#[cfg(windows)]
fn spawn_windows_cleanup(directory: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    // Retry the image lock rather than waiting on a PID that might be reused.
    // Delete only the known backup and an empty directory, never a directory tree.
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$directory = $env:GITSCRY_UPDATE_CLEANUP_DIR
$backup = [IO.Path]::Combine($directory, 'previous-executable')
for ($attempt = 0; $attempt -lt 600; $attempt++) {
    try {
        [IO.File]::Delete($backup)
        [IO.Directory]::Delete($directory)
        exit 0
    } catch {
        Start-Sleep -Milliseconds 100
    }
}
exit 1
"#;
    let system_root = std::env::var_os("SystemRoot")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SystemRoot is not set"))?;
    let powershell =
        PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    Command::new(powershell)
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        // Pass the path as data, so quotes, Unicode and shell syntax stay literal.
        .env("GITSCRY_UPDATE_CLEANUP_DIR", directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()?;
    Ok(())
}

struct UpdateLock {
    file: File,
}

impl UpdateLock {
    fn acquire(executable: &Path) -> Result<Self, String> {
        let parent = executable
            .parent()
            .ok_or_else(|| "running executable has no parent directory".to_owned())?;
        let name = executable
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| "running executable has no usable file name".to_owned())?;
        let path = parent.join(format!(".{name}.gitscry-update.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("opening the update lock: {error}"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                Err("another GitScry update is already in progress; retry".to_owned())
            }
            Err(std::fs::TryLockError::Error(error)) => {
                Err(format!("acquiring the update lock: {error}"))
            }
        }
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        // Keep this inode: unlinking can split concurrent processes across lock files.
        let _ = self.file.unlock();
    }
}

struct GithubReleaseSource;

impl ReleaseSource for GithubReleaseSource {
    fn latest_release(&mut self) -> Result<Release, String> {
        let body = self.request(RELEASE_URL, "application/vnd.github+json")?;
        let release: GithubRelease = serde_json::from_slice(&body)
            .map_err(|error| format!("parsing GitHub release metadata: {error}"))?;
        Ok(Release {
            tag_name: release.tag_name,
            draft: release.draft,
            prerelease: release.prerelease,
            assets: release
                .assets
                .into_iter()
                .map(|asset| Asset {
                    name: asset.name,
                    url: asset.browser_download_url,
                })
                .collect(),
        })
    }

    fn download(&mut self, asset: &Asset) -> Result<Vec<u8>, String> {
        let url = asset
            .url
            .as_deref()
            .ok_or_else(|| format!("GitHub asset `{}` has no download URL", asset.name))?;
        self.request(url, "application/octet-stream")
    }
}

impl GithubReleaseSource {
    fn request(&self, url: &str, accept: &str) -> Result<Vec<u8>, String> {
        let mut response = ureq::get(url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", accept)
            .call()
            .map_err(|error| format!("requesting {url}: {error}"))?;
        response
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD_BYTES)
            .read_to_vec()
            .map_err(|error| format!("reading response from {url}: {error}"))
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{PinnedFile, RuntimeManifest, RuntimePins, RuntimeTargetPins};
    use std::{
        collections::{BTreeMap, HashMap},
        fs,
        io::{Cursor, Write},
    };
    use tempfile::TempDir;

    struct FixtureSource {
        release: Result<Release, String>,
        downloads: HashMap<String, Vec<u8>>,
        calls: Vec<String>,
    }

    impl ReleaseSource for FixtureSource {
        fn latest_release(&mut self) -> Result<Release, String> {
            self.calls.push("latest".to_owned());
            self.release.clone()
        }

        fn download(&mut self, asset: &Asset) -> Result<Vec<u8>, String> {
            self.calls.push(asset.name.clone());
            self.downloads
                .get(&asset.name)
                .cloned()
                .ok_or_else(|| format!("missing fixture asset `{}`", asset.name))
        }
    }

    fn fixture_source(version: &str, archive: Vec<u8>, archive_name: &str) -> FixtureSource {
        let checksum = Sha256::digest(&archive);
        let checksum_name = format!("{archive_name}.sha256");
        let mut downloads = HashMap::new();
        downloads.insert(archive_name.to_owned(), archive);
        downloads.insert(
            checksum_name.clone(),
            format!("{}  {archive_name}\n", hex_digest(&checksum)).into_bytes(),
        );
        FixtureSource {
            release: Ok(Release {
                tag_name: version.to_owned(),
                draft: false,
                prerelease: false,
                assets: vec![
                    Asset {
                        name: archive_name.to_owned(),
                        url: None,
                    },
                    Asset {
                        name: checksum_name,
                        url: None,
                    },
                ],
            }),
            downloads,
            calls: Vec::new(),
        }
    }

    fn tar_xz(path: &str, contents: &[u8]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            let path_bytes = path.as_bytes();
            assert!(path_bytes.len() <= 100);
            header.as_mut_bytes()[..path_bytes.len()].copy_from_slice(path_bytes);
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, contents).unwrap();
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        let mut input = Cursor::new(tar_bytes);
        lzma_rs::xz_compress(&mut input, &mut compressed).unwrap();
        compressed
    }

    fn fixture_pins(target: Target) -> RuntimePins {
        fixture_pins_for(target, "1.23.2", b"fixture runtime")
    }

    fn fixture_pins_for(
        target: Target,
        runtime_version: &str,
        runtime_bytes: &[u8],
    ) -> RuntimePins {
        let library = match target {
            Target::Aarch64AppleDarwin => format!("libonnxruntime.{runtime_version}.dylib"),
            Target::X86_64UnknownLinuxGnu => "libonnxruntime.so".into(),
            Target::X86_64PcWindowsMsvc => "onnxruntime.dll".into(),
        };
        let archive = format!("fixture archive {runtime_version}");
        RuntimePins {
            version: runtime_version.into(),
            binding_version: "2.0.0-rc.11".into(),
            api_version: 23,
            targets: BTreeMap::from([(
                target.triple().into(),
                RuntimeTargetPins {
                    archive_url: "https://example.invalid/onnxruntime".into(),
                    archive_sha256: hex_digest(&Sha256::digest(archive.as_bytes())),
                    archive_bytes: archive.len() as u64,
                    archive_format: match target {
                        Target::X86_64PcWindowsMsvc => "zip",
                        _ => "tar",
                    }
                    .into(),
                    library: library.clone(),
                    files: vec![PinnedFile {
                        archive_path: format!("onnxruntime/lib/{library}"),
                        name: library,
                        bytes: runtime_bytes.len() as u64,
                        sha256: hex_digest(&Sha256::digest(runtime_bytes)),
                    }],
                },
            )]),
        }
    }

    fn fixture_manifest(target: Target, app_version: &str) -> RuntimeManifest {
        RuntimeManifest::from_pins(app_version, target.triple(), &fixture_pins(target)).unwrap()
    }

    fn tar_xz_package(target: Target, app_version: &str, executable: &[u8]) -> Vec<u8> {
        tar_xz_package_with_runtime(target, app_version, executable, b"fixture runtime")
    }

    fn tar_xz_package_with_runtime(
        target: Target,
        app_version: &str,
        executable: &[u8],
        runtime_bytes: &[u8],
    ) -> Vec<u8> {
        let manifest = fixture_manifest(target, app_version);
        tar_xz_package_with_manifest(target, app_version, executable, &manifest, runtime_bytes)
    }

    fn tar_xz_package_with_pins(
        target: Target,
        app_version: &str,
        executable: &[u8],
        pins: &RuntimePins,
        runtime_bytes: &[u8],
    ) -> Vec<u8> {
        let manifest = RuntimeManifest::from_pins(app_version, target.triple(), pins).unwrap();
        tar_xz_package_with_manifest(target, app_version, executable, &manifest, runtime_bytes)
    }

    fn tar_xz_package_with_manifest(
        target: Target,
        app_version: &str,
        executable: &[u8],
        manifest: &RuntimeManifest,
        runtime_bytes: &[u8],
    ) -> Vec<u8> {
        let prefix = format!("gitscry-{}", target.triple());
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            append_tar_file(
                &mut builder,
                &format!("{prefix}/{}", target.executable_name()),
                executable,
                0o755,
            );
            append_tar_file(
                &mut builder,
                &format!("{prefix}/runtime/manifests/{app_version}.json"),
                &serde_json::to_vec(manifest).unwrap(),
                0o644,
            );
            for file in &manifest.files {
                append_tar_file(
                    &mut builder,
                    &format!("{prefix}/runtime/{}/{}", manifest.runtime_id, file.name),
                    runtime_bytes,
                    0o644,
                );
            }
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        lzma_rs::xz_compress(&mut Cursor::new(tar_bytes), &mut compressed).unwrap();
        compressed
    }

    fn append_tar_file<W: Write>(
        builder: &mut tar::Builder<W>,
        path: &str,
        contents: &[u8],
        mode: u32,
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_size(contents.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        builder.append(&header, contents).unwrap();
    }

    #[cfg(windows)]
    fn zip_package(target: Target, app_version: &str, executable: &[u8]) -> Vec<u8> {
        use zip::write::SimpleFileOptions;

        let manifest = fixture_manifest(target, app_version);
        let runtime_bytes = b"fixture runtime";
        let prefix = format!("gitscry-{}", target.triple());
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                format!("{prefix}/{}", target.executable_name()),
                SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(executable).unwrap();
        writer
            .start_file(
                format!("{prefix}/runtime/manifests/{app_version}.json"),
                SimpleFileOptions::default(),
            )
            .unwrap();
        writer
            .write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        for file in &manifest.files {
            writer
                .start_file(
                    format!("{prefix}/runtime/{}/{}", manifest.runtime_id, file.name),
                    SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(runtime_bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn executable() -> (TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gitscry");
        fs::write(&path, b"old executable").unwrap();
        (directory, path)
    }

    fn run_fixture(
        source: &mut FixtureSource,
        current: &str,
        executable: &Path,
    ) -> Result<(Outcome, Vec<UpdateStage>), AppError> {
        let mut stages = Vec::new();
        let target = Target::X86_64UnknownLinuxGnu;
        let outcome = run_with(
            source,
            &Version::parse(current).unwrap(),
            executable,
            target,
            &mut |stage| stages.push(stage),
        )?;
        Ok((outcome, stages))
    }

    #[test]
    fn updates_verified_tarball_and_reports_stages() {
        let archive_name = Target::X86_64UnknownLinuxGnu.archive_name();
        let archive = tar_xz_package(Target::X86_64UnknownLinuxGnu, "0.2.0", b"new executable");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let (directory, executable) = executable();

        let (outcome, stages) = run_fixture(&mut source, "0.1.0", &executable).unwrap();

        assert_eq!(fs::read(&executable).unwrap(), b"new executable");
        let manifest = fixture_manifest(Target::X86_64UnknownLinuxGnu, "0.2.0");
        assert_eq!(
            fs::read(
                directory
                    .path()
                    .join("runtime")
                    .join(manifest.runtime_id)
                    .join(manifest.library)
            )
            .unwrap(),
            b"fixture runtime"
        );
        assert_eq!(outcome.message, "Updated GitScry 0.1.0 → 0.2.0.");
        assert_eq!(
            stages,
            vec![
                UpdateStage::Checking,
                UpdateStage::Downloading,
                UpdateStage::Verifying,
                UpdateStage::Installing,
            ]
        );
        assert_eq!(
            source.calls,
            vec![
                "latest".to_owned(),
                archive_name.to_owned(),
                format!("{archive_name}.sha256"),
            ]
        );
    }

    #[test]
    fn updater_accepts_runtime_pins_from_the_new_release() {
        let target = Target::X86_64UnknownLinuxGnu;
        let runtime_bytes = b"updated runtime";
        let pins = fixture_pins_for(target, "1.24.0", runtime_bytes);
        let archive =
            tar_xz_package_with_pins(target, "0.2.0", b"new executable", &pins, runtime_bytes);
        let mut source = fixture_source("v0.2.0", archive, target.archive_name());
        let (directory, executable) = executable();

        run_fixture(&mut source, "0.1.0", &executable).unwrap();

        let manifest = RuntimeManifest::from_pins("0.2.0", target.triple(), &pins).unwrap();
        assert_eq!(
            fs::read(
                directory
                    .path()
                    .join("runtime")
                    .join(manifest.runtime_id)
                    .join(manifest.library)
            )
            .unwrap(),
            runtime_bytes
        );
    }

    #[test]
    fn missing_runtime_package_preserves_the_old_executable() {
        let target = Target::X86_64UnknownLinuxGnu;
        let archive_name = target.archive_name();
        let archive = tar_xz(
            "gitscry-x86_64-unknown-linux-gnu/gitscry",
            b"new executable",
        );
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let (_directory, executable) = executable();

        let error = match run_fixture(&mut source, "0.1.0", &executable) {
            Err(error) => error,
            Ok(_) => panic!("missing runtime package must be rejected"),
        };

        assert!(error.to_string().contains("ONNX Runtime"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn invalid_runtime_package_preserves_the_old_executable() {
        let target = Target::X86_64UnknownLinuxGnu;
        let archive_name = target.archive_name();
        let archive =
            tar_xz_package_with_runtime(target, "0.2.0", b"new executable", b"tampered runtime");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let (directory, executable) = executable();

        let error = match run_fixture(&mut source, "0.1.0", &executable) {
            Err(error) => error,
            Ok(_) => panic!("invalid runtime package must be rejected"),
        };

        assert!(error.to_string().contains("failed verification"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
        assert!(!directory.path().join("runtime").exists());
    }

    #[test]
    fn failed_executable_replacement_preserves_old_runtime() {
        let target = Target::X86_64UnknownLinuxGnu;
        let old_pins = fixture_pins(target);
        let old_manifest = RuntimeManifest::from_pins("0.1.0", target.triple(), &old_pins).unwrap();
        let (directory, executable) = executable();
        let runtime_root = directory.path().join("runtime");
        let old_runtime = runtime_root.join(&old_manifest.runtime_id);
        let old_manifests = runtime_root.join("manifests");
        fs::create_dir_all(&old_runtime).unwrap();
        fs::create_dir_all(&old_manifests).unwrap();
        fs::write(old_runtime.join(&old_manifest.library), b"fixture runtime").unwrap();
        fs::write(
            old_manifests.join("0.1.0.json"),
            serde_json::to_vec(&old_manifest).unwrap(),
        )
        .unwrap();

        let new_runtime = b"updated runtime";
        let new_pins = fixture_pins_for(target, "1.24.0", new_runtime);
        let new_manifest = RuntimeManifest::from_pins("0.2.0", target.triple(), &new_pins).unwrap();
        let archive =
            tar_xz_package_with_pins(target, "0.2.0", b"new executable", &new_pins, new_runtime);
        let error = install_archive_with_publish(&archive, target, &executable, "0.2.0", |_, _| {
            Err("injected executable replacement failure".to_owned())
        })
        .unwrap_err();

        assert!(error.contains("injected executable replacement failure"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
        assert!(runtime_root.join("manifests/0.2.0.json").is_file());
        let old_runtime = crate::runtime::resolve_installation(
            directory.path(),
            "0.1.0",
            target.triple(),
            &old_pins,
        )
        .unwrap();
        assert_eq!(
            fs::read(old_runtime.library_path).unwrap(),
            b"fixture runtime"
        );
        let new_runtime_path = runtime_root
            .join(new_manifest.runtime_id)
            .join(new_manifest.library);
        assert_eq!(fs::read(new_runtime_path).unwrap(), new_runtime);
    }
    #[test]
    fn current_version_skips_download() {
        let (_directory, executable) = executable();
        let mut source = FixtureSource {
            release: Ok(Release {
                tag_name: "v0.1.0".to_owned(),
                draft: false,
                prerelease: false,
                assets: Vec::new(),
            }),
            downloads: HashMap::new(),
            calls: Vec::new(),
        };

        let (outcome, stages) = run_fixture(&mut source, "0.1.0", &executable).unwrap();

        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
        assert_eq!(outcome.message, "GitScry 0.1.0 is already up to date.");
        assert_eq!(stages, vec![UpdateStage::Checking]);
        assert_eq!(source.calls, vec!["latest"]);
    }

    #[test]
    fn newer_current_version_refuses_downgrade() {
        let (_directory, executable) = executable();
        let mut source = FixtureSource {
            release: Ok(Release {
                tag_name: "v0.1.0".to_owned(),
                draft: false,
                prerelease: false,
                assets: Vec::new(),
            }),
            downloads: HashMap::new(),
            calls: Vec::new(),
        };

        let error = run_fixture(&mut source, "0.2.0", &executable)
            .err()
            .expect("expected update error");

        assert!(error.to_string().contains("refusing to downgrade"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
        assert_eq!(source.calls, vec!["latest"]);
    }

    #[test]
    fn checksum_failure_preserves_old_executable() {
        let archive_name = Target::X86_64UnknownLinuxGnu.archive_name();
        let archive = tar_xz("gitscry/gitscry", b"new executable");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        source.downloads.insert(
            format!("{archive_name}.sha256"),
            b"not a checksum\n".to_vec(),
        );
        let (_directory, executable) = executable();

        let error = run_fixture(&mut source, "0.1.0", &executable)
            .err()
            .expect("expected update error");

        assert!(error.to_string().contains("checksum asset"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn traversal_archive_is_rejected_before_replacement() {
        let archive_name = Target::X86_64UnknownLinuxGnu.archive_name();
        let archive = tar_xz("../gitscry", b"malicious");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let (_directory, executable) = executable();

        let error = run_fixture(&mut source, "0.1.0", &executable)
            .err()
            .expect("expected update error");

        assert!(error.to_string().contains("unsafe path"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn missing_checksum_asset_preserves_old_executable() {
        let archive_name = Target::X86_64UnknownLinuxGnu.archive_name();
        let archive = tar_xz("gitscry/gitscry", b"new executable");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let checksum_name = format!("{archive_name}.sha256");
        source
            .release
            .as_mut()
            .unwrap()
            .assets
            .retain(|asset| asset.name != checksum_name);
        let (_directory, executable) = executable();

        let error = run_fixture(&mut source, "0.1.0", &executable)
            .err()
            .expect("expected update error");

        assert!(error.to_string().contains("missing required asset"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn archive_without_expected_executable_preserves_old_executable() {
        let archive_name = Target::X86_64UnknownLinuxGnu.archive_name();
        let archive = tar_xz("gitscry/not-gitscry", b"not executable");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let (_directory, executable) = executable();

        let error = run_fixture(&mut source, "0.1.0", &executable)
            .err()
            .expect("expected update error");

        assert!(error.to_string().contains("does not contain"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn invalid_archive_preserves_old_executable() {
        let archive_name = Target::X86_64UnknownLinuxGnu.archive_name();
        let archive = b"not a tar.xz archive".to_vec();
        let mut source = fixture_source("v0.2.0", archive.clone(), archive_name);
        source.downloads.insert(
            format!("{archive_name}.sha256"),
            format!(
                "{}  {archive_name}\n",
                hex_digest(&Sha256::digest(&archive))
            )
            .into_bytes(),
        );
        let (_directory, executable) = executable();

        let error = run_fixture(&mut source, "0.1.0", &executable)
            .err()
            .expect("expected update error");

        assert!(
            error
                .to_string()
                .contains("decompressing the tar.xz archive")
        );
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn update_lock_rejects_a_second_holder_and_keeps_its_file() {
        let (_directory, executable) = executable();
        let lock_path = executable.parent().unwrap().join(format!(
            ".{}.gitscry-update.lock",
            executable.file_name().unwrap().to_string_lossy()
        ));
        let first = UpdateLock::acquire(&executable).unwrap();
        assert!(lock_path.is_file());

        let error = match UpdateLock::acquire(&executable) {
            Ok(_) => panic!("a second update lock holder was allowed"),
            Err(error) => error,
        };
        assert!(error.contains("already in progress"));

        drop(first);
        assert!(lock_path.is_file());
        let second = UpdateLock::acquire(&executable).unwrap();
        drop(second);
        assert!(lock_path.is_file());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_target_is_replaced_without_replacing_link() {
        use std::os::unix::fs::symlink;

        let archive_name = Target::X86_64UnknownLinuxGnu.archive_name();
        let archive = tar_xz_package(Target::X86_64UnknownLinuxGnu, "0.2.0", b"new executable");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("real-gitscry");
        let link = directory.path().join("gitscry");
        fs::write(&target, b"old executable").unwrap();
        symlink(&target, &link).unwrap();

        run_fixture(&mut source, "0.1.0", &link).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).unwrap(), b"new executable");
    }

    #[cfg(windows)]
    #[test]
    fn self_update_succeeds_and_cleans_up_after_exit() {
        use std::process::Command;
        use std::time::{Duration, Instant};

        const CHILD: &str = "GITSCRY_SELF_UPDATE_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let executable = std::env::current_exe().unwrap();
            let target = Target::X86_64PcWindowsMsvc;
            let archive = zip_package(target, "0.2.0", b"new executable");
            let mut source = fixture_source("v0.2.0", archive, target.archive_name());
            let outcome = run_with(
                &mut source,
                &Version::parse("0.1.0").unwrap(),
                &executable,
                target,
                &mut |_| {},
            )
            .unwrap();
            assert_eq!(outcome.message, "Updated GitScry 0.1.0 → 0.2.0.");
            return;
        }

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("gitscry.exe");
        fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        let output = Command::new(&executable)
            .args([
                "--exact",
                "app::update::tests::self_update_succeeds_and_cleans_up_after_exit",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert_eq!(fs::read(&executable).unwrap(), b"new executable");
        assert!(
            output.status.success(),
            "self-update failed after installing the new executable:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let update_storage_remains = fs::read_dir(directory.path()).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gitscry-update-")
            });
            if !update_storage_remains {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "old executable was not cleaned up"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(windows)]
    #[test]
    fn updates_verified_zip_on_windows() {
        let target = Target::X86_64PcWindowsMsvc;
        let archive_name = target.archive_name();
        let archive = zip_package(target, "0.2.0", b"new executable");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("gitscry.exe");
        fs::write(&executable, b"old executable").unwrap();
        let mut stages = Vec::new();

        let outcome = run_with(
            &mut source,
            &Version::parse("0.1.0").unwrap(),
            &executable,
            target,
            &mut |stage| stages.push(stage),
        )
        .unwrap();

        assert_eq!(fs::read(&executable).unwrap(), b"new executable");
        assert_eq!(outcome.message, "Updated GitScry 0.1.0 → 0.2.0.");
        assert_eq!(
            stages,
            vec![
                UpdateStage::Checking,
                UpdateStage::Downloading,
                UpdateStage::Verifying,
                UpdateStage::Installing,
            ]
        );
    }
}
