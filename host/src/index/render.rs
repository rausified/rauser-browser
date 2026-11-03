//! Artifact Markdown rendering, including the Timeline roll-ups.

use super::*;

pub(super) fn artifact_path(artifacts_dir: &str, artifact: &ArtifactRow) -> PathBuf {
    artifact_file(
        artifacts_dir,
        &artifact.artifact_id,
        &artifact.artifact_type,
        &artifact.site,
    )
}

/// [`artifact_path`] from the `artifacts` columns it depends on.
pub(super) fn artifact_file(
    artifacts_dir: &str,
    artifact_id: &str,
    artifact_type: &str,
    site: &str,
) -> PathBuf {
    let native = artifact_id
        .split_once('/')
        .map_or(artifact_id, |(_, value)| value);
    let hash = sha256_hex(artifact_id.as_bytes());
    checked_relative_dir(artifacts_dir)
        .expect("validated artifacts directory")
        .join(slug(artifact_type, "artifact"))
        .join(slug(site, "site"))
        .join(format!("{}-{}.md", slug(native, "item"), &hash[..12]))
}

pub(super) fn slug(value: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= 80 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { fallback.into() } else { out }
}

/// A link from the rendered artifact file (nested under
/// `artifacts_dir/<type>/<site>/...md`) to a vault-root-relative target,
/// e.g. a summary file.
pub(super) fn relative_artifact_link(
    artifacts_dir: &str,
    artifact: &ArtifactRow,
    target: &str,
) -> String {
    let path = artifact_path(artifacts_dir, artifact);
    let from_dir = path.parent().unwrap_or(Path::new(""));
    markdown::relative_link(from_dir, Path::new(target))
}

pub(super) fn render_artifact(artifacts_dir: &str, artifact: &ArtifactRow) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!(
        "title: {}\n",
        yaml_string(&crate::presentation::plain_line(&artifact.title, 2048))
    ));
    out.push_str(&format!("url: {}\n", yaml_string(&artifact.url)));
    out.push_str("brauser:\n  kind: artifact\n");
    out.push_str(&format!(
        "  artifact_id: {}\n",
        yaml_string(&artifact.artifact_id)
    ));
    out.push_str(&format!(
        "  type: {}\n",
        yaml_string(&artifact.artifact_type)
    ));
    out.push_str(&format!("  first_seen: {}\n", artifact.first_seen));
    out.push_str(&format!("  last_seen: {}\n", artifact.last_seen));
    out.push_str(&format!(
        "  focused_min: {}\n",
        whole_minutes(artifact.focused_secs)
    ));
    out.push_str(&format!(
        "  background_min: {}\n",
        whole_minutes(artifact.background_secs)
    ));
    out.push_str(&format!("  opens: {}\n", artifact.opens));
    out.push_str(&format!("  days_opened: {}\n", artifact.days.len()));
    if let Some(mode) = &artifact.last_mode {
        out.push_str(&format!("  last_mode: {}\n", yaml_string(mode)));
    }
    if artifact.rolled_up {
        out.push_str("  rollup: true\n");
    }
    out.push_str("---\n\n");
    if let Some(why) = &artifact.why {
        out.push_str(&format!(
            "Why: {}\n\n",
            markdown::inline(why, MAX_RENDERED_FIELD_CHARS)
        ));
    }
    if let Some(summary) = artifact.summaries.last() {
        let link = relative_artifact_link(artifacts_dir, artifact, &summary.summary_path);
        out.push_str(&format!(
            "Summary: [{}, {} importance](<{link}>)\n\n",
            markdown::inline(&summary.document_type, MAX_RENDERED_FIELD_CHARS),
            crate::summary::importance_str(summary.importance),
        ));
    }
    out.push_str("## Timeline\n\n");
    // Newest first: each summary line, then the day rollups whose latest
    // date is not after it, so a summary sorts before a rollup of its day.
    // Rollups and summary lines are both dated in their event's own offset,
    // so the 20 newest summaries (by instant) are merged by that local date,
    // newest instant first within a date; across offsets (travel) the lines
    // can then differ from instant order, but each stays above its own
    // day's rollup.
    const MAX_SUMMARY_LINES: usize = 20;
    let mut shown = artifact
        .summaries
        .iter()
        .rev()
        .take(MAX_SUMMARY_LINES)
        .map(|summary| (summary_instant(summary), summary))
        .collect::<Vec<_>>();
    shown.sort_by(|(left, _), (right, _)| right.date().cmp(&left.date()).then(right.cmp(left)));
    let mut summaries = shown.into_iter().map(|(_, summary)| summary).peekable();
    for period in timeline_rollups(artifact) {
        while let Some(summary) =
            summaries.next_if(|summary| summary_instant(summary).date() >= period.last_date)
        {
            out.push_str(&timeline_summary_line(summary));
        }
        out.push_str(&format!(
            "- {} · {}\n",
            period.label,
            measures(period.focused, period.background, period.opens)
        ));
    }
    for summary in summaries {
        out.push_str(&timeline_summary_line(summary));
    }
    let earlier = artifact.summaries.len().saturating_sub(MAX_SUMMARY_LINES);
    if earlier > 0 {
        out.push_str(&format!("- {earlier} earlier summaries\n"));
    }
    out
}

