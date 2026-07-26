use crate::status::RateLimitSnapshotDisplay;
use codex_login::AuthProfile;
use std::cmp::Ordering;
use std::collections::BTreeMap;

pub(super) enum AuthProfileSelectionTarget {
    Default,
    Named(AuthProfile),
}

pub(super) struct AuthProfileSelectionEntry {
    pub(super) target: AuthProfileSelectionTarget,
    pub(super) usage_status: AuthProfileUsageStatus,
    pub(super) original_index: usize,
}

/// Identity of a rendered profile-popup row. The popup is re-sorted whenever new
/// usage lands, so an in-place refresh has to restore the cursor by identity
/// rather than by index (otherwise the cursor silently lands on a different
/// profile).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuthProfilePopupRow {
    GroupHeader,
    Default,
    Named(String),
    NewProfile,
}

impl AuthProfileSelectionTarget {
    pub(super) fn popup_row(&self) -> AuthProfilePopupRow {
        match self {
            Self::Default => AuthProfilePopupRow::Default,
            Self::Named(profile) => AuthProfilePopupRow::Named(profile.name.clone()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthProfileUsageGroup {
    Active,
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AuthProfileUsageStatus {
    pub(super) group: AuthProfileUsageGroup,
    remaining_percent: Option<f64>,
}

impl AuthProfileUsageStatus {
    pub(super) fn unknown() -> Self {
        Self {
            group: AuthProfileUsageGroup::Active,
            remaining_percent: None,
        }
    }

    pub(super) fn for_snapshots(snapshots: &BTreeMap<String, RateLimitSnapshotDisplay>) -> Self {
        let Some(snapshot) = usage_snapshot_with_windows(snapshots) else {
            return Self::unknown();
        };
        let Some(remaining_percent) = limiting_remaining_percent(snapshot) else {
            return Self::unknown();
        };
        let group = if remaining_percent <= 0.0 {
            AuthProfileUsageGroup::Exhausted
        } else {
            AuthProfileUsageGroup::Active
        };
        Self {
            group,
            remaining_percent: Some(remaining_percent),
        }
    }
}

pub(super) fn sort_auth_profile_selection_entries(entries: &mut [AuthProfileSelectionEntry]) {
    entries.sort_by(|left, right| {
        auth_profile_usage_group_rank(left.usage_status.group)
            .cmp(&auth_profile_usage_group_rank(right.usage_status.group))
            .then_with(|| {
                if left.usage_status.group == AuthProfileUsageGroup::Active {
                    compare_remaining_percent_desc(
                        left.usage_status.remaining_percent,
                        right.usage_status.remaining_percent,
                    )
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| left.original_index.cmp(&right.original_index))
    });
}

pub(super) fn usage_snapshot_with_windows(
    snapshots: &BTreeMap<String, RateLimitSnapshotDisplay>,
) -> Option<&RateLimitSnapshotDisplay> {
    snapshots
        .get("codex")
        .filter(|snapshot| usage_snapshot_has_windows(snapshot))
        .or_else(|| {
            snapshots
                .values()
                .find(|snapshot| usage_snapshot_has_windows(snapshot))
        })
}

fn limiting_remaining_percent(snapshot: &RateLimitSnapshotDisplay) -> Option<f64> {
    [snapshot.primary.as_ref(), snapshot.secondary.as_ref()]
        .into_iter()
        .flatten()
        .map(|window| (100.0 - window.used_percent).clamp(0.0, 100.0))
        .reduce(f64::min)
}

fn auth_profile_usage_group_rank(group: AuthProfileUsageGroup) -> u8 {
    match group {
        AuthProfileUsageGroup::Active => 0,
        AuthProfileUsageGroup::Exhausted => 1,
    }
}

fn compare_remaining_percent_desc(left: Option<f64>, right: Option<f64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.total_cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn usage_snapshot_has_windows(snapshot: &RateLimitSnapshotDisplay) -> bool {
    snapshot.primary.is_some() || snapshot.secondary.is_some()
}
