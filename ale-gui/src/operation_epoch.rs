use std::sync::atomic::{AtomicU64, Ordering};

/// Invalidates a permission/start operation synchronously, before async cleanup runs.
#[derive(Default)]
pub(crate) struct OperationEpoch(AtomicU64);
impl OperationEpoch {
    pub fn advance(&self) -> u64 {
        self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }
    pub fn is_current(&self, ticket: u64) -> bool {
        self.0.load(Ordering::Acquire) == ticket
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_permission_cannot_start_a_later_recording() {
        let epoch = OperationEpoch::default();
        let old = epoch.advance();
        epoch.advance(); // background/cancel
        let new = epoch.advance(); // another click after resume
        assert!(!epoch.is_current(old));
        assert!(epoch.is_current(new));
    }
}
