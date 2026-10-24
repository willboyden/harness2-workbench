//! Fuzzy matching for the command palette (Ctrl/Cmd+K).

/// Score how well `query` matches `text` as a case-insensitive subsequence. Higher is
/// better; `None` when some query character is missing. Consecutive matches, matches at
/// word starts and a match at the very start score extra.
pub(crate) fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut qi = 0;
    let mut total = 0_i32;
    let mut prev_match: Option<usize> = None;
    for (i, c) in t.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            let mut s = 1;
            if prev_match.is_some_and(|p| p + 1 == i) {
                s += 4;
            }
            if i == 0 {
                s += 6;
            } else if matches!(t[i - 1], ' ' | '-' | '_' | '/' | '.' | '·') {
                s += 3;
            }
            total += s;
            prev_match = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    // Prefer shorter texts among equal matches.
    let len = i32::try_from(t.len()).unwrap_or(i32::MAX);
    Some(total * 100 - len)
}

/// Indices of `items` matching `query`, best first (stable for equal scores).
pub(crate) fn rank<'a>(query: &str, items: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, t)| score(query, t).map(|s| (i, s)))
        .collect();
    if !query.trim().is_empty() {
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    }
    scored.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matching() {
        assert!(score("jb", "Go to Jobs").is_some());
        assert!(score("xyz", "Go to Jobs").is_none());
        assert_eq!(score("", "anything"), Some(0));
        assert!(score("new", "New run") > score("new", "Go to Benchmarks: new setup"));
    }

    #[test]
    fn ranking_prefers_word_starts_and_runs() {
        let items = [
            "Open settings",
            "Go to Jobs",
            "Start the demo",
            "Jobs: lab-pilot run",
        ];
        let r = rank("jobs", items);
        assert_eq!(r.first().copied(), Some(3));
        assert!(r.contains(&1));
        assert!(!r.contains(&2));
        assert_eq!(rank("", items), vec![0, 1, 2, 3]);
    }
}
