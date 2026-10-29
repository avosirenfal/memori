//! Small collection helpers used across layers.

use std::cmp::Ordering;

/// A list with at least one element, so "first" and "only" never fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmpty<T> {
    first: T,
    rest: Vec<T>,
}

impl<T> NonEmpty<T> {
    pub fn from_vec(items: Vec<T>) -> Option<Self> {
        let mut items = items.into_iter();
        Some(Self { first: items.next()?, rest: items.collect() })
    }

    pub fn first(&self) -> &T {
        &self.first
    }

    /// The element, when there is exactly one.
    pub fn only(&self) -> Option<&T> {
        self.rest.is_empty().then_some(&self.first)
    }

    pub fn len(&self) -> usize {
        1 + self.rest.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&self.first).chain(&self.rest)
    }
}

/// Returns `items` sorted by `key` (stable).
pub fn sorted_by_key<T, K: Ord>(items: Vec<T>, key: impl FnMut(&T) -> K) -> Vec<T> {
    let mut items = items;
    items.sort_by_key(key);
    items
}

/// Returns `items` sorted by `compare` (stable).
pub fn sorted_by<T>(items: Vec<T>, compare: impl FnMut(&T, &T) -> Ordering) -> Vec<T> {
    let mut items = items;
    items.sort_by(compare);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty_refuses_empty_lists() {
        assert_eq!(NonEmpty::<u8>::from_vec(vec![]), None);
        let one = NonEmpty::from_vec(vec![7]).unwrap();
        assert_eq!(one.only(), Some(&7));
        let two = NonEmpty::from_vec(vec![1, 2]).unwrap();
        assert_eq!(two.only(), None);
        assert_eq!(two.iter().copied().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(two.len(), 2);
    }
}
