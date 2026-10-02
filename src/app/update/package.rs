use super::{ArchiveFormat, Target};
use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;

pub(super) fn extract(archive: &[u8], target: Target, staging_dir: &Path) -> Result<(), String> {
    match target.archive_format() {
        ArchiveFormat::TarXz => extract_tar_xz(archive, target, staging_dir),
        ArchiveFormat::Zip => extract_zip(archive, target, staging_dir),
    }
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
#[derive(Debug)]
enum PackageFile {
    Executable,
    Runtime(PathBuf),
}

struct PackageStaging<'a> {
    target: Target,
    directory: &'a Path,
    found_executable: bool,
    runtime_bytes: u64,
}

impl<'a> PackageStaging<'a> {
    fn new(target: Target, directory: &'a Path) -> Self {
        Self {
            target,
            directory,
            found_executable: false,
            runtime_bytes: 0,
        }
    }

    fn receive(
        &mut self,
        path: &Path,
        entry: &mut impl Read,
        size: u64,
        is_file: bool,
        is_directory: bool,
    ) -> Result<(), String> {
        let executable = self.target.executable_name();
        let Some(file) = package_file(path, executable, is_file)? else {
            return Ok(());
        };
        match file {
            PackageFile::Executable => {
                if !is_file {
                    return Err(format!(
                        "archive entry `{executable}` is not a regular file"
                    ));
                }
                if self.found_executable {
                    return Err(format!(
                        "archive contains multiple `{executable}` executables"
                    ));
                }
                write_entry(
                    entry,
                    size,
                    &self.directory.join(executable),
                    MAX_EXECUTABLE_BYTES,
                    "executable",
                )?;
                self.found_executable = true;
            }
            PackageFile::Runtime(relative) => {
                if is_file {
                    self.runtime_bytes = write_runtime_entry(
                        entry,
                        size,
                        &self.directory.join(relative),
                        self.runtime_bytes,
                    )?;
                } else if !is_directory {
                    return Err("archive contains a non-regular ONNX Runtime entry".to_owned());
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<(), String> {
        require_executable(self.found_executable, self.target)
    }
}

fn extract_tar_xz(archive: &[u8], target: Target, staging_dir: &Path) -> Result<(), String> {
    let mut compressed = Cursor::new(archive);
    let mut decompressed = LimitedVec::new(MAX_ARCHIVE_BYTES);
    lzma_rs::xz_decompress(&mut compressed, &mut decompressed)
        .map_err(|error| format!("decompressing the tar.xz archive: {error}"))?;
    let decompressed = decompressed.into_inner();

    let mut package = PackageStaging::new(target, staging_dir);
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
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err("archive contains an unsafe link entry".to_owned());
        }
        let size = entry.size();
        package.receive(
            &path,
            &mut entry,
            size,
            entry_type.is_file(),
            entry_type.is_dir(),
        )?;
    }
    package.finish()
}

fn extract_zip(archive: &[u8], target: Target, staging_dir: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|error| format!("reading the zip archive: {error}"))?;
    let mut package = PackageStaging::new(target, staging_dir);
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
        let is_file = entry.is_file();
        let is_directory = entry.is_dir();
        let size = entry.size();
        package.receive(&path, &mut entry, size, is_file, is_directory)?;
    }
    package.finish()
}

fn package_file(
    path: &Path,
    executable: &str,
    is_file: bool,
) -> Result<Option<PackageFile>, String> {
    validate_archive_path(path)?;
    let components = path.components().collect::<Vec<_>>();
    let runtime_positions = components
        .iter()
        .enumerate()
        .filter(|(_, component)| component.as_os_str() == OsStr::new("runtime"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if !runtime_positions.is_empty() {
        if runtime_positions.len() != 1 || runtime_positions[0] > 1 {
            return Err("archive contains an invalid ONNX Runtime path".to_owned());
        }
        let suffix = &components[runtime_positions[0]..];
        if (is_file && suffix.len() != 3) || (!is_file && suffix.len() > 2) {
            return Err("archive contains an invalid ONNX Runtime package layout".to_owned());
        }
        let mut relative = PathBuf::from("runtime");
        for component in suffix.iter().skip(1) {
            relative.push(component.as_os_str());
        }
        return Ok(Some(PackageFile::Runtime(relative)));
    }

    if path.file_name() == Some(OsStr::new(executable)) {
        if components.len() > 2 {
            return Err("archive contains an invalid executable path".to_owned());
        }
        return Ok(Some(PackageFile::Executable));
    }
    Ok(None)
}

fn require_executable(found: bool, target: Target) -> Result<(), String> {
    if found {
        Ok(())
    } else {
        Err(format!(
            "archive does not contain the `{}` executable",
            target.executable_name()
        ))
    }
}

fn write_runtime_entry<R: Read>(
    entry: &mut R,
    size: u64,
    destination: &Path,
    total_runtime_bytes: u64,
) -> Result<u64, String> {
    let new_total = total_runtime_bytes
        .checked_add(size)
        .ok_or_else(|| "ONNX Runtime payload is too large".to_owned())?;
    if new_total > MAX_ARCHIVE_BYTES as u64 {
        return Err("ONNX Runtime payload exceeds the extraction limit".to_owned());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("creating ONNX Runtime staging directory: {error}"))?;
    }
    write_entry(
        entry,
        size,
        destination,
        MAX_ARCHIVE_BYTES as u64,
        "runtime file",
    )?;
    Ok(new_total)
}
fn validate_archive_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.to_string_lossy().contains('\\') {
        return Err("archive contains an unsafe path".to_owned());
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "archive contains an unsafe path `{}`",
            path.display()
        ));
    }
    Ok(())
}

