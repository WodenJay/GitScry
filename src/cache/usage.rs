use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags, params};

const DATABASE_NAME: &str = "usage.sqlite";
const BUSY_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub(crate) struct DailyUsageRecord {
    pub(crate) date: String,
    pub(crate) command: String,
    pub(crate) calls: u64,
    pub(crate) elapsed_ns: u64,
}

pub(crate) struct Invocation {
    command: &'static str,
    date: String,
    started: Instant,
}

impl Invocation {
    pub(crate) fn begin(command: &'static str) -> Option<Self> {
        let started = Instant::now();
        let date = local_today().ok()?;
        Some(Self {
            command,
            date,
            started,
        })
    }

    pub(crate) fn finish(self) {
        let elapsed_ns = self.started.elapsed().as_nanos().min(i64::MAX as u128) as i64;
        let _ = record(&self.date, self.command, elapsed_ns);
    }
}

pub(crate) fn local_today() -> Result<String, String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .query_row("SELECT date('now', 'localtime')", [], |row| row.get(0))
        .map_err(|error| error.to_string())
}

fn data_directory() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("GitScry"))
            .ok_or_else(|| "LOCALAPPDATA is not set".to_owned())
    }

    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("Library/Application Support/GitScry"))
            .ok_or_else(|| "HOME is not set".to_owned())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let xdg = env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let data = xdg.or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .map(|home| home.join(".local/share"))
        });
        data.map(|path| path.join("gitscry"))
            .ok_or_else(|| "XDG_DATA_HOME and HOME are not set".to_owned())
    }

    #[cfg(not(any(target_os = "windows", unix)))]
    {
        Err("no standard local data directory is available on this platform".to_owned())
    }
}

fn database_path() -> Result<PathBuf, String> {
    Ok(data_directory()?.join(DATABASE_NAME))
}

fn record(date: &str, command: &str, elapsed_ns: i64) -> Result<(), String> {
    let path = database_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| "usage database has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let connection = Connection::open(&path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_daily (
                date TEXT NOT NULL,
                command TEXT NOT NULL,
                calls INTEGER NOT NULL CHECK (calls > 0),
                elapsed_ns INTEGER NOT NULL CHECK (elapsed_ns >= 0),
                PRIMARY KEY (date, command)
            ) WITHOUT ROWID;",
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO usage_daily (date, command, calls, elapsed_ns)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT (date, command) DO UPDATE SET
                 calls = usage_daily.calls + excluded.calls,
                 elapsed_ns = usage_daily.elapsed_ns + excluded.elapsed_ns",
            params![date, command, elapsed_ns],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn read_records() -> Result<Vec<DailyUsageRecord>, String> {
    let path = database_path()?;
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Ok(Vec::new());
    }
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare("SELECT date, command, calls, elapsed_ns FROM usage_daily")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|error| error.to_string())?;

    rows.map(|row| {
        let (date, command, calls, elapsed_ns) = row.map_err(|error| error.to_string())?;
        Ok(DailyUsageRecord {
            date,
            command,
            calls: u64::try_from(calls)
                .ok()
                .filter(|calls| *calls > 0)
                .ok_or_else(|| "invalid call count in usage database".to_owned())?,
            elapsed_ns: u64::try_from(elapsed_ns)
                .map_err(|_| "invalid elapsed time in usage database".to_owned())?,
        })
    })
    .collect()
}
