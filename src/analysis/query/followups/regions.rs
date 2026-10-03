use super::{LinePosition, RegionAssociation, RegionTracking, RegionTrackingReason, TrackedRegion};
use crate::{
    app::AppError,
    cache::{PatchHistoryHunk, QuerySession},
};

const MAX_PATCHES: usize = 10_000;
const MAX_HUNKS: usize = 4_096;
const MAX_PATCH_BYTES: usize = 64 * 1024 * 1024;
const MAX_HUNK_BYTES: usize = 16 * 1024;

#[derive(Default)]
pub(super) struct Budget {
    patches: usize,
    hunks: usize,
    bytes: usize,
}

impl Budget {
    pub(super) fn new() -> Self {
        Self {
            patches: MAX_PATCHES,
            hunks: MAX_HUNKS,
            bytes: MAX_PATCH_BYTES,
        }
    }
}

pub(super) struct ChangeContext<'a> {
    pub(super) oid: &'a str,
    pub(super) change_ordinal: i64,
    pub(super) content_changed: bool,
    pub(super) previous_revision: &'a str,
    pub(super) seed_path: Option<&'a [u8]>,
    pub(super) previous_path: &'a [u8],
    pub(super) current_path: Option<&'a [u8]>,
}

pub(super) struct Advance {
    pub(super) tracking: RegionTracking,
    pub(super) associations: Vec<RegionAssociation>,
    pub(super) downgrade_reason: Option<RegionTrackingReason>,
}

enum PatchMaterial {
    Available(Vec<PatchHistoryHunk>),
    Unavailable(RegionTrackingReason),
}

struct ParsedHunk {
    old_first: i64,
    old_lines: i64,
    new_first: i64,
    new_lines: i64,
    context: Vec<(i64, i64)>,
    edits: Vec<EditGroup>,
}

struct EditGroup {
    old_boundary: i64,
    new_boundary: i64,
    deleted: Vec<i64>,
    added: Vec<i64>,
}

pub(super) fn seed_tracking(
    session: &QuerySession,
    oid: &str,
    change_ordinal: i64,
    content_changed: bool,
    deleted: bool,
    budget: &mut Budget,
) -> Result<RegionTracking, AppError> {
    if deleted || !content_changed {
        return Ok(RegionTracking::Available(Vec::new()));
    }
    let hunks = match patch_material(session, oid, change_ordinal, budget)? {
        PatchMaterial::Available(hunks) => hunks,
        PatchMaterial::Unavailable(reason) => {
            return Ok(RegionTracking::Unavailable(reason));
        }
    };
    if hunks.is_empty() {
        return Ok(RegionTracking::Unavailable(
            RegionTrackingReason::NonTextualSeedChange,
        ));
    }
    let parsed = match parse_hunks(&hunks, change_ordinal) {
        Ok(parsed) => parsed,
        Err(reason) => return Ok(RegionTracking::Unavailable(reason)),
    };
    let mut added_lines = Vec::new();
    for hunk in parsed {
        added_lines.extend(hunk.edits.into_iter().flat_map(|edit| edit.added));
    }
    let regions = line_positions(added_lines)
        .into_iter()
        .map(|position| TrackedRegion {
            seed_position: position.clone(),
            current_positions: vec![position],
        })
        .collect();
    Ok(RegionTracking::Available(regions))
}

pub(super) fn advance(
    session: &QuerySession,
    tracking: &RegionTracking,
    context: ChangeContext<'_>,
    include_associations: bool,
    budget: &mut Budget,
) -> Result<Advance, AppError> {
    let RegionTracking::Available(regions) = tracking else {
        let RegionTracking::Unavailable(reason) = tracking else {
            unreachable!()
        };
        return Ok(Advance {
            tracking: tracking.clone(),
            associations: Vec::new(),
            downgrade_reason: Some(*reason),
        });
    };
    if !regions
        .iter()
        .any(|region| !region.current_positions.is_empty())
        || !context.content_changed
    {
        return Ok(Advance {
            tracking: tracking.clone(),
            associations: Vec::new(),
            downgrade_reason: None,
        });
    }
    let Some(seed_path) = context.seed_path else {
        return Ok(unavailable(RegionTrackingReason::InvalidPatchMapping));
    };
    let hunks = match patch_material(session, context.oid, context.change_ordinal, budget)? {
        PatchMaterial::Available(hunks) => hunks,
        PatchMaterial::Unavailable(reason) => return Ok(unavailable(reason)),
    };
    if hunks.is_empty() {
        return Ok(unavailable(RegionTrackingReason::NonTextualChange));
    }
    let parsed = match parse_hunks(&hunks, context.change_ordinal) {
        Ok(parsed) => parsed,
        Err(reason) => return Ok(unavailable(reason)),
    };
    match map_regions(regions, &parsed, &context, seed_path, include_associations) {
        Ok((regions, associations)) => Ok(Advance {
            tracking: RegionTracking::Available(regions),
            associations,
            downgrade_reason: None,
        }),
        Err(reason) => Ok(unavailable(reason)),
    }
}

