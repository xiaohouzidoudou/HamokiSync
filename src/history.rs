use std::collections::VecDeque;

const MAX_ITEMS: usize = 10;
const MAX_MEMORY: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemData {
    Text(String),
    Png(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    pub fingerprint: String,
    pub data: ItemData,
}

impl HistoryItem {
    pub fn byte_len(&self) -> usize {
        match &self.data {
            ItemData::Text(text) => text.len(),
            ItemData::Png(bytes) => bytes.len(),
        }
    }
}

/// Memory-only history: never writes copied secrets or pixels to disk.
/// Maximum 10 unique items and 32 MiB total retained payload bytes.
#[derive(Debug, Default)]
pub struct History {
    entries: VecDeque<HistoryItem>,
    memory_bytes: usize,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, item: HistoryItem) -> bool {
        let size = item.byte_len();
        if size == 0 || size > MAX_MEMORY {
            return false;
        }
        if self.entries.front().map(|i| &i.fingerprint) == Some(&item.fingerprint) {
            return false;
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|i| i.fingerprint == item.fingerprint)
        {
            self.remove(index);
        }
        self.memory_bytes += size;
        self.entries.push_front(item);
        while self.entries.len() > MAX_ITEMS || self.memory_bytes > MAX_MEMORY {
            if let Some(last) = self.entries.pop_back() {
                self.memory_bytes -= last.byte_len();
            }
        }
        true
    }

    fn remove(&mut self, index: usize) {
        if let Some(item) = self.entries.remove(index) {
            self.memory_bytes -= item.byte_len();
        }
    }

    pub fn latest(&self) -> Option<&HistoryItem> {
        self.entries.front()
    }

    pub fn previous(&self) -> Option<&HistoryItem> {
        self.entries.get(1)
    }

    pub fn get(&self, index: usize) -> Option<&HistoryItem> {
        self.entries.get(index)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.memory_bytes
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.memory_bytes = 0;
    }

    pub fn iter(&self) -> impl Iterator<Item = &HistoryItem> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(n: usize) -> HistoryItem {
        HistoryItem {
            fingerprint: format!("T:{n}"),
            data: ItemData::Text(format!("Test line {n}")),
        }
    }
    #[test]
    fn newest_first_and_oldest_dropped_at_ten() {
        let mut history = History::new();
        for n in 0..12 {
            assert!(history.push(text(n)));
        }
        assert_eq!(history.len(), 10);
        assert_eq!(history.latest().unwrap().fingerprint, "T:11");
        assert_eq!(history.previous().unwrap().fingerprint, "T:10");
        assert_eq!(history.get(9).unwrap().fingerprint, "T:2");
        assert!(!history.push(text(11)));
        assert_eq!(history.len(), 10);
    }
    #[test]
    fn duplicated_content_moves_to_front() {
        let mut h = History::new();
        h.push(text(1));
        h.push(text(2));
        h.push(text(1));
        assert_eq!(h.len(), 2);
        assert_eq!(h.latest().unwrap().fingerprint, "T:1");
    }
    #[test]
    fn large_image_budget_evicts_older_items() {
        let mut h = History::new();
        for n in 0..5 {
            h.push(HistoryItem {
                fingerprint: n.to_string(),
                data: ItemData::Png(vec![0x42; 9 * 1024 * 1024]),
            });
        }
        assert!(h.len() <= 3);
        assert!(h.bytes() <= 32 * 1024 * 1024);
        h.clear();
        assert_eq!(h.bytes(), 0);
    }
}
