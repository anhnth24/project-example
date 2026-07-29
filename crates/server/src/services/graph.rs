//! Pure graph algorithms for the Document Graph MVP (P2-17): degree-based
//! bounding/pruning and connected components ("communities"). No DB, no I/O —
//! fully unit-testable and deterministic regardless of input ordering.
//!
//! Deliberately hand-rolled instead of a graph crate: components/pruning are
//! small, well-understood algorithms (BFS + two sorts) and the project's own
//! dependency policy (`scripts/check-dependency-policy.py`) favors not
//! reaching for a dependency when ~100 lines of plain Rust cover the need —
//! same reasoning the web side applies to the force-directed layout.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use uuid::Uuid;

/// One candidate edge before pruning. `kind` is carried through so the same
/// document pair can carry more than one edge (e.g. both a `conflict` and a
/// `co_citation` edge) — pruning/components treat those as distinct edges.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdgeInput {
    pub source: Uuid,
    pub target: Uuid,
    pub kind: String,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PrunedGraph {
    /// Kept node ids, ascending — deterministic regardless of input order.
    pub node_ids: Vec<Uuid>,
    /// Kept edges, ranked by weight desc (ties by source/target/kind asc)
    /// before truncation, but returned in that same ranked order.
    pub edges: Vec<GraphEdgeInput>,
}

/// Degree-based bound matching the contract's "cap nodes/edges (vd
/// 500/2000), degree-based pruning khi vượt":
///
/// 1. Rank nodes by degree (edge endpoint count) descending, ties broken by
///    ascending id, and keep at most `max_nodes`. This favors well-connected
///    ("central") documents over arbitrarily-ordered ones when a tenant has
///    more visible documents than the cap allows.
/// 2. Drop any edge with an endpoint that didn't survive step 1.
/// 3. Rank the remaining edges by weight descending, ties broken by
///    `(source, target, kind)` ascending, and keep at most `max_edges`.
pub fn prune(
    node_ids: &[Uuid],
    edges: &[GraphEdgeInput],
    max_nodes: usize,
    max_edges: usize,
) -> PrunedGraph {
    let mut degree: BTreeMap<Uuid, usize> = node_ids.iter().map(|id| (*id, 0)).collect();
    for edge in edges {
        if let Some(d) = degree.get_mut(&edge.source) {
            *d += 1;
        }
        if let Some(d) = degree.get_mut(&edge.target) {
            *d += 1;
        }
    }

    let mut ranked_nodes: Vec<Uuid> = node_ids.to_vec();
    ranked_nodes.sort_by(|a, b| {
        let da = degree.get(a).copied().unwrap_or(0);
        let db = degree.get(b).copied().unwrap_or(0);
        db.cmp(&da).then_with(|| a.cmp(b))
    });
    let kept: BTreeSet<Uuid> = ranked_nodes.into_iter().take(max_nodes).collect();

    let mut kept_edges: Vec<GraphEdgeInput> = edges
        .iter()
        .filter(|edge| kept.contains(&edge.source) && kept.contains(&edge.target))
        .cloned()
        .collect();
    kept_edges.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (a.source, a.target, &a.kind).cmp(&(b.source, b.target, &b.kind)))
    });
    kept_edges.truncate(max_edges);

    let mut node_ids_out: Vec<Uuid> = kept.into_iter().collect();
    node_ids_out.sort();
    PrunedGraph {
        node_ids: node_ids_out,
        edges: kept_edges,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    /// Members, ascending — deterministic regardless of input order.
    pub node_ids: Vec<Uuid>,
}

