//! Golden behavior and topology checks for the converted tree export.
use super::*;

fn cases() -> Vec<[f32; 32]> {
    fn visit(
        tree: &[Node],
        index: usize,
        lower: [f32; 32],
        upper: [f32; 32],
        output: &mut Vec<[f32; 32]>,
    ) {
        if lower.iter().zip(upper).any(|(a, b)| *a > b) {
            return;
        }
        let base = std::array::from_fn(|i| 0.0_f32.clamp(lower[i], upper[i]));
        match tree[index] {
            Node::Leaf(_) => output.push(base),
            Node::Branch(feature, threshold, left, right) => {
                for value in [threshold.next_down(), threshold, threshold.next_up()] {
                    if value >= lower[feature] && value <= upper[feature] {
                        let mut sample = base;
                        sample[feature] = value;
                        output.push(sample);
                    }
                }
                let mut left_upper = upper;
                left_upper[feature] = left_upper[feature].min(threshold);
                visit(tree, left, lower, left_upper, output);
                let mut right_lower = lower;
                right_lower[feature] = right_lower[feature].max(threshold.next_up());
                visit(tree, right, right_lower, upper, output);
            }
        }
    }
    let mut output = Vec::new();
    for tree in TREES {
        visit(
            tree,
            0,
            [f32::NEG_INFINITY; 32],
            [f32::INFINITY; 32],
            &mut output,
        );
    }
    output
}
fn fingerprint() -> u64 {
    let mut digest = 0xcbf29ce484222325_u64;
    for sample in cases() {
        for score in predict_acoustic_probabilities(&sample) {
            for byte in score.to_bits().to_le_bytes() {
                digest = (digest ^ byte as u64).wrapping_mul(0x100000001b3);
            }
        }
    }
    digest
}

#[test]
fn converted_export_matches_original_boundary_and_leaf_predictions() {
    // Recorded against the original nested evaluator at 82e72a0. Each reachable
    // leaf and each split's adjacent f32 values participates in the fingerprint.
    assert_eq!(cases().len(), 5452);
    assert_eq!(fingerprint(), 0x97ce8978c7a60b84);
}

#[test]
fn static_model_has_valid_forward_edges_and_normalized_leaves() {
    assert_eq!(TREES.len(), 20);
    for tree in TREES {
        for (index, node) in tree.iter().enumerate() {
            match node {
                Node::Branch(feature, threshold, left, right) => {
                    assert!(*feature < 32 && threshold.is_finite());
                    assert!(*left > index && *left < tree.len());
                    assert!(*right > index && *right < tree.len());
                }
                Node::Leaf(weights) => {
                    assert!(weights.iter().all(|(category, weight)| *category < 12
                        && weight.is_finite()
                        && *weight > 0.0));
                    assert!(
                        (weights.iter().map(|(_, weight)| weight).sum::<f32>() - 1.0).abs()
                            < 0.00005
                    );
                }
            }
        }
    }
}
