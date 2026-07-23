//! A string-keyed table for ruleset entries.
//!
//! The ruleset is consulted constantly: every unit action looks up its unit
//! spec, every sight ray its feature spec, every production check every
//! building in the game. A `BTreeMap<String, _>` charges a string comparison
//! per level of the tree for each of those, and profiling a simulated game
//! found string comparison to be the single largest cost in the engine.
//!
//! `SpecMap` keeps its keys sorted, so iteration order — which saves,
//! observations, and per-seed determinism all depend on — is exactly what the
//! `BTreeMap` gave. Beside them it keeps an open-addressed hash table, so a
//! lookup costs one comparison instead of seven. Hashing is only ever used to
//! find an entry, never to order one, so nothing about a game's outcome
//! depends on it.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

const EMPTY: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub struct SpecMap<T> {
    keys: Vec<String>,
    values: Vec<T>,
    table: Vec<u32>,
}

/// A multiply-shift hash over whole eight-byte words. Ruleset keys are short
/// identifiers, so this is two multiplies for a typical name — cheaper than
/// the tree descent it replaces.
fn hash_key(key: &str) -> u64 {
    const SEED: u64 = 0x9E37_79B9_7F4A_7C15;
    let bytes = key.as_bytes();
    let mut hash = SEED ^ (bytes.len() as u64);
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes(chunk.try_into().unwrap());
        hash = (hash ^ word).wrapping_mul(SEED);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut word = [0u8; 8];
        word[..rest.len()].copy_from_slice(rest);
        hash = (hash ^ u64::from_le_bytes(word)).wrapping_mul(SEED);
    }
    hash ^ (hash >> 31)
}

impl<T> Default for SpecMap<T> {
    fn default() -> Self {
        SpecMap {
            keys: Vec::new(),
            values: Vec::new(),
            table: Vec::new(),
        }
    }
}

impl<T> SpecMap<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Rebuild the lookup table. Called whenever an entry is added or removed,
    /// which happens when the ruleset is loaded and when a mod patches it —
    /// never during play.
    fn reindex(&mut self) {
        let capacity = (self.keys.len() * 2).next_power_of_two().max(8);
        self.table = vec![EMPTY; capacity];
        let mask = capacity - 1;
        for (index, key) in self.keys.iter().enumerate() {
            let mut slot = hash_key(key) as usize & mask;
            while self.table[slot] != EMPTY {
                slot = (slot + 1) & mask;
            }
            self.table[slot] = index as u32;
        }
    }

    #[inline]
    fn position(&self, key: &str) -> Option<usize> {
        if self.table.is_empty() {
            return None;
        }
        let mask = self.table.len() - 1;
        let mut slot = hash_key(key) as usize & mask;
        loop {
            let index = self.table[slot];
            if index == EMPTY {
                return None;
            }
            if self.keys[index as usize] == key {
                return Some(index as usize);
            }
            slot = (slot + 1) & mask;
        }
    }

    #[inline]
    pub fn get(&self, key: &str) -> Option<&T> {
        self.position(key).map(|index| &self.values[index])
    }

    #[inline]
    pub fn get_mut(&mut self, key: &str) -> Option<&mut T> {
        self.position(key).map(|index| &mut self.values[index])
    }

    #[inline]
    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    pub fn insert(&mut self, key: String, value: T) -> Option<T> {
        match self.keys.binary_search(&key) {
            Ok(index) => Some(std::mem::replace(&mut self.values[index], value)),
            Err(index) => {
                self.keys.insert(index, key);
                self.values.insert(index, value);
                self.reindex();
                None
            }
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<T> {
        let index = self.keys.binary_search_by(|probe| probe.as_str().cmp(key)).ok()?;
        self.keys.remove(index);
        let value = self.values.remove(index);
        self.reindex();
        Some(value)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&str, &mut T) -> bool) {
        let mut index = 0;
        let mut kept = Vec::with_capacity(self.keys.len());
        self.values.retain_mut(|value| {
            let keeping = keep(&self.keys[index], value);
            kept.push(keeping);
            index += 1;
            keeping
        });
        let mut decision = kept.into_iter();
        self.keys.retain(|_| decision.next().unwrap_or(true));
        self.reindex();
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &String> + ExactSizeIterator {
        self.keys.iter()
    }

    pub fn values(&self) -> std::slice::Iter<'_, T> {
        self.values.iter()
    }

    pub fn values_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.values.iter_mut()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut T)> {
        self.keys.iter().zip(self.values.iter_mut())
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.values.clear();
        self.table.clear();
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&String, &T)> + ExactSizeIterator {
        self.keys.iter().zip(self.values.iter())
    }

    pub fn into_values(self) -> std::vec::IntoIter<T> {
        self.values.into_iter()
    }
}

