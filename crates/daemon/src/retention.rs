//! What ainetd keeps, as `retention` in settings.json says: in-memory caps
//! applied as soon as they change, and, when asked for, logged events
//! pruned by age or count and answered inbox items dropped once old. By
//! default nothing is pruned, as before.

use std::time::Duration;

use ainet_core::{InboxItem, now_ms};

use crate::{Daemon, Shared, lock};

/// How often events and the inbox are pruned (also once at start).
const EVERY: Duration = Duration::from_secs(60);

impl Daemon {
    /// Puts new settings to work where they apply to what is already
    /// there.
    pub(crate) fn settings_changed(&mut self) {
        self.trim_messages();
        self.trim_chat();
        self.trim_inbox();
        self.prune();
    }

    /// Prunes logged events and expires inbox items, as far as asked.
    fn prune(&mut self) {
        let k = self.settings().retention.clone();
        let now = now_ms();
        let before = k
            .events_max_age_days
            .map(|days| now - i64::from(days) * 86_400_000);
        if before.is_some() || k.events_max_count.is_some() {
            let gone = self.store.prune_events(before, k.events_max_count);
            if gone > 0 {
                tracing::info!("pruned {gone} logged events");
            }
        }
        if let Some(mins) = k.inbox_expiry_mins {
            expire(&mut self.inbox, now - (mins as i64) * 60_000);
        }
    }
}

/// Drops answered inbox items made before `before`; waiting ones stay.
fn expire(inbox: &mut Vec<InboxItem>, before: i64) {
    inbox.retain(|i| i.waiting || i.created >= before);
}

pub(crate) async fn ticker(d: Shared) {
    loop {
        lock(&d).prune();
        tokio::time::sleep(EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use ainet_core::InboxKind;

    use super::*;

    fn item(id: u64, created: i64, waiting: bool) -> InboxItem {
        InboxItem {
            id,
            agent: "a".into(),
            kind: InboxKind::Question,
            summary: String::new(),
            detail: String::new(),
            created,
            waiting,
            options: vec![],
            multi_select: false,
        }
    }

    #[test]
    fn old_answered_items_expire_and_waiting_ones_never() {
        let mut inbox = vec![item(1, 10, false), item(2, 10, true), item(3, 50, false)];
        expire(&mut inbox, 20);
        let ids: Vec<u64> = inbox.iter().map(|i| i.id).collect();
        assert_eq!(ids, [2, 3]);
    }
}
