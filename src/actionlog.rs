//! The record of every action a game has applied.
//!
//! The log is a complete replay record — applying it to a fresh game of the
//! same seed reproduces the game exactly — so it cannot be trimmed. But a game
//! is also cloned once per branch the AI searches, thousands of times a turn,
//! and copying every action ever taken made each of those clones cost more the
//! longer the game had run.
//!
//! Actions are only ever appended, and a branch that adds one does not disturb
//! what came before it, so the log is held as a chain that shares its history.
//! Cloning one is a reference count; appending to a clone leaves the original
//! untouched.

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::game::Action;

/// One applied action, and everything applied before it.
struct Entry {
    previous: Option<Arc<Entry>>,
    applied: (usize, Action),
}

#[derive(Clone, Default)]
pub struct ActionLog {
    last: Option<Arc<Entry>>,
    len: usize,
}

impl ActionLog {
    pub fn new() -> ActionLog {
        ActionLog::default()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, seat: usize, action: Action) {
        self.last = Some(Arc::new(Entry {
            previous: self.last.take(),
            applied: (seat, action),
        }));
        self.len += 1;
    }

    pub fn last(&self) -> Option<&(usize, Action)> {
        self.last.as_deref().map(|entry| &entry.applied)
    }

    /// Every action, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &(usize, Action)> + '_ {
        self.chain().into_iter().map(|entry| &entry.applied)
    }

    /// The actions applied since the log was `start` entries long.
    pub fn since(&self, start: usize) -> impl DoubleEndedIterator<Item = &(usize, Action)> + '_ {
        self.chain().into_iter().skip(start.min(self.len)).map(|entry| &entry.applied)
    }

    /// The chain walked out into oldest-first order.
    fn chain(&self) -> Vec<&Entry> {
        let mut entries = Vec::with_capacity(self.len);
        let mut cursor = self.last.as_deref();
        while let Some(entry) = cursor {
            entries.push(entry);
            cursor = entry.previous.as_deref();
        }
        entries.reverse();
        entries
    }
}

impl Drop for ActionLog {
    fn drop(&mut self) {
        // A uniquely owned persistent log is a linked list of Arcs. Letting the
        // default Arc destructor walk that list recurses once per action, which
        // can overflow the stack after a long game. Peel off unique entries
        // iteratively instead. If a branch still shares the history, its owner
        // will finish the cleanup when that history becomes unique.
        let mut cursor = self.last.take();
        while let Some(entry) = cursor {
            match Arc::try_unwrap(entry) {
                Ok(mut entry) => cursor = entry.previous.take(),
                Err(_) => break,
            }
        }
    }
}

impl std::fmt::Debug for ActionLog {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for ActionLog {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self
                .iter()
                .zip(other.iter())
                .all(|(left, right)| left == right)
    }
}

impl FromIterator<(usize, Action)> for ActionLog {
    fn from_iter<I: IntoIterator<Item = (usize, Action)>>(entries: I) -> ActionLog {
        let mut log = ActionLog::new();
        for (seat, action) in entries {
            log.push(seat, action);
        }
        log
    }
}

impl Serialize for ActionLog {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for ActionLog {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<(usize, Action)>::deserialize(deserializer).map(ActionLog::from_iter)
    }
}

#[cfg(test)]
mod tests {
    use super::ActionLog;
    use crate::game::Action;

    fn end(seat: usize) -> (usize, Action) {
        (seat, Action::EndTurn)
    }

    #[test]
    fn keeps_actions_in_the_order_they_were_applied() {
        let log: ActionLog = (0..5).map(end).collect();
        assert_eq!(log.len(), 5);
        assert_eq!(
            log.iter().map(|(seat, _)| *seat).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
        assert_eq!(log.last().map(|(seat, _)| *seat), Some(4));
        assert_eq!(
            log.since(2).map(|(seat, _)| *seat).collect::<Vec<_>>(),
            [2, 3, 4]
        );
    }

    #[test]
    fn a_branch_does_not_disturb_what_it_was_branched_from() {
        let mut trunk: ActionLog = (0..3).map(end).collect();
        let mut branch = trunk.clone();
        branch.push(9, Action::EndTurn);
        trunk.push(7, Action::EndTurn);
        assert_eq!(branch.len(), 4);
        assert_eq!(trunk.len(), 4);
        assert_eq!(branch.last().map(|(seat, _)| *seat), Some(9));
        assert_eq!(trunk.last().map(|(seat, _)| *seat), Some(7));
        assert_eq!(
            trunk.iter().map(|(seat, _)| *seat).collect::<Vec<_>>(),
            [0, 1, 2, 7]
        );
    }

    #[test]
    fn long_branched_logs_drop_without_recursing() {
        std::thread::Builder::new()
            .name("action-log-drop".to_string())
            .stack_size(64 * 1024)
            .spawn(|| {
                let trunk: ActionLog = (0..30_000).map(|_| end(0)).collect();
                let mut branch = trunk.clone();
                branch.push(1, Action::EndTurn);

                drop(trunk);
                drop(branch);
            })
            .expect("drop-test thread should start")
            .join()
            .expect("dropping a long action log should not overflow its stack");
    }
}
