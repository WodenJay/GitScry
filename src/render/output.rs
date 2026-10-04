//! Display limits apply only after a query's complete original stdout is formatted.
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[cfg(windows)]
mod windows;

const BYTE_LIMIT: usize = 32_768;
const LINE_LIMIT: usize = 400;

pub(super) fn write_query(
    original: &str,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    full_output: bool,
    temporary_directory: &Path,
) -> io::Result<()> {
    let lines = original.split_inclusive('\n').count();
    if full_output || (original.len() <= BYTE_LIMIT && lines <= LINE_LIMIT) {
        return stdout.write_all(original.as_bytes());
    }
    let path = match preserve(original, temporary_directory) {
        Ok(path) => path,
        Err(error) => {
            writeln!(
                stderr,
                "warning: could not preserve full query output: {error}; displaying complete output instead"
            )?;
            return stdout.write_all(original.as_bytes());
        }
    };
    stdout.write_all(prefix(original).as_bytes())?;
    writeln!(stdout, "\n\n[GitScry output truncated]")?;
    writeln!(
        stdout,
        "Complete output: {lines} lines, {} UTF-8 bytes.",
        original.len()
    )?;
    writeln!(stdout, "Full output file: {}", path.display())?;
    writeln!(
        stdout,
        "Set GITSCRY_FULL_OUTPUT=1 to emit complete output without creating a spill file."
    )?;
    writeln!(
        stdout,
        "If the displayed prefix is insufficient, use rg or other search tools to find the information you need in the full output file. If that information is still insufficient, read the complete file."
    )
}

fn preserve(original: &str, temporary_directory: &Path) -> io::Result<PathBuf> {
    let directory = temporary_directory.join("gitscry");
    std::fs::create_dir_all(&directory)?;
    // NamedTempFile uses exclusive creation and owner-only permissions on Unix.
    let mut builder = tempfile::Builder::new();
    builder.prefix("query-").suffix(".txt");
    #[cfg(windows)]
    let mut file = builder.make_in(&directory, windows::create)?;
    #[cfg(not(windows))]
    let mut file = builder.tempfile_in(&directory)?;
    file.write_all(original.as_bytes())?;
    file.flush()?;
    let (_, path) = file.keep().map_err(|error| error.error)?;
    Ok(path)
}

fn prefix(original: &str) -> &str {
    let mut end = 0;
    for line in original.split_inclusive('\n').take(LINE_LIMIT) {
        let available = BYTE_LIMIT - end;
        if line.len() > available {
            let mut cut = available;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            end += cut;
            break;
        }
        end += line.len();
    }
    &original[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(original: &str, full: bool) -> (String, String, tempfile::TempDir) {
        let temporary = tempfile::tempdir().unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        write_query(original, &mut stdout, &mut stderr, full, temporary.path()).unwrap();
        (
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
            temporary,
        )
    }

    #[test]
    fn small_and_exact_limits_are_unchanged_without_spill_files() {
        for original in [
            String::new(),
            "a\r\nb".into(),
            "a\n".repeat(399),
            "a\n".repeat(400),
            "a".repeat(32767),
            "a".repeat(32768),
        ] {
            let (stdout, stderr, temporary) = display(&original, false);
            assert_eq!(stdout, original);
            assert!(stderr.is_empty());
            assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn overflowing_output_keeps_a_bounded_original_prefix_and_exact_spill() {
        let cases = [
            ("a\n".repeat(401), "a\n".repeat(400), 401),
            (format!("{}last", "a\n".repeat(400)), "a\n".repeat(400), 401),
            ("a".repeat(32769), "a".repeat(32768), 1),
            (format!("{}é終", "a".repeat(32767)), "a".repeat(32767), 1),
            (
                format!("{}終\nlast", "a\r\n".repeat(10000)),
                "a\r\n".repeat(400),
                10002,
            ),
            (
                format!("first\n{}\nlast", "b".repeat(32768)),
                format!("first\n{}", "b".repeat(32762)),
                3,
            ),
        ];
        for (original, expected_prefix, lines) in cases {
            let (stdout, stderr, temporary) = display(&original, false);
            let (visible, notice) = stdout.split_once("\n\n[GitScry output truncated]").unwrap();
            assert_eq!(visible, expected_prefix);
            assert!(original.starts_with(visible));
            assert!(visible.len() <= 32768 && visible.split_inclusive('\n').count() <= 400);
            assert!(notice.contains(&format!("{lines} lines, {} UTF-8 bytes", original.len())));
            assert!(stderr.is_empty());
            let path = notice
                .lines()
                .find_map(|line| line.strip_prefix("Full output file: "))
                .unwrap();
            assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
            assert!(Path::new(path).starts_with(temporary.path().join("gitscry")));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }

    #[test]
    fn full_mode_skips_preservation_even_for_oversized_output() {
        let original = "a\n".repeat(20000);
        let (stdout, stderr, temporary) = display(&original, true);
        assert_eq!(stdout, original);
        assert!(stderr.is_empty());
        assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
    }

    #[test]
    fn preservation_failure_warns_and_emits_original_successfully() {
        let temporary = tempfile::tempdir().unwrap();
        // A regular file where the dedicated directory must be is portable and deterministic.
        std::fs::write(temporary.path().join("gitscry"), "blocked").unwrap();
        let original = "a\n".repeat(401);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        write_query(&original, &mut stdout, &mut stderr, false, temporary.path()).unwrap();
        assert_eq!(stdout, original.as_bytes());
        assert!(String::from_utf8(stderr).unwrap().contains("warning:"));
    }
}