fn unavailable(reason: RegionTrackingReason) -> Advance {
    Advance {
        tracking: RegionTracking::Unavailable(reason),
        associations: Vec::new(),
        downgrade_reason: Some(reason),
    }
}

fn patch_material(
    session: &QuerySession,
    oid: &str,
    change_ordinal: i64,
    budget: &mut Budget,
) -> Result<PatchMaterial, AppError> {
    if budget.patches == 0 || budget.hunks == 0 || budget.bytes == 0 {
        return Ok(PatchMaterial::Unavailable(
            RegionTrackingReason::TrackingBudgetExhausted,
        ));
    }
    let bytes_per_hunk = budget.bytes.min(MAX_HUNK_BYTES);
    let max_hunks = budget.hunks.min((budget.bytes / bytes_per_hunk).max(1));
    budget.patches -= 1;
    let history =
        session.patch_history_for_change(oid, change_ordinal, max_hunks, bytes_per_hunk)?;
    budget.hunks = budget.hunks.saturating_sub(history.hunks.len());
    budget.bytes = budget.bytes.saturating_sub(
        history
            .hunks
            .iter()
            .filter_map(|hunk| hunk.text.as_ref())
            .map(Vec::len)
            .sum(),
    );
    if history.missing_objects {
        return Ok(PatchMaterial::Unavailable(
            RegionTrackingReason::MissingPatchMaterial,
        ));
    }
    if history.truncated || history.hunks.iter().any(|hunk| hunk.text.is_none()) {
        return Ok(PatchMaterial::Unavailable(
            RegionTrackingReason::TruncatedPatchMaterial,
        ));
    }
    Ok(PatchMaterial::Available(history.hunks))
}

fn parse_hunks(
    hunks: &[PatchHistoryHunk],
    change_ordinal: i64,
) -> Result<Vec<ParsedHunk>, RegionTrackingReason> {
    hunks
        .iter()
        .map(|hunk| {
            if hunk.change_ordinal != change_ordinal {
                return Err(RegionTrackingReason::InvalidPatchMapping);
            }
            parse_hunk(hunk)
        })
        .collect()
}

fn parse_hunk(hunk: &PatchHistoryHunk) -> Result<ParsedHunk, RegionTrackingReason> {
    let old_first = first_line(hunk.old_start, hunk.old_lines)?;
    let new_first = first_line(hunk.new_start, hunk.new_lines)?;
    let text = hunk
        .text
        .as_deref()
        .ok_or(RegionTrackingReason::TruncatedPatchMaterial)?;
    let mut old_line = old_first;
    let mut new_line = new_first;
    let mut context = Vec::new();
    let mut edits = Vec::new();
    let mut edit = None;
    let mut first_row = true;
    for raw in text.split_inclusive(|byte| *byte == b'\n') {
        let row = raw.strip_suffix(b"\n").unwrap_or(raw);
        if first_row {
            first_row = false;
            if !row.starts_with(b"@@ ") {
                return Err(RegionTrackingReason::InvalidPatchMapping);
            }
            continue;
        }
        match row.first() {
            Some(b' ') => {
                if let Some(edit) = edit.take() {
                    edits.push(edit);
                }
                context.push((old_line, new_line));
                old_line = old_line
                    .checked_add(1)
                    .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
                new_line = new_line
                    .checked_add(1)
                    .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
            }
            Some(b'-') => {
                let edit = edit.get_or_insert_with(|| EditGroup {
                    old_boundary: old_line,
                    new_boundary: new_line,
                    deleted: Vec::new(),
                    added: Vec::new(),
                });
                edit.deleted.push(old_line);
                old_line = old_line
                    .checked_add(1)
                    .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
            }
            Some(b'+') => {
                let edit = edit.get_or_insert_with(|| EditGroup {
                    old_boundary: old_line,
                    new_boundary: new_line,
                    deleted: Vec::new(),
                    added: Vec::new(),
                });
                edit.added.push(new_line);
                new_line = new_line
                    .checked_add(1)
                    .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
            }
            Some(b'\\') => {}
            _ => return Err(RegionTrackingReason::InvalidPatchMapping),
        }
    }
    if first_row {
        return Err(RegionTrackingReason::InvalidPatchMapping);
    }
    if let Some(edit) = edit {
        edits.push(edit);
    }
    if old_line - old_first != hunk.old_lines || new_line - new_first != hunk.new_lines {
        return Err(RegionTrackingReason::InvalidPatchMapping);
    }
    Ok(ParsedHunk {
        old_first,
        old_lines: hunk.old_lines,
        new_first,
        new_lines: hunk.new_lines,
        context,
        edits,
    })
}

