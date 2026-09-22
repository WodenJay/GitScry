use super::{AppError, Outcome, UpdateStage};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Component, Path, PathBuf},
};
use tempfile::Builder;

const RELEASE_URL: &str = "https://api.github.com/repos/WodenJay/GitScry/releases/latest";
const USER_AGENT: &str = concat!("gitscry/", env!("CARGO_PKG_VERSION"));
const MAX_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;
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
                notices: Vec::new(),
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
    install_archive(&archive, target, &executable).map_err(update_error)?;

    Ok(Outcome {
        progress: Vec::new(),
        message: format!("Updated GitScry {current} → {latest}."),
        notices: Vec::new(),
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
    X86_64UnknownLinuxMusl,
    X86_64PcWindowsMsvc,
}

impl Target {
    fn current() -> Result<Self, String> {
        #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
        {
            Ok(Self::Aarch64AppleDarwin)
        }
        #[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "musl"))]
        {
            Ok(Self::X86_64UnknownLinuxMusl)
        }
        #[cfg(all(target_arch = "x86_64", target_os = "windows", target_env = "msvc"))]
        {
            Ok(Self::X86_64PcWindowsMsvc)
        }
        #[cfg(not(any(
            all(target_arch = "aarch64", target_os = "macos"),
            all(target_arch = "x86_64", target_os = "linux", target_env = "musl"),
            all(target_arch = "x86_64", target_os = "windows", target_env = "msvc"),
        )))]
        {
            Err(format!(
                "GitScry updates are not supported for target `{}`",
                runtime_target()
            ))
        }
    }

    fn archive_name(self) -> &'static str {
        match self {
            Self::Aarch64AppleDarwin => "gitscry-aarch64-apple-darwin.tar.xz",
            Self::X86_64UnknownLinuxMusl => "gitscry-x86_64-unknown-linux-musl.tar.xz",
            Self::X86_64PcWindowsMsvc => "gitscry-x86_64-pc-windows-msvc.zip",
        }
    }

    fn executable_name(self) -> &'static str {
        match self {
            Self::X86_64PcWindowsMsvc => "gitscry.exe",
            Self::Aarch64AppleDarwin | Self::X86_64UnknownLinuxMusl => "gitscry",
        }
    }

    fn archive_format(self) -> ArchiveFormat {
        match self {
            Self::Aarch64AppleDarwin | Self::X86_64UnknownLinuxMusl => ArchiveFormat::TarXz,
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

fn install_archive(archive: &[u8], target: Target, executable: &Path) -> Result<(), String> {
    let parent = executable
        .parent()
        .ok_or_else(|| "running executable has no parent directory".to_owned())?;
    let temporary = Builder::new()
        .prefix(".gitscry-update-")
        .tempdir_in(parent)
        .map_err(|error| format!("creating private update storage: {error}"))?;
    let staged = temporary.path().join(target.executable_name());

    match target.archive_format() {
        ArchiveFormat::TarXz => extract_tar_xz(archive, target.executable_name(), &staged)?,
        ArchiveFormat::Zip => extract_zip(archive, target.executable_name(), &staged)?,
    }
    copy_executable_permissions(executable, &staged)?;
    replace_executable(&staged, executable)?;

    if !fs::metadata(executable)
        .map_err(|error| format!("checking the installed executable: {error}"))?
        .is_file()
    {
        return Err("replacement did not leave an executable at the running path".to_owned());
    }
    Ok(())
}

struct LimitedVec {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedVec {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(1024 * 1024)),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for LimitedVec {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let new_len =
            self.bytes.len().checked_add(bytes.len()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "archive is too large")
            })?;
        if new_len > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive is too large",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn extract_tar_xz(archive: &[u8], expected: &str, staged: &Path) -> Result<(), String> {
    let mut compressed = Cursor::new(archive);
    let mut decompressed = LimitedVec::new(MAX_ARCHIVE_BYTES);
    lzma_rs::xz_decompress(&mut compressed, &mut decompressed)
        .map_err(|error| format!("decompressing the tar.xz archive: {error}"))?;
    let decompressed = decompressed.into_inner();

    let mut found = false;
    let mut archive = tar::Archive::new(Cursor::new(decompressed));
    let entries = archive
        .entries()
        .map_err(|error| format!("reading the tar archive: {error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("reading a tar entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("reading a tar entry path: {error}"))?
            .into_owned();
        validate_archive_path(&path)?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err("archive contains an unsafe link entry".to_owned());
        }
        if path.file_name() == Some(OsStr::new(expected)) {
            if found {
                return Err(format!(
                    "archive contains multiple `{expected}` executables"
                ));
            }
            if !entry_type.is_file() {
                return Err(format!("archive entry `{expected}` is not a regular file"));
            }
            let size = entry.size();
            write_entry(&mut entry, size, staged)?;
            found = true;
        }
    }

    if found {
        Ok(())
    } else {
        Err(format!(
            "archive does not contain the `{expected}` executable"
        ))
    }
}

fn extract_zip(archive: &[u8], expected: &str, staged: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|error| format!("reading the zip archive: {error}"))?;
    let mut found = false;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("reading a zip entry: {error}"))?;
        let raw_name = entry.name().to_owned();
        validate_archive_path(Path::new(&raw_name))?;
        let path = entry
            .enclosed_name()
            .ok_or_else(|| "archive contains an unsafe path".to_owned())?
            .to_path_buf();
        validate_archive_path(&path)?;
        if entry.is_symlink() {
            return Err("archive contains an unsafe link entry".to_owned());
        }
        if path.file_name() == Some(OsStr::new(expected)) {
            if found {
                return Err(format!(
                    "archive contains multiple `{expected}` executables"
                ));
            }
            if !entry.is_file() {
                return Err(format!("archive entry `{expected}` is not a regular file"));
            }
            let size = entry.size();
            write_entry(&mut entry, size, staged)?;
            found = true;
        }
    }

    if found {
        Ok(())
    } else {
        Err(format!(
            "archive does not contain the `{expected}` executable"
        ))
    }
}

