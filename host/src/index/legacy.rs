//! In-place migration of older `index-state.json` cache versions.

use super::*;

/// Bring a cache (or saved catch-up progress) of an older format up to
/// `STATE_VERSION`, scheduling a one-time gap rescan. False means it must be
/// rebuilt from the logs instead.
pub(super) fn migrate_state(state: &mut IndexState) -> bool {
    if state.events.values().any(|event| !event.valid_provenance()) {
        return false;
    }
    match state.version {
        STATE_VERSION => return true,
        4 => migrate_v4_state(state),
        5 => {}
        6 | 7 => {}
        _ => return false,
    }
    if state.version <= 5 {
        migrate_v5_state(state);
    }
    state.version = STATE_VERSION;
    state.gap_logs.clear();
    state.gaps.clear();
    true
}

/// Upgrade a version 5 cache. It holds no `EventData::Summary`: #59's
/// summary events had no caller, so no log held one, and a cached event of
/// #59's shape (no record key or harness) fails to deserialize, which makes
/// the whole cache unreadable and rescanned. Version 6 changes summary
/// parse acceptance (the record key and harness, adapter artifact types)
/// and summary derivation, and the same release changed how every artifact
/// file renders (front matter through `markdown::yaml_string`, the Why line
/// through `markdown::inline`), so every artifact becomes dirty: the next
/// regeneration rewrites each file whose rendering changed and leaves the
/// rest unchanged. The checkpoints are kept, so a log a version 5 build
/// checkpointed after skipping a version 6 summary marker (a downgrade,
/// then an upgrade) is not reread until it changes; `brauser reindex`
/// recovers that summary.
pub(super) fn migrate_v5_state(state: &mut IndexState) {
    let artifacts = state
        .events
        .values()
        .map(|event| event.artifact_id().to_owned())
        .collect::<Vec<_>>();
    state.dirty.extend(artifacts);
    state.version = STATE_VERSION;
}

/// Upgrade a version 4 cache to version 5: drop activity a log rescan
/// would now skip (a title over capture's limit), and mark those artifacts
/// and every M1 visit's artifact dirty so their owned files are regenerated
/// with the v5 derivation (M1 visits count as opens).
pub(super) fn migrate_v4_state(state: &mut IndexState) {
    let dropped = state
        .events
        .iter()
        .filter(|(_, event)| {
            matches!(&event.data, EventData::Activity(event) if !logged_activity_is_bounded(event))
        })
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in dropped {
        if let Some(event) = state.events.remove(&id) {
            state.dirty.insert(event.artifact_id().to_owned());
        }
        state.positions.remove(&id);
        state.sequence.remove(&id);
    }
    for event in state.events.values() {
        if let EventData::Legacy(event) = &event.data {
            state.dirty.insert(event.artifact_id.clone());
        }
    }
    state.version = 5;
}
