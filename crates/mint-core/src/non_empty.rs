//! Lists that always hold at least one item.

use std::ops::{Deref, DerefMut};

/// A list with at least one item. It reads and writes items as a slice, but
/// only its own methods change how many it holds, and none empties it.
#[derive(Clone, Debug, PartialEq)]
pub struct NonEmpty<T>(Vec<T>);

impl<T> NonEmpty<T> {
    /// `items`, unless there are none.
    #[must_use]
    pub fn from_vec(items: Vec<T>) -> Option<Self> {
        (!items.is_empty()).then_some(Self(items))
    }

    /// The list of `item` alone.
    #[must_use]
    pub fn one(item: T) -> Self {
        Self(vec![item])
    }

    #[must_use]
    pub fn first(&self) -> &T {
        &self.0[0]
    }

    /// The list with each item replaced by `f` of it.
    #[must_use]
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> NonEmpty<U> {
        NonEmpty(self.0.into_iter().map(f).collect())
    }

    /// `f` of the first item, combined in order with `f` of each other
    /// item by `combine`.
    pub fn fold_map<U>(&self, mut f: impl FnMut(&T) -> U, combine: impl Fn(U, U) -> U) -> U {
        self.0[1..]
            .iter()
            .fold(f(self.first()), |acc, item| combine(acc, f(item)))
    }
}

impl<T> Deref for NonEmpty<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> DerefMut for NonEmpty<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<T> IntoIterator for NonEmpty<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a NonEmpty<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_empty_list_has_a_first_item() {
        assert_eq!(NonEmpty::<u8>::from_vec(Vec::new()), None);
        let list = NonEmpty::from_vec(vec![3, 1, 2]).unwrap();
        assert_eq!(*list.first(), 3);
        assert_eq!(list.len(), 3);
        assert_eq!(list.fold_map(|&n| n, u8::max), 3);
        assert_eq!(
            list.map(|n| n * 2).into_iter().collect::<Vec<_>>(),
            [6, 2, 4]
        );
        assert_eq!(NonEmpty::one(7).fold_map(|&n| n, u8::min), 7);
    }
}
