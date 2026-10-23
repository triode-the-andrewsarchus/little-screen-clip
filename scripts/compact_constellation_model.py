#!/usr/bin/env python3
"""Convert the original nested Rust model exporter to static decision-tree data.

Usage: compact_constellation_model.py original_generated_model.rs output.rs
Threshold literals, leaf weights, category order, and tree order are preserved.
This converts an existing export; it does not train or recalibrate the model.
"""
import re
import sys
from pathlib import Path


def parse_tree(body):
    tokens = re.findall(r'if|else|f\[\d+\]|out\[\d+\]|<=|\+=|-?[0-9]+\.[0-9]+_f32|[{};]', body)
    position = 0
    nodes = []

    def take(expected=None):
        nonlocal position
        value = tokens[position]
        position += 1
        if expected is not None and value != expected:
            raise ValueError(f'Expected {expected!r}, got {value!r}')
        return value

    def block():
        take('{')
        index = len(nodes)
        nodes.append(None)
        if tokens[position] == 'if':
            take('if')
            feature = int(take()[2:-1])
            take('<=')
            threshold = take()
            left = block()
            take('else')
            right = block()
            nodes[index] = f'Node::Branch({feature}, {threshold}, {left}, {right})'
        else:
            weights = []
            while tokens[position] != '}':
                category = int(take()[4:-1])
                take('+=')
                weights.append(f'({category}, {take()})')
                take(';')
            nodes[index] = f'Node::Leaf(&[{", ".join(weights)}])'
        take('}')
        return index

    block()
    if position != len(tokens):
        raise ValueError('Unconsumed tree tokens')
    return nodes


def convert(source):
    start = source.index('fn eval_tree_0')
    prefix = source[:start]
    prefix = prefix.replace(
        '//! Evaluates 20 decision trees of max depth 7 over 32 acoustic DSP features.',
        '//! Evaluates 20 decision trees of maximum depth seven over 32 acoustic features.\n'
        '//!\n'
        "//! The static nodes preserve the initial export's thresholds and weights. See\n"
        '//! `scripts/compact_constellation_model.py` for the lossless conversion from its\n'
        '//! original nested Rust form. The training dataset/exporter is not in this repo;\n'
        "//! this representation documents inference, not the model's training provenance.\n"
        '//! Category order must stay aligned with both the leaves and feature extractor.',
    )
    trees = re.findall(r'fn eval_tree_\d+\(f: &\[f32; 32\], out: &mut \[f32; 12\]\) (.*?)(?=\nfn eval_tree_|\n/// Predicts)', source[start:], re.S)
    if len(trees) != 20:
        raise ValueError(f'Expected 20 trees, found {len(trees)}')
    output = [prefix, '''// Static model nodes retain the original export's exact f32 literals.
// Indices are local to each tree, with its root at zero.
enum Node {
    Branch(usize, f32, usize, usize),
    Leaf(&'static [(usize, f32)]),
}

''']
    for index, body in enumerate(trees):
        nodes = parse_tree(body)
        output.append(f'#[rustfmt::skip]\nstatic TREE_{index}: &[Node] = &[\n')
        output.extend(f'    {node},\n' for node in nodes)
        output.append('];\n\n')
    output.append('static TREES: &[&[Node]] = &[\n')
    output.extend(f'    TREE_{index},\n' for index in range(len(trees)))
    output.append('];\n\n')
    output.append('''/// Returns the ensemble's scores in [`SampleCategory::ALL`] order.
///
/// Each tree follows `feature <= threshold` branches to one sparse leaf. Trees
/// and leaf additions retain the original export order, including f32 rounding.
/// The resulting scores are averaged; exported rounding means their sum can
/// differ slightly from one. Callers must supply finite acoustic features.
/// These scores are classification heuristics, not calibrated probabilities.
pub fn predict_acoustic_probabilities(features: &[f32; 32]) -> [f32; 12] {
    let mut accum = [0.0f32; 12];
    for tree in TREES {
        let mut index = 0;
        loop {
            match tree[index] {
                Node::Branch(feature, threshold, left, right) => {
                    index = if features[feature] <= threshold { left } else { right };
                }
                Node::Leaf(weights) => {
                    for &(category, weight) in weights { accum[category] += weight; }
                    break;
                }
            }
        }
    }
    for score in &mut accum { *score *= 1.0 / TREES.len() as f32; }
    accum
}
''')
    output.append('\n#[cfg(test)]\n#[path = "model_tests.rs"]\nmod tests;\n')
    return ''.join(output)


if __name__ == '__main__':
    source, target = map(Path, sys.argv[1:])
    target.write_text(convert(source.read_text()))
