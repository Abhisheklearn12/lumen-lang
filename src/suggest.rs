//! "Did you mean …?" suggestions.
//!
//! [`closest`] offers the candidate with the smallest Levenshtein distance to a
//! misspelled name, but only within a threshold that grows with its length, so
//! unrelated names are never suggested.

/// The candidate nearest to `target`, if within the threshold. Ties go to the
/// earliest candidate, so pass candidates in a deterministic order.
pub fn closest<'a>(target: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let max_distance = threshold(target.len());
    let mut best: Option<(&str, usize)> = None;
    for candidate in candidates {
        let distance = levenshtein(target, candidate);
        if distance <= max_distance && best.is_none_or(|(_, b)| distance < b) {
            best = Some((candidate, distance));
        }
    }
    best.map(|(name, _)| name)
}

/// The largest edit distance accepted for a target of `len` bytes.
fn threshold(len: usize) -> usize {
    match len {
        0..=2 => 1,
        3..=5 => 2,
        _ => 3,
    }
}

/// Levenshtein distance over `char`s, using two rows of the DP table.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1) // deletion
                .min(curr[j] + 1) // insertion
                .min(prev[j] + cost); // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("flaw", "lawn"), 2);
    }

    #[test]
    fn suggests_near_misses() {
        let names = ["print_int", "print_str", "main"];
        assert_eq!(closest("print_itn", names), Some("print_int"));
        assert_eq!(closest("mian", names), Some("main"));
    }

    #[test]
    fn does_not_suggest_unrelated_names() {
        let names = ["print_int", "main"];
        assert_eq!(closest("xyzzy", names), None);
    }

    #[test]
    fn short_names_tolerate_one_edit_only() {
        assert_eq!(closest("ab", ["ac"]), Some("ac")); // distance 1, ok
        assert_eq!(closest("ab", ["cd"]), None); // distance 2, too far
    }
}
