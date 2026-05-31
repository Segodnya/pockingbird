//! Per-locale fuzzy clustering: BK-tree over non-empty values → neighbors within
//! `fuzzy_max_distance` → union-find → cluster representative as the canonical.
//!
//! The tier reduces global fuzzy matching to a hashable canonical: each value is
//! mapped to its cluster's representative (the lexicographically smallest member,
//! for determinism), and bucketing downstream reuses the exact/normalized
//! pipeline over those canonicals.
//!
//! Note: edit distance is not transitive, so a chain of ≤d steps can drift
//! further apart — union-find clusters are therefore approximate. This is
//! documented and acceptable for duplicate hunting.
//!
//! BK-tree and union-find are kept in-house here (small, no extra dependency).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use strsim::levenshtein;

// ---------------------------------------------------------------------------
// BK-tree (3.1)
// ---------------------------------------------------------------------------

/// A BK-tree over strings under the Levenshtein metric. Supports insertion and
/// "all terms within edit distance ≤ d" queries.
#[derive(Debug, Default)]
pub struct BkTree {
    root: Option<Node>,
}

#[derive(Debug)]
struct Node {
    term: String,
    /// edge distance → child subtree.
    children: BTreeMap<usize, Node>,
}

impl BkTree {
    /// Insert a term. Duplicate terms (distance 0) are ignored.
    pub fn insert(&mut self, term: String) {
        match &mut self.root {
            None => {
                self.root = Some(Node {
                    term,
                    children: BTreeMap::new(),
                });
            }
            Some(root) => insert_node(root, term),
        }
    }

    /// Every inserted term within Levenshtein distance `radius` of `term`.
    pub fn within(&self, term: &str, radius: usize) -> Vec<&str> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            query_node(root, term, radius, &mut out);
        }
        out
    }
}

fn insert_node(node: &mut Node, term: String) {
    let distance = levenshtein(&node.term, &term);
    if distance == 0 {
        return;
    }
    match node.children.get_mut(&distance) {
        Some(child) => insert_node(child, term),
        None => {
            node.children.insert(
                distance,
                Node {
                    term,
                    children: BTreeMap::new(),
                },
            );
        }
    }
}

fn query_node<'a>(node: &'a Node, term: &str, radius: usize, out: &mut Vec<&'a str>) {
    let distance = levenshtein(&node.term, term);
    if distance <= radius {
        out.push(&node.term);
    }
    // Triangle inequality: a match within `radius` of `term` sits on an edge
    // whose distance is within [distance - radius, distance + radius].
    let lo = distance.saturating_sub(radius);
    let hi = distance + radius;
    for (_, child) in node.children.range(lo..=hi) {
        query_node(child, term, radius, out);
    }
}

// ---------------------------------------------------------------------------
// Union-find (3.2)
// ---------------------------------------------------------------------------

