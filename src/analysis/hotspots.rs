//! Aggregation over eligible touches, independent of display and lineage traversal.
use super::capabilities::timeline::format_timestamp;
use crate::cache::FileTouches;

pub(crate) struct Report {
    pub(crate) target: String,
    pub(crate) total: usize,
    pub(crate) files: Vec<File>,
}
pub(crate) struct File {
    pub(crate) path: Vec<u8>,
    pub(crate) touching_commits: usize,
    pub(crate) last_changed: String,
    pub(crate) additions: Option<u64>,
    pub(crate) deletions: Option<u64>,
    pub(crate) churn_complete: bool,
}
impl Report {
    pub(crate) fn aggregate(target: String, touches: Vec<FileTouches>, limit: usize) -> Self {
        let mut files: Vec<_> = touches
            .into_iter()
            .filter_map(|touches| {
                let latest = *touches.times.iter().max()?;
                Some(File {
                    path: touches.path,
                    touching_commits: touches.times.len(),
                    last_changed: format_timestamp(latest),
                    additions: touches.additions,
                    deletions: touches.deletions,
                    churn_complete: touches.churn_complete,
                })
            })
            .collect();
        files.sort_by(|a, b| {
            b.touching_commits
                .cmp(&a.touching_commits)
                .then_with(|| a.path.cmp(&b.path))
        });
        let total = files.len();
        files.truncate(limit);
        Self {
            target,
            total,
            files,
        }
    }
}