/// A summary's instant in its own offset. Logged events are validated
/// RFC 3339, so the epoch fallback is never reached in practice.
pub(super) fn summary_instant(summary: &SummaryEvent) -> OffsetDateTime {
    parsed_timestamp(&summary.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// `- YYYY-MM-DD HH:MM · Summarized: <title> — <type>, <importance> importance`,
/// in the event's own offset and without seconds, so the file does not
/// churn, worded like the worklog.
pub(super) fn timeline_summary_line(summary: &SummaryEvent) -> String {
    let at = summary_instant(summary);
    format!(
        "- {} {:02}:{:02} · Summarized: {} — {}, {} importance\n",
        at.date(),
        at.hour(),
        at.minute(),
        markdown::inline(&summary.title, MAX_RENDERED_FIELD_CHARS),
        markdown::inline(&summary.document_type, MAX_RENDERED_FIELD_CHARS),
        crate::summary::importance_str(summary.importance),
    )
}

pub(super) fn whole_minutes(seconds: u64) -> u64 {
    seconds / 60
}

pub(super) fn displayed_minutes(seconds: u64) -> String {
    if seconds < 60 {
        "<1 min".into()
    } else {
        format!("{} min", whole_minutes(seconds))
    }
}

#[derive(Clone)]
pub(super) struct Rollup {
    label: String,
    focused: u64,
    background: u64,
    opens: u64,
    last_date: Date,
}

/// Activity totals and M1 visit entries (one open each) by day.
pub(super) fn timeline_rollups(artifact: &ArtifactRow) -> Vec<Rollup> {
    let activities = artifact.activities.iter().filter_map(|event| {
        let at = OffsetDateTime::parse(&event.occurred_at, &Rfc3339).ok()?;
        Some((
            at,
            u64::from(event.focused_seconds),
            u64::from(event.background_seconds),
            u64::from(event.opens),
        ))
    });
    let legacy = artifact.legacy_visits.iter().map(|at| (*at, 0, 0, 1));
    let mut days = BTreeMap::<Date, Rollup>::new();
    for (at, focused, background, opens) in activities.chain(legacy) {
        let date = at.date();
        let row = days.entry(date).or_insert_with(|| Rollup {
            label: date.to_string(),
            focused: 0,
            background: 0,
            opens: 0,
            last_date: date,
        });
        row.focused = row.focused.saturating_add(focused);
        row.background = row.background.saturating_add(background);
        row.opens = row.opens.saturating_add(opens);
    }
    let dates = days.keys().copied().collect::<Vec<_>>();
    let recent = dates.len().saturating_sub(14);
    let latest = dates.last().copied();
    let mut rolled = BTreeMap::<String, Rollup>::new();
    for (index, date) in dates.into_iter().enumerate() {
        let row = days.remove(&date).expect("date came from map");
        let label = if index >= recent {
            row.label.clone()
        } else if latest.is_some_and(|end| (end - date).whole_days() <= 26 * 7) {
            let (year, week, _) = date.to_iso_week_date();
            format!("{year:04}-W{week:02}")
        } else {
            format!("{:04}-{:02}", date.year(), date.month() as u8)
        };
        let target = rolled.entry(label.clone()).or_insert(Rollup {
            label,
            focused: 0,
            background: 0,
            opens: 0,
            last_date: date,
        });
        target.focused = target.focused.saturating_add(row.focused);
        target.background = target.background.saturating_add(row.background);
        target.opens = target.opens.saturating_add(row.opens);
        target.last_date = target.last_date.max(date);
    }
    let mut output = rolled.into_values().collect::<Vec<_>>();
    output.sort_by(|left, right| {
        right
            .last_date
            .cmp(&left.last_date)
            .then(right.label.cmp(&left.label))
    });
    output
}

pub(super) fn measures(focused: u64, background: u64, opens: u64) -> String {
    let mut values = Vec::new();
    if focused > 0 {
        values.push(format!("focused {}", displayed_minutes(focused)));
    }
    if background > 0 {
        values.push(format!("{} in background", displayed_minutes(background)));
    }
    if opens > 0 {
        values.push(format!("opened {opens}×"));
    }
    if values.is_empty() {
        "seen".into()
    } else {
        values.join(" · ")
    }
}