impl<T> std::ops::Index<&str> for SpecMap<T> {
    type Output = T;

    #[inline]
    fn index(&self, key: &str) -> &T {
        self.get(key)
            .unwrap_or_else(|| panic!("no ruleset entry named {key:?}"))
    }
}

impl<T> std::ops::Index<&String> for SpecMap<T> {
    type Output = T;

    #[inline]
    fn index(&self, key: &String) -> &T {
        &self[key.as_str()]
    }
}

impl<'a, T> IntoIterator for &'a SpecMap<T> {
    type Item = (&'a String, &'a T);
    type IntoIter = std::iter::Zip<std::slice::Iter<'a, String>, std::slice::Iter<'a, T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.keys.iter().zip(self.values.iter())
    }
}

impl<T> FromIterator<(String, T)> for SpecMap<T> {
    fn from_iter<I: IntoIterator<Item = (String, T)>>(iter: I) -> Self {
        let ordered: BTreeMap<String, T> = iter.into_iter().collect();
        SpecMap::from(ordered)
    }
}

impl<T> Extend<(String, T)> for SpecMap<T> {
    fn extend<I: IntoIterator<Item = (String, T)>>(&mut self, iter: I) {
        for (key, value) in iter {
            self.insert(key, value);
        }
    }
}

impl<T> From<BTreeMap<String, T>> for SpecMap<T> {
    fn from(entries: BTreeMap<String, T>) -> Self {
        let mut map = SpecMap {
            keys: Vec::with_capacity(entries.len()),
            values: Vec::with_capacity(entries.len()),
            table: Vec::new(),
        };
        for (key, value) in entries {
            map.keys.push(key);
            map.values.push(value);
        }
        map.reindex();
        map
    }
}

impl<T: PartialEq> PartialEq for SpecMap<T> {
    fn eq(&self, other: &Self) -> bool {
        self.keys == other.keys && self.values == other.values
    }
}

impl<T: Serialize> Serialize for SpecMap<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.iter())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for SpecMap<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BTreeMap::<String, T>::deserialize(deserializer).map(SpecMap::from)
    }
}

#[cfg(test)]
mod tests {
    use super::SpecMap;

    #[test]
    fn keeps_entries_in_key_order() {
        let mut map = SpecMap::new();
        map.insert("warrior".to_string(), 1);
        map.insert("archer".to_string(), 2);
        map.insert("slinger".to_string(), 3);
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        assert_eq!(keys, ["archer", "slinger", "warrior"]);
        assert_eq!(map.values().copied().collect::<Vec<_>>(), [2, 3, 1]);
    }

    #[test]
    fn finds_replaces_and_removes() {
        let mut map = SpecMap::new();
        for name in ["a", "bb", "ccc", "dddddddd", "eeeeeeeee", "f"] {
            map.insert(name.to_string(), name.len());
        }
        for name in ["a", "bb", "ccc", "dddddddd", "eeeeeeeee", "f"] {
            assert_eq!(map.get(name), Some(&name.len()));
            assert!(map.contains_key(name));
        }
        assert_eq!(map.get("missing"), None);
        assert_eq!(map.insert("bb".to_string(), 99), Some(2));
        assert_eq!(map["bb"], 99);
        assert_eq!(map.remove("bb"), Some(99));
        assert_eq!(map.get("bb"), None);
        assert_eq!(map.len(), 5);
    }
}
