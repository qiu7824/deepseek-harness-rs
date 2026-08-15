//! Array set and normalization helpers (port of `src/array.ts`).

use std::hash::Hash;

use indexmap::IndexSet;

/// Return true when every item in `array2` is present in `array1`.
pub fn contain<T: PartialEq>(array1: &[T], array2: &[T]) -> bool {
    array2.iter().all(|item| array1.contains(item))
}

/// Return items that appear in both arrays (order follows `array1`).
pub fn intersection<T: PartialEq + Clone>(array1: &[T], array2: &[T]) -> Vec<T> {
    array1
        .iter()
        .filter(|item| array2.contains(item))
        .cloned()
        .collect()
}

/// Return items from `array1` that do not appear in `array2`.
pub fn difference<T: PartialEq + Clone>(array1: &[T], array2: &[T]) -> Vec<T> {
    array1
        .iter()
        .filter(|item| !array2.contains(item))
        .cloned()
        .collect()
}

/// Return the set-union of two arrays while preserving first occurrence
/// order.
pub fn union<T: Eq + Hash + Clone>(array1: &[T], array2: &[T]) -> Vec<T> {
    let mut set: IndexSet<T> = IndexSet::new();
    set.extend(array1.iter().cloned());
    set.extend(array2.iter().cloned());
    set.into_iter().collect()
}

/// Remove duplicate values while preserving first occurrence order.
pub fn deduplicate<T: Eq + Hash + Clone>(array: &[T]) -> Vec<T> {
    array.iter().cloned().collect::<IndexSet<T>>().into_iter().collect()
}

/// Remove one item from a list and report whether it was found.
pub fn remove<T: PartialEq>(list: &mut Vec<T>, item: &T) -> bool {
    match list.iter().position(|x| x == item) {
        Some(index) => {
            list.remove(index);
            true
        }
        None => false,
    }
}

/// Input accepted by [`make_array`]: one value or many (TS `T | T[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaybeArray<T> {
    One(T),
    Many(Vec<T>),
}

/// Normalize nullish, scalar, or array input to an array
/// (TS `makeArray(null | undefined | T | T[])`).
pub fn make_array<T>(source: Option<MaybeArray<T>>) -> Vec<T> {
    match source {
        None => Vec::new(),
        Some(MaybeArray::One(value)) => vec![value],
        Some(MaybeArray::Many(values)) => values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment_and_sets() {
        assert!(contain(&[1, 2, 3], &[2, 3]));
        assert!(!contain(&[1, 2], &[2, 3]));
        assert_eq!(intersection(&[1, 2, 3], &[2, 3, 4]), vec![2, 3]);
        assert_eq!(difference(&[1, 2, 3], &[2]), vec![1, 3]);
        assert_eq!(union(&[1, 2], &[2, 3]), vec![1, 2, 3]);
        assert_eq!(deduplicate(&[3, 1, 3, 2, 1]), vec![3, 1, 2]);
    }

    #[test]
    fn removal_and_normalization() {
        let mut list = vec![1, 2, 3];
        assert!(remove(&mut list, &2));
        assert_eq!(list, vec![1, 3]);
        assert!(!remove(&mut list, &9));
        assert!(make_array::<i32>(None).is_empty());
        assert_eq!(make_array(Some(MaybeArray::One(1))), vec![1]);
        assert_eq!(make_array(Some(MaybeArray::Many(vec![1, 2]))), vec![1, 2]);
    }
}
