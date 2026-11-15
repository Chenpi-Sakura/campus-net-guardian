use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(650);

/// 只管理草稿版本和保存时机，不读写配置，也不持有窗口或后台。
#[derive(Default)]
pub(super) struct Autosave {
    revision: u64,
    saved: u64,
    deadline: Option<Instant>,
    in_flight: Option<u64>,
}

impl Autosave {
    pub(super) fn edit(&mut self, now: Instant) {
        self.revision = self.revision.wrapping_add(1);
        self.deadline = Some(now + DEBOUNCE);
    }

    pub(super) fn expedite(&mut self, now: Instant) {
        if self.deadline.is_some() {
            self.deadline = Some(now);
        }
    }

    pub(super) fn begin(&mut self, now: Instant) -> Option<u64> {
        if self.in_flight.is_some()
            || !self.is_dirty()
            || self.deadline.is_none_or(|deadline| now < deadline)
        {
            return None;
        }
        self.deadline = None;
        self.in_flight = Some(self.revision);
        Some(self.revision)
    }

    pub(super) fn finish(&mut self, success: bool) {
        if let Some(revision) = self.in_flight.take()
            && success
        {
            self.saved = revision;
        }
        // 失败不重新安排同一版本；保存期间的新编辑已有独立 deadline。
    }

    pub(super) fn is_current(&self, revision: u64) -> bool {
        self.revision == revision
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.revision != self.saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_restarts_after_each_edit() {
        let now = Instant::now();
        let mut save = Autosave::default();
        assert_eq!(save.begin(now + DEBOUNCE), None);
        save.edit(now);
        save.edit(now + Duration::from_millis(500));
        assert_eq!(save.begin(now + DEBOUNCE), None);
        assert_eq!(save.begin(now + Duration::from_millis(1150)), Some(2));
    }

    #[test]
    fn acknowledgement_only_saves_its_version_and_coalesces_newer_edits() {
        let now = Instant::now();
        let mut save = Autosave::default();
        save.edit(now);
        let first = save.begin(now + DEBOUNCE).unwrap();
        save.edit(now + DEBOUNCE);
        save.edit(now + DEBOUNCE * 2);
        assert!(!save.is_current(first));
        assert_eq!(save.begin(now + DEBOUNCE * 3), None);
        save.finish(true);
        assert!(save.is_dirty());
        assert_eq!(save.begin(now + DEBOUNCE * 3), Some(3));
        save.finish(true);
        assert!(!save.is_dirty());
        assert_eq!(save.begin(now + DEBOUNCE * 4), None);
    }

    #[test]
    fn failure_keeps_draft_without_retrying_until_another_edit() {
        let now = Instant::now();
        let mut save = Autosave::default();
        save.edit(now);
        assert_eq!(save.begin(now + DEBOUNCE), Some(1));
        save.finish(false);
        assert!(save.is_dirty());
        save.expedite(now + DEBOUNCE * 2);
        assert_eq!(save.begin(now + DEBOUNCE * 20), None);
        save.edit(now + DEBOUNCE * 20);
        assert_eq!(save.begin(now + DEBOUNCE * 21), Some(2));
    }

    #[test]
    fn failure_of_older_version_does_not_discard_newer_pending_draft() {
        let now = Instant::now();
        let mut save = Autosave::default();
        save.edit(now);
        save.begin(now + DEBOUNCE).unwrap();
        save.edit(now + DEBOUNCE);
        save.finish(false);
        assert_eq!(save.begin(now + DEBOUNCE * 2), Some(2));
    }

    #[test]
    fn returning_from_settings_can_flush_without_replacing_draft() {
        let now = Instant::now();
        let mut save = Autosave::default();
        save.edit(now);
        save.expedite(now);
        assert_eq!(save.begin(now), Some(1));
    }
}
