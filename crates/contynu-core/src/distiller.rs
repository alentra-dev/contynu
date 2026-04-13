//! Dream Phase: Autonomous Memory Consolidation
//!
//! Finds clusters of related memories within the same kind that have high
//! textual overlap (Jaccard similarity on word sets) and proposes them as
//! consolidation candidates. The actual synthesis is performed by the AI
//! via the `consolidate_memories` MCP tool.

use std::collections::{HashMap, HashSet};

use crate::error::Result;
use crate::ids::MemoryId;
use crate::store::{MemoryObject, MemoryObjectKind, MetadataStore};

/// A cluster of related memories that are candidates for consolidation.
#[derive(Debug, Clone)]
pub struct ConsolidationCandidate {
    pub kind: MemoryObjectKind,
    pub memory_ids: Vec<MemoryId>,
    pub texts: Vec<String>,
    pub avg_similarity: f64,
}

/// Minimum Jaccard similarity to consider two memories related.
const SIMILARITY_THRESHOLD: f64 = 0.35;

/// Minimum cluster size to suggest consolidation.
const MIN_CLUSTER_SIZE: usize = 2;

/// Maximum number of candidates to return per call.
const MAX_CANDIDATES: usize = 5;

/// Compute Jaccard similarity between two sets of lowercase words.
fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let words_a: HashSet<&str> = a.split_whitespace().collect();
    let words_b: HashSet<&str> = b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Find clusters of related memories within the same kind using Jaccard similarity.