/// Connected components over an undirected graph built from `edges`'
/// endpoints (edge `kind`/`weight` do not affect connectivity — a
/// `conflict` edge and a `co_citation` edge between the same pair merge
/// them into the same component either way). Isolated nodes form their own
/// singleton component.
///
/// Deterministic: components are ordered by their smallest member id, and
/// each component's members are sorted ascending — independent of the order
/// `node_ids`/`edges` are given in.
pub fn connected_components(node_ids: &[Uuid], edges: &[GraphEdgeInput]) -> Vec<Component> {
    let mut adjacency: BTreeMap<Uuid, BTreeSet<Uuid>> =
        node_ids.iter().map(|id| (*id, BTreeSet::new())).collect();
    for edge in edges {
        adjacency
            .entry(edge.source)
            .or_default()
            .insert(edge.target);
        adjacency
            .entry(edge.target)
            .or_default()
            .insert(edge.source);
    }

    let mut sorted_nodes = node_ids.to_vec();
    sorted_nodes.sort();
    sorted_nodes.dedup();

    let mut visited: BTreeSet<Uuid> = BTreeSet::new();
    let mut components = Vec::new();
    for &start in &sorted_nodes {
        if visited.contains(&start) {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        visited.insert(start);
        let mut members = Vec::new();
        while let Some(current) = queue.pop_front() {
            members.push(current);
            if let Some(neighbors) = adjacency.get(&current) {
                for &next in neighbors {
                    if visited.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }
        members.sort();
        components.push(Component { node_ids: members });
    }
    components.sort_by_key(|c| c.node_ids.first().copied());
    components
}

/// Maps an evidence count to a `0..1` edge weight: saturating, not linear,
/// so a single conflict/co-citation still registers a real (if modest)
/// weight and repeated evidence pushes it toward — but never reaches — 1.0.
/// Deliberately not a claim of calibrated probability, just a bounded,
/// monotone strength signal for layout/rendering.
pub fn saturating_weight(count: i64) -> f64 {
    1.0 - 1.0 / (1.0 + count.max(1) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn edge(a: u128, b: u128, kind: &str, weight: f64) -> GraphEdgeInput {
        GraphEdgeInput {
            source: uid(a),
            target: uid(b),
            kind: kind.to_string(),
            weight,
        }
    }

    #[test]
    fn components_groups_connected_nodes_and_isolates_singletons() {
        let nodes = vec![uid(1), uid(2), uid(3), uid(4)];
        let edges = vec![edge(1, 2, "conflict", 0.5)];
        let components = connected_components(&nodes, &edges);
        assert_eq!(
            components,
            vec![
                Component {
                    node_ids: vec![uid(1), uid(2)]
                },
                Component {
                    node_ids: vec![uid(3)]
                },
                Component {
                    node_ids: vec![uid(4)]
                },
            ]
        );
    }

    #[test]
    fn components_are_order_independent() {
        let nodes_a = vec![uid(1), uid(2), uid(3)];
        let nodes_b = vec![uid(3), uid(1), uid(2)];
        let edges_a = vec![edge(2, 3, "co_citation", 0.2), edge(1, 2, "conflict", 0.8)];
        let edges_b = vec![edge(1, 2, "conflict", 0.8), edge(2, 3, "co_citation", 0.2)];
        assert_eq!(
            connected_components(&nodes_a, &edges_a),
            connected_components(&nodes_b, &edges_b)
        );
    }

    #[test]
    fn components_merge_multi_kind_edges_between_same_pair() {
        let nodes = vec![uid(1), uid(2)];
        let edges = vec![edge(1, 2, "conflict", 0.9), edge(1, 2, "co_citation", 0.3)];
        let components = connected_components(&nodes, &edges);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].node_ids, vec![uid(1), uid(2)]);
    }

    #[test]
    fn prune_keeps_highest_degree_nodes_when_over_cap() {
        // Hub (1) connects to 2,3,4; 5 and 6 are an isolated pair.
        let nodes = vec![uid(1), uid(2), uid(3), uid(4), uid(5), uid(6)];
        let edges = vec![
            edge(1, 2, "conflict", 0.5),
            edge(1, 3, "conflict", 0.5),
            edge(1, 4, "conflict", 0.5),
            edge(5, 6, "conflict", 0.5),
        ];
        let pruned = prune(&nodes, &edges, 4, 100);
        // Degrees: 1->3, 2->1, 3->1, 4->1, 5->1, 6->1. Node 1 always kept;
        // the tie among {2,3,4,5,6} (all degree 1) is broken by ascending id,
        // so 2,3,4 (the smallest three ids after 1) survive over 5,6.
        assert_eq!(pruned.node_ids, vec![uid(1), uid(2), uid(3), uid(4)]);
    }

    #[test]
    fn prune_drops_edges_with_a_dropped_endpoint() {
        let nodes = vec![uid(1), uid(2), uid(3)];
        let edges = vec![edge(1, 2, "conflict", 0.9), edge(2, 3, "conflict", 0.9)];
        let pruned = prune(&nodes, &edges, 2, 100);
        assert_eq!(pruned.node_ids, vec![uid(1), uid(2)]);
        assert_eq!(pruned.edges, vec![edge(1, 2, "conflict", 0.9)]);
    }

    #[test]
    fn prune_keeps_highest_weight_edges_when_over_cap() {
        let nodes = vec![uid(1), uid(2), uid(3), uid(4)];
        let edges = vec![
            edge(1, 2, "conflict", 0.1),
            edge(1, 3, "conflict", 0.9),
            edge(1, 4, "conflict", 0.5),
        ];
        let pruned = prune(&nodes, &edges, 10, 2);
        assert_eq!(
            pruned.edges,
            vec![edge(1, 3, "conflict", 0.9), edge(1, 4, "conflict", 0.5)]
        );
    }

    #[test]
    fn prune_is_deterministic_regardless_of_input_order() {
        let nodes_a = vec![uid(1), uid(2), uid(3)];
        let nodes_b = vec![uid(3), uid(2), uid(1)];
        let edges_a = vec![edge(1, 2, "conflict", 0.4), edge(2, 3, "conflict", 0.6)];
        let edges_b = vec![edge(2, 3, "conflict", 0.6), edge(1, 2, "conflict", 0.4)];
        assert_eq!(
            prune(&nodes_a, &edges_a, 10, 10),
            prune(&nodes_b, &edges_b, 10, 10)
        );
    }

    #[test]
    fn saturating_weight_is_bounded_monotone_and_never_reaches_one() {
        assert!(saturating_weight(1) > 0.0);
        assert!(saturating_weight(1) < saturating_weight(2));
        assert!(saturating_weight(2) < saturating_weight(100));
        assert!(saturating_weight(1_000_000) < 1.0);
        assert_eq!(saturating_weight(0), saturating_weight(1)); // count clamped to >= 1
    }
}