fn validate_archive_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.to_string_lossy().contains('\\') {
        return Err("archive contains an unsafe path".to_owned());
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "archive contains an unsafe path `{}`",
            path.display()
        ));
    }
    Ok(())
}

fn write_entry<R: Read>(entry: &mut R, size: u64, staged: &Path) -> Result<(), String> {
    if size > MAX_EXECUTABLE_BYTES {
        return Err(format!(
            "executable in archive is larger than {MAX_EXECUTABLE_BYTES} bytes"
        ));
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staged)
        .map_err(|error| format!("creating the staged executable: {error}"))?;
    let copied = io::copy(entry, &mut output)
        .map_err(|error| format!("extracting the executable: {error}"))?;
    if copied != size {
        return Err(format!(
            "extracted executable size {copied} does not match archive size {size}"
        ));
    }
    output
        .sync_all()
        .map_err(|error| format!("flushing the staged executable: {error}"))?;
    Ok(())
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

    if let Err(error) = fs::remove_file(&backup) {
        schedule_delete_on_reboot(&backup).map_err(|schedule_error| {
            format!(
                "cleaning up the old executable: {error}; scheduling deferred cleanup: {schedule_error}"
            )
        })?;
    }
    Ok(())
}

#[cfg(windows)]
fn schedule_delete_on_reboot(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 0x0000_0004;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let scheduled =
        unsafe { MoveFileExW(path.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
    if scheduled == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn MoveFileExW(existing_file_name: *const u16, new_file_name: *const u16, flags: u32) -> i32;
}

struct UpdateLock {
    file: File,
    path: PathBuf,
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
            Ok(()) => Ok(Self { file, path }),
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
        let _ = self.file.unlock();
        let _ = fs::remove_file(&self.path);
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
    use std::{collections::HashMap, fs};
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
        let outcome = run_with(
            source,
            &Version::parse(current).unwrap(),
            executable,
            Target::X86_64UnknownLinuxMusl,
            &mut |stage| stages.push(stage),
        )?;
        Ok((outcome, stages))
    }

    #[test]
    fn updates_verified_tarball_and_reports_stages() {
        let archive_name = Target::X86_64UnknownLinuxMusl.archive_name();
        let archive = tar_xz(
            "gitscry-x86_64-unknown-linux-musl/gitscry",
            b"new executable",
        );
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let (_directory, executable) = executable();

        let (outcome, stages) = run_fixture(&mut source, "0.1.0", &executable).unwrap();

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

        let error = run_fixture(&mut source, "0.2.0", &executable).unwrap_err();

        assert!(error.to_string().contains("refusing to downgrade"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
        assert_eq!(source.calls, vec!["latest"]);
    }

    #[test]
    fn checksum_failure_preserves_old_executable() {
        let archive_name = Target::X86_64UnknownLinuxMusl.archive_name();
        let archive = tar_xz("gitscry/gitscry", b"new executable");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        source.downloads.insert(
            format!("{archive_name}.sha256"),
            b"not a checksum\n".to_vec(),
        );
        let (_directory, executable) = executable();

        let error = run_fixture(&mut source, "0.1.0", &executable).unwrap_err();

        assert!(error.to_string().contains("checksum asset"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn traversal_archive_is_rejected_before_replacement() {
        let archive_name = Target::X86_64UnknownLinuxMusl.archive_name();
        let archive = tar_xz("../gitscry", b"malicious");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let (_directory, executable) = executable();

        let error = run_fixture(&mut source, "0.1.0", &executable).unwrap_err();

        assert!(error.to_string().contains("unsafe path"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn missing_checksum_asset_preserves_old_executable() {
        let archive_name = Target::X86_64UnknownLinuxMusl.archive_name();
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

        let error = run_fixture(&mut source, "0.1.0", &executable).unwrap_err();

        assert!(error.to_string().contains("missing required asset"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn archive_without_expected_executable_preserves_old_executable() {
        let archive_name = Target::X86_64UnknownLinuxMusl.archive_name();
        let archive = tar_xz("gitscry/not-gitscry", b"not executable");
        let mut source = fixture_source("v0.2.0", archive, archive_name);
        let (_directory, executable) = executable();

        let error = run_fixture(&mut source, "0.1.0", &executable).unwrap_err();

        assert!(error.to_string().contains("does not contain"));
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn invalid_archive_preserves_old_executable() {
        let archive_name = Target::X86_64UnknownLinuxMusl.archive_name();
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

        let error = run_fixture(&mut source, "0.1.0", &executable).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("decompressing the tar.xz archive")
        );
        assert_eq!(fs::read(&executable).unwrap(), b"old executable");
    }

    #[test]
    fn update_lock_rejects_a_second_holder() {
        let (_directory, executable) = executable();
        let first = UpdateLock::acquire(&executable).unwrap();

        let error = match UpdateLock::acquire(&executable) {
            Ok(_) => panic!("a second update lock holder was allowed"),
            Err(error) => error,
        };
        assert!(error.contains("already in progress"));

        drop(first);
        assert!(UpdateLock::acquire(&executable).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_target_is_replaced_without_replacing_link() {
        use std::os::unix::fs::symlink;

        let archive_name = Target::X86_64UnknownLinuxMusl.archive_name();
        let archive = tar_xz("gitscry/gitscry", b"new executable");
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
}