fn first_line(start: i64, count: i64) -> Result<i64, RegionTrackingReason> {
    if start < 0 || count < 0 || (count > 0 && start == 0) {
        return Err(RegionTrackingReason::InvalidPatchMapping);
    }
    if count == 0 {
        start
            .checked_add(1)
            .ok_or(RegionTrackingReason::InvalidPatchMapping)
    } else {
        Ok(start)
    }
}

fn map_regions(
    regions: &[TrackedRegion],
    hunks: &[ParsedHunk],
    context: &ChangeContext<'_>,
    seed_path: &[u8],
    include_associations: bool,
) -> Result<(Vec<TrackedRegion>, Vec<RegionAssociation>), RegionTrackingReason> {
    let mut mapped = vec![Vec::new(); regions.len()];
    let mut associations = Vec::new();
    let (mut old_cursor, mut new_cursor) = (1_i64, 1_i64);
    for hunk in hunks {
        if hunk.old_first < old_cursor || hunk.new_first < new_cursor {
            return Err(RegionTrackingReason::InvalidPatchMapping);
        }
        for (region, mapped_positions) in regions.iter().zip(&mut mapped) {
            map_gap(
                &region.current_positions,
                old_cursor,
                hunk.old_first,
                new_cursor - old_cursor,
                mapped_positions,
            )?;
        }
        for &(old_line, new_line) in &hunk.context {
            for (index, region) in regions.iter().enumerate() {
                if contains(&region.current_positions, old_line) {
                    mapped[index].push(LinePosition {
                        start_line: new_line,
                        line_count: 1,
                    });
                }
            }
        }
        for edit in &hunk.edits {
            for (region_index, region) in regions.iter().enumerate() {
                let deleted = edit
                    .deleted
                    .iter()
                    .copied()
                    .filter(|line| contains(&region.current_positions, *line))
                    .collect::<Vec<_>>();
                let insertion_inside = !edit.added.is_empty()
                    && contains_interior_boundary(&region.current_positions, edit.old_boundary);
                if deleted.is_empty() && !insertion_inside {
                    continue;
                }
                mapped[region_index].extend(line_positions(edit.added.iter().copied()));
                if include_associations {
                    let previous_positions = if deleted.is_empty() {
                        vec![point(edit.old_boundary)]
                    } else {
                        line_positions(deleted)
                    };
                    let current_positions = if edit.added.is_empty() {
                        vec![point(edit.new_boundary)]
                    } else {
                        line_positions(edit.added.iter().copied())
                    };
                    for previous_position in &previous_positions {
                        for current_position in &current_positions {
                            associations.push(RegionAssociation {
                                seed_path: seed_path.to_vec(),
                                previous_revision: context.previous_revision.to_owned(),
                                previous_path: context.previous_path.to_vec(),
                                current_path: context.current_path.map(<[u8]>::to_vec),
                                seed_position: region.seed_position.clone(),
                                previous_position: previous_position.clone(),
                                current_position: current_position.clone(),
                            });
                        }
                    }
                }
            }
        }
        old_cursor = hunk
            .old_first
            .checked_add(hunk.old_lines)
            .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
        new_cursor = hunk
            .new_first
            .checked_add(hunk.new_lines)
            .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
    }
    for (region, mapped_positions) in regions.iter().zip(&mut mapped) {
        map_tail(
            &region.current_positions,
            old_cursor,
            new_cursor - old_cursor,
            mapped_positions,
        )?;
    }
    let regions = regions
        .iter()
        .zip(mapped)
        .map(|(region, positions)| {
            Ok(TrackedRegion {
                seed_position: region.seed_position.clone(),
                current_positions: normalize_positions(positions)?,
            })
        })
        .collect::<Result<_, RegionTrackingReason>>()?;
    associations.sort();
    associations.dedup();
    Ok((regions, associations))
}