/// Uses a greedy single-linkage approach: for each ungrouped memory, pull in
/// all other ungrouped memories of the same kind that are similar enough.
fn find_clusters(memories: &[MemoryObject]) -> Vec<ConsolidationCandidate> {
    // Group by kind first
    let mut by_kind: HashMap<MemoryObjectKind, Vec<&MemoryObject>> = HashMap::new();
    for m in memories {
        by_kind.entry(m.kind).or_default().push(m);
    }

    let mut candidates = Vec::new();

    for (kind, mems) in &by_kind {
        if mems.len() < MIN_CLUSTER_SIZE {
            continue;
        }

        // Normalize texts for comparison
        let normalized: Vec<String> = mems.iter().map(|m| m.text.to_lowercase()).collect();

        let mut used = vec![false; mems.len()];

        for i in 0..mems.len() {
            if used[i] {
                continue;
            }

            let mut cluster_indices = vec![i];

            for j in (i + 1)..mems.len() {
                if used[j] {
                    continue;
                }

                // Check similarity against any member of the current cluster
                let similar_to_cluster = cluster_indices.iter().any(|&ci| {
                    jaccard_similarity(&normalized[ci], &normalized[j]) >= SIMILARITY_THRESHOLD
                });

                if similar_to_cluster {
                    cluster_indices.push(j);
                }
            }

            if cluster_indices.len() >= MIN_CLUSTER_SIZE {
                // Compute average pairwise similarity
                let mut total_sim = 0.0;
                let mut pair_count = 0;
                for a in 0..cluster_indices.len() {
                    for b in (a + 1)..cluster_indices.len() {
                        total_sim += jaccard_similarity(
                            &normalized[cluster_indices[a]],
                            &normalized[cluster_indices[b]],
                        );
                        pair_count += 1;
                    }
                }
                let avg_similarity = if pair_count > 0 {
                    total_sim / pair_count as f64
                } else {
                    0.0
                };

                for &ci in &cluster_indices {
                    used[ci] = true;
                }

                candidates.push(ConsolidationCandidate {
                    kind: *kind,
                    memory_ids: cluster_indices
                        .iter()
                        .map(|&ci| mems[ci].memory_id.clone())
                        .collect(),
                    texts: cluster_indices
                        .iter()
                        .map(|&ci| mems[ci].text.clone())
                        .collect(),
                    avg_similarity,
                });
            }
        }
    }

    // Sort by average similarity descending (best clusters first)
    candidates.sort_by(|a, b| {
        b.avg_similarity
            .partial_cmp(&a.avg_similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(MAX_CANDIDATES);
    candidates
}

/// Scan the project's active memories and return consolidation candidates.
pub fn suggest_consolidation(
    store: &MetadataStore,
    session_id: &crate::ids::SessionId,
) -> Result<Vec<ConsolidationCandidate>> {
    let memories = store.list_active_memories(session_id, None)?;
    Ok(find_clusters(&memories))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{MemoryId, SessionId};
    use crate::store::{MemoryObject, MemoryObjectKind, MemoryScope};

    fn make_memory(kind: MemoryObjectKind, text: &str) -> MemoryObject {
        MemoryObject {
            memory_id: MemoryId::new(),
            session_id: SessionId::parse("prj_019d503680a475a3ae465200a90cd4fa").unwrap(),
            kind,
            scope: MemoryScope::Project,
            status: "active".into(),
            text: text.into(),
            importance: 0.5,
            reason: None,
            source_model: None,
            superseded_by: None,
            created_at: chrono::Utc::now(),
            updated_at: None,
            access_count: 0,
            last_accessed_at: None,
        }
    }

    #[test]
    fn jaccard_identical_strings() {
        assert!((jaccard_similarity("hello world", "hello world") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_disjoint_strings() {
        assert!((jaccard_similarity("hello world", "foo bar")).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_partial_overlap() {
        let sim = jaccard_similarity(
            "the auth system uses JWT tokens",
            "auth system uses HMAC-SHA256 for JWT",
        );
        assert!(sim > 0.3, "Expected significant overlap, got {sim}");
    }

    #[test]
    fn finds_clusters_of_similar_memories() {
        let memories = vec![
            make_memory(
                MemoryObjectKind::Fact,
                "The auth system uses JWT tokens for authentication",
            ),
            make_memory(
                MemoryObjectKind::Fact,
                "Auth system uses JWT tokens with HMAC-SHA256 signing",
            ),
            make_memory(MemoryObjectKind::Fact, "The database is PostgreSQL 15"),
            make_memory(MemoryObjectKind::Decision, "Use Redis for caching"),
        ];

        let clusters = find_clusters(&memories);
        assert_eq!(clusters.len(), 1, "Should find exactly one cluster");
        assert_eq!(clusters[0].kind, MemoryObjectKind::Fact);
        assert_eq!(clusters[0].memory_ids.len(), 2);
    }

    #[test]
    fn no_clusters_when_all_distinct() {
        let memories = vec![
            make_memory(MemoryObjectKind::Fact, "The sky is blue"),
            make_memory(MemoryObjectKind::Fact, "PostgreSQL runs on port 5432"),
            make_memory(MemoryObjectKind::Fact, "Rust compiles to native code"),
        ];

        let clusters = find_clusters(&memories);
        assert!(clusters.is_empty(), "Should find no clusters");
    }

    #[test]
    fn clusters_respect_kind_boundaries() {
        let memories = vec![
            make_memory(MemoryObjectKind::Fact, "Auth uses JWT tokens for signing"),
            make_memory(
                MemoryObjectKind::Decision,
                "Auth uses JWT tokens for signing",
            ),
        ];

        let clusters = find_clusters(&memories);
        assert!(
            clusters.is_empty(),
            "Same text but different kinds should not cluster"
        );
    }

    #[test]
    fn consolidate_transaction_works() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::StatePaths::new(dir.path());
        state.ensure_layout().unwrap();
        let store = MetadataStore::open(state.sqlite_db()).unwrap();
        let session_id = SessionId::new();

        store
            .register_session(&crate::store::SessionRecord {
                session_id: session_id.clone(),
                project_id: None,
                status: "active".into(),
                cli_name: None,
                cli_version: None,
                model_name: None,
                cwd: None,
                repo_root: None,
                host_fingerprint: None,
                started_at: chrono::Utc::now(),
                ended_at: None,
            })
            .unwrap();

        // Insert two similar memories
        let mem1 = make_memory(MemoryObjectKind::Fact, "Auth uses JWT");
        let mem2 = make_memory(MemoryObjectKind::Fact, "Auth uses JWT with HMAC");
        let mut mem1 = mem1;
        let mut mem2 = mem2;
        mem1.session_id = session_id.clone();
        mem2.session_id = session_id.clone();
        store.insert_memory_object(&mem1).unwrap();
        store.insert_memory_object(&mem2).unwrap();

        // Consolidate them
        let golden = MemoryObject {
            memory_id: MemoryId::new(),
            session_id: session_id.clone(),
            kind: MemoryObjectKind::Fact,
            scope: MemoryScope::Project,
            status: "active".into(),
            text: "Auth uses JWT with HMAC-SHA256 signing".into(),
            importance: 0.85,
            reason: Some("Consolidated from 2 memories".into()),
            source_model: None,
            superseded_by: None,
            created_at: chrono::Utc::now(),
            updated_at: None,
            access_count: 0,
            last_accessed_at: None,
        };

        let superseded = store
            .consolidate_memories(&[mem1.memory_id.clone(), mem2.memory_id.clone()], &golden)
            .unwrap();

        assert_eq!(superseded, 2);

        // Verify originals are superseded
        let m1 = store.get_memory(&mem1.memory_id).unwrap().unwrap();
        assert_eq!(m1.status, "superseded");
        assert_eq!(m1.superseded_by.as_ref().unwrap(), &golden.memory_id);

        // Verify golden is active
        let g = store.get_memory(&golden.memory_id).unwrap().unwrap();
        assert_eq!(g.status, "active");

        // Verify only golden shows in active list
        let active = store.list_active_memories(&session_id, None).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].memory_id, golden.memory_id);
    }
}
