#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueHealthSnapshot {
    pub capacity: usize,
    pub depth: usize,
    pub dropped: u64,
}

impl QueueHealthSnapshot {
    #[must_use]
    pub const fn new(capacity: usize, depth: usize, dropped: u64) -> Self {
        Self {
            capacity,
            depth,
            dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::QueueHealthSnapshot;

    #[test]
    fn queue_health_keeps_capacity_depth_and_drops_separate() {
        let snapshot = QueueHealthSnapshot::new(64, 17, 2);
        assert_eq!(snapshot.capacity, 64);
        assert_eq!(snapshot.depth, 17);
        assert_eq!(snapshot.dropped, 2);
    }
}