/// Disjoint-set forest with union by rank and path halving.
#[derive(Debug)]
pub struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    pub fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
            rank: vec![0; size],
        }
    }

    pub fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]]; // path halving
            x = self.parent[x];
        }
        x
    }

    pub fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        match self.rank[ra].cmp(&self.rank[rb]) {
            Ordering::Less => self.parent[ra] = rb,
            Ordering::Greater => self.parent[rb] = ra,
            Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-locale clustering (3.3, 3.4)
// ---------------------------------------------------------------------------

/// Map each distinct value to its fuzzy cluster representative.
///
/// Values shorter than `min_length` (in Unicode scalars) skip the fuzzy tier:
/// their canonical is the value itself (min-length guard — distance ≤ d on short
/// strings merges genuinely different words like `On`/`Off`).
pub fn fuzzy_canonicals(
    values: &[String],
    max_distance: usize,
    min_length: usize,
) -> HashMap<String, String> {
    let distinct: Vec<&str> = values
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .collect();

    let mut canonical: HashMap<String, String> = HashMap::new();
    let mut eligible: Vec<&str> = Vec::new();
    for &value in &distinct {
        if value.chars().count() >= min_length {
            eligible.push(value);
        } else {
            // Short string: its own canonical, skips fuzzy entirely.
            canonical.insert(value.to_string(), value.to_string());
        }
    }

    if eligible.is_empty() {
        return canonical;
    }

    let mut tree = BkTree::default();
    for &value in &eligible {
        tree.insert(value.to_string());
    }
    let index_of: HashMap<&str, usize> = eligible
        .iter()
        .enumerate()
        .map(|(index, &value)| (value, index))
        .collect();

    let mut union_find = UnionFind::new(eligible.len());
    for (index, &value) in eligible.iter().enumerate() {
        for neighbor in tree.within(value, max_distance) {
            if let Some(&other) = index_of.get(neighbor) {
                union_find.union(index, other);
            }
        }
    }

    // Representative = lexicographically smallest member of each cluster.
    let mut representative: HashMap<usize, &str> = HashMap::new();
    for (index, &value) in eligible.iter().enumerate() {
        let root = union_find.find(index);
        representative
            .entry(root)
            .and_modify(|current| {
                if value < *current {
                    *current = value;
                }
            })
            .or_insert(value);
    }

    for (index, &value) in eligible.iter().enumerate() {
        let root = union_find.find(index);
        canonical.insert(value.to_string(), representative[&root].to_string());
    }

    canonical
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- BK-tree (3.1) ---

    fn sample_tree() -> BkTree {
        let mut tree = BkTree::default();
        for term in ["book", "books", "boo", "boon", "cake", "cape", "cart"] {
            tree.insert(term.to_string());
        }
        tree
    }

    #[test]
    fn bktree_within_radius_one() {
        let tree = sample_tree();
        let mut found = tree.within("book", 1);
        found.sort_unstable();
        assert_eq!(found, ["boo", "book", "books", "boon"]);
    }

    #[test]
    fn bktree_within_radius_zero_is_exact() {
        let tree = sample_tree();
        assert_eq!(tree.within("book", 0), ["book"]);
        assert!(tree.within("zzz", 0).is_empty());
    }

    #[test]
    fn bktree_radius_two_widens_the_set() {
        let tree = sample_tree();
        let mut found = tree.within("cake", 1);
        found.sort_unstable();
        assert_eq!(found, ["cake", "cape"]); // cart is distance 2
        assert!(tree.within("cake", 2).contains(&"cart"));
    }

    // --- union-find (3.2) ---

    #[test]
    fn unionfind_forms_correct_classes() {
        let mut uf = UnionFind::new(5);
        uf.union(0, 1);
        uf.union(2, 3);
        uf.union(1, 3);
        assert_eq!(uf.find(0), uf.find(3));
        assert_eq!(uf.find(1), uf.find(2));
        assert_ne!(uf.find(0), uf.find(4));
    }

    // --- clustering (3.3) ---

    #[test]
    fn neighbors_cluster_with_smallest_representative() {
        let values = vec![
            "Save".to_string(),
            "Save.".to_string(),
            "Delete".to_string(),
        ];
        let map = fuzzy_canonicals(&values, 2, 1);
        assert_eq!(map["Save"], "Save");
        assert_eq!(map["Save."], "Save"); // merged; rep is the smaller member
        assert_eq!(map["Delete"], "Delete"); // alone

        // Representative is deterministic across runs.
        assert_eq!(map, fuzzy_canonicals(&values, 2, 1));
    }

    // --- min-length guard (3.4) ---

    #[test]
    fn short_strings_skip_fuzzy() {
        // On/Off are distance 1 but below the default min_length (5).
        let values = vec!["On".to_string(), "Off".to_string()];
        let map = fuzzy_canonicals(&values, 2, 5);
        assert_eq!(map["On"], "On");
        assert_eq!(map["Off"], "Off");
    }
}