fn map_gap(
    positions: &[LinePosition],
    start: i64,
    end: i64,
    delta: i64,
    mapped: &mut Vec<LinePosition>,
) -> Result<(), RegionTrackingReason> {
    for position in positions {
        let position_end = position
            .start_line
            .checked_add(position.line_count)
            .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
        let overlap_start = position.start_line.max(start);
        let overlap_end = position_end.min(end);
        if overlap_start < overlap_end {
            mapped.push(translate(overlap_start, overlap_end, delta)?);
        }
    }
    Ok(())
}

fn map_tail(
    positions: &[LinePosition],
    start: i64,
    delta: i64,
    mapped: &mut Vec<LinePosition>,
) -> Result<(), RegionTrackingReason> {
    for position in positions {
        let position_end = position
            .start_line
            .checked_add(position.line_count)
            .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
        let overlap_start = position.start_line.max(start);
        if overlap_start < position_end {
            mapped.push(translate(overlap_start, position_end, delta)?);
        }
    }
    Ok(())
}

fn translate(start: i64, end: i64, delta: i64) -> Result<LinePosition, RegionTrackingReason> {
    let start_line = start
        .checked_add(delta)
        .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
    let end_line = end
        .checked_add(delta)
        .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
    if start_line < 1 || end_line < start_line {
        return Err(RegionTrackingReason::InvalidPatchMapping);
    }
    Ok(LinePosition {
        start_line,
        line_count: end_line - start_line,
    })
}

fn contains(positions: &[LinePosition], line: i64) -> bool {
    positions.iter().any(|position| {
        position
            .start_line
            .checked_add(position.line_count)
            .is_some_and(|end| position.start_line <= line && line < end)
    })
}

fn contains_interior_boundary(positions: &[LinePosition], boundary: i64) -> bool {
    positions.iter().any(|position| {
        position
            .start_line
            .checked_add(position.line_count)
            .is_some_and(|end| position.start_line < boundary && boundary < end)
    })
}

fn point(line: i64) -> LinePosition {
    LinePosition {
        start_line: line,
        line_count: 0,
    }
}

fn line_positions(lines: impl IntoIterator<Item = i64>) -> Vec<LinePosition> {
    let mut lines = lines.into_iter().collect::<Vec<_>>();
    lines.sort_unstable();
    lines.dedup();
    let mut positions: Vec<LinePosition> = Vec::new();
    for line in lines {
        if let Some(last) = positions.last_mut()
            && last.start_line.checked_add(last.line_count) == Some(line)
        {
            last.line_count += 1;
            continue;
        }
        positions.push(LinePosition {
            start_line: line,
            line_count: 1,
        });
    }
    positions
}

fn normalize_positions(
    mut positions: Vec<LinePosition>,
) -> Result<Vec<LinePosition>, RegionTrackingReason> {
    positions.sort_unstable();
    let mut normalized: Vec<LinePosition> = Vec::new();
    for position in positions {
        if position.line_count <= 0 || position.start_line < 1 {
            return Err(RegionTrackingReason::InvalidPatchMapping);
        }
        let end = position
            .start_line
            .checked_add(position.line_count)
            .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
        if let Some(last) = normalized.last_mut() {
            let last_end = last
                .start_line
                .checked_add(last.line_count)
                .ok_or(RegionTrackingReason::InvalidPatchMapping)?;
            if position.start_line <= last_end {
                last.line_count = last_end.max(end) - last.start_line;
                continue;
            }
        }
        normalized.push(position);
    }
    Ok(normalized)
}