fn write_entry<R: Read>(
    entry: &mut R,
    size: u64,
    staged: &Path,
    max_bytes: u64,
    description: &str,
) -> Result<(), String> {
    if size > max_bytes {
        return Err(format!(
            "{description} in archive exceeds the {max_bytes}-byte extraction limit"
        ));
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staged)
        .map_err(|error| format!("creating the staged {description}: {error}"))?;
    let copied = io::copy(&mut entry.take(max_bytes + 1), &mut output)
        .map_err(|error| format!("extracting the {description}: {error}"))?;
    if copied > max_bytes {
        return Err(format!(
            "{description} in archive exceeds the {max_bytes}-byte extraction limit"
        ));
    }
    if copied != size {
        return Err(format!(
            "extracted {description} size {copied} does not match archive size {size}"
        ));
    }
    output
        .sync_all()
        .map_err(|error| format!("flushing the staged {description}: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGETS: [Target; 2] = [Target::X86_64UnknownLinuxGnu, Target::X86_64PcWindowsMsvc];

    fn archive(target: Target, files: &[(&str, &[u8])], link: Option<&str>) -> Vec<u8> {
        match target.archive_format() {
            ArchiveFormat::TarXz => {
                let mut tar = tar::Builder::new(Vec::new());
                for &(name, contents) in files {
                    let mut header = tar::Header::new_gnu();
                    header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
                    header.set_size(contents.len() as u64);
                    header.set_mode(0o755);
                    header.set_cksum();
                    tar.append(&header, contents).unwrap();
                }
                if let Some(name) = link {
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    header.set_mode(0o777);
                    header.set_link_name("outside").unwrap();
                    tar.append_data(&mut header, name, io::empty()).unwrap();
                }
                let mut compressed = Vec::new();
                lzma_rs::xz_compress(&mut Cursor::new(tar.into_inner().unwrap()), &mut compressed)
                    .unwrap();
                compressed
            }
            ArchiveFormat::Zip => {
                let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
                let options = zip::write::SimpleFileOptions::default();
                for &(name, contents) in files {
                    zip.start_file(name, options).unwrap();
                    zip.write_all(contents).unwrap();
                }
                if let Some(name) = link {
                    zip.add_symlink(name, "outside", options).unwrap();
                }
                zip.finish().unwrap().into_inner()
            }
        }
    }

    #[test]
    fn both_formats_stage_executable_and_runtime_from_supported_layouts() {
        for target in TARGETS {
            for prefix in ["", "release/"] {
                let executable = format!("{prefix}{}", target.executable_name());
                let runtime = format!("{prefix}runtime/id/library");
                let bytes = archive(
                    target,
                    &[(&executable, b"binary"), (&runtime, b"runtime")],
                    None,
                );
                let staging = tempfile::tempdir().unwrap();
                extract(&bytes, target, staging.path()).unwrap();
                assert_eq!(
                    fs::read(staging.path().join(target.executable_name())).unwrap(),
                    b"binary"
                );
                assert_eq!(
                    fs::read(staging.path().join("runtime/id/library")).unwrap(),
                    b"runtime"
                );
            }
        }
    }

    #[test]
    fn both_formats_reject_duplicate_executables_and_missing_executables() {
        for target in TARGETS {
            let duplicate = format!("release/{}", target.executable_name());
            for files in [
                vec![
                    (target.executable_name(), b"binary".as_slice()),
                    (&duplicate, b"binary".as_slice()),
                ],
                vec![("README", b"text".as_slice())],
            ] {
                let staging = tempfile::tempdir().unwrap();
                let error =
                    extract(&archive(target, &files, None), target, staging.path()).unwrap_err();
                assert!(
                    error.contains("multiple") || error.contains("does not contain"),
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn both_formats_reject_unsafe_paths_links_and_runtime_layouts() {
        for target in TARGETS {
            for path in [
                "../outside",
                "directory\\outside",
                "one/two/runtime/id/library",
                "runtime/id/nested/library",
            ] {
                let staging = tempfile::tempdir().unwrap();
                let error = extract(
                    &archive(target, &[(path, b"payload")], None),
                    target,
                    staging.path(),
                )
                .unwrap_err();
                assert!(
                    error.contains("unsafe path") || error.contains("ONNX Runtime"),
                    "{error}"
                );
            }
            let staging = tempfile::tempdir().unwrap();
            let error =
                extract(&archive(target, &[], Some("link")), target, staging.path()).unwrap_err();
            assert!(error.contains("unsafe link"), "{error}");
        }
    }

    #[test]
    fn staging_rejects_oversized_and_non_regular_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let target = Target::X86_64UnknownLinuxGnu;
        let mut staging = PackageStaging::new(target, directory.path());
        let executable = Path::new(target.executable_name());
        assert!(
            staging
                .receive(executable, &mut io::empty(), 0, false, true)
                .unwrap_err()
                .contains("not a regular file")
        );
        assert!(
            staging
                .receive(
                    executable,
                    &mut io::empty(),
                    MAX_EXECUTABLE_BYTES + 1,
                    true,
                    false
                )
                .unwrap_err()
                .contains("extraction limit")
        );
        assert!(
            staging
                .receive(Path::new("runtime/id"), &mut io::empty(), 0, false, false)
                .unwrap_err()
                .contains("non-regular")
        );
        staging.runtime_bytes = MAX_ARCHIVE_BYTES as u64;
        assert!(
            staging
                .receive(
                    Path::new("runtime/id/library"),
                    &mut io::empty(),
                    1,
                    true,
                    false
                )
                .unwrap_err()
                .contains("extraction limit")
        );
        assert!(!directory.path().join(target.executable_name()).exists());
        assert!(!directory.path().join("runtime").exists());
    }
}
