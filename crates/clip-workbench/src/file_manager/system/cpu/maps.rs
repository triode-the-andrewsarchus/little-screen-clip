//! Aggregate complete, sparse CPU sets without expanding a large logical-ID space.
use super::{Cpu, Member};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Bin {
    pub first: u32,
    pub last: u32,
    pub total: u32,
    pub counts: Vec<Option<u32>>,
    pub selected: bool,
}
fn count(ranges: &[(u32, u32)]) -> u32 {
    ranges.iter().map(|(first, last)| last - first + 1).sum()
}
fn window(ranges: &[(u32, u32)], mut start: u32, mut length: u32) -> Vec<(u32, u32)> {
    let mut selected = Vec::new();
    for &(first, last) in ranges {
        let size = last - first + 1;
        if start >= size {
            start -= size;
            continue;
        }
        let begin = first + start;
        let size = length.min(last - begin + 1);
        selected.push((begin, begin + size - 1));
        length -= size;
        start = 0;
        if length == 0 {
            break;
        }
    }
    selected
}
fn intersection(left: &[(u32, u32)], right: &[(u32, u32)]) -> u32 {
    let (mut a, mut b, mut total) = (0, 0, 0);
    while a < left.len() && b < right.len() {
        let first = left[a].0.max(right[b].0);
        let last = left[a].1.min(right[b].1);
        if first <= last {
            total += last - first + 1;
        }
        if left[a].1 < right[b].1 {
            a += 1;
        } else {
            b += 1;
        }
    }
    total
}
pub(super) fn bins(domain: &[(u32, u32)], members: &[Member], selected: Option<u32>) -> Vec<Bin> {
    let total = count(domain);
    let length = total.min(128);
    let mut bins = Vec::new();
    for index in 0..length {
        let start = (u64::from(index) * u64::from(total) / u64::from(length)) as u32;
        let end = (u64::from(index + 1) * u64::from(total) / u64::from(length)) as u32;
        let ranges = window(domain, start, end - start);
        bins.push(Bin {
            first: ranges[0].0,
            last: ranges.last().unwrap().1,
            total: end - start,
            counts: members
                .iter()
                .map(|member| member.ranges.as_ref().map(|set| intersection(&ranges, set)))
                .collect(),
            selected: selected.is_some_and(|id| {
                ranges
                    .iter()
                    .any(|(first, last)| (*first..=*last).contains(&id))
            }),
        });
    }
    bins
}
impl Cpu {
    pub(super) fn count(&self, membership: usize) -> Option<u32> {
        self.bins
            .iter()
            .map(|bin| bin.counts.get(membership).copied().flatten())
            .try_fold(0u32, |total, value| value.map(|value| total + value))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_cpu_numbers_keep_membership_and_do_not_invent_holes() {
        let members = vec![Member {
            label: "Online".into(),
            ranges: Some(vec![(2, 2), (999, 999)]),
        }];
        let bins = bins(&[(2, 3), (999, 999)], &members, Some(3));
        assert_eq!(bins.len(), 3);
        assert_eq!(
            bins.iter().map(|bin| bin.first).collect::<Vec<_>>(),
            vec![2, 3, 999]
        );
        assert_eq!(
            bins.iter().map(|bin| bin.counts[0]).collect::<Vec<_>>(),
            vec![Some(1), Some(0), Some(1)]
        );
        assert!(bins[1].selected);
    }
    #[test]
    fn million_cpu_domain_is_fully_represented_in_bounded_bins() {
        let members = vec![Member {
            label: "Online".into(),
            ranges: Some(vec![(0, 499999)]),
        }];
        let bins = bins(&[(0, 999999)], &members, Some(999999));
        assert_eq!(bins.len(), 128);
        assert_eq!(bins.iter().map(|bin| bin.total).sum::<u32>(), 1000000);
        assert_eq!(
            bins.iter().map(|bin| bin.counts[0].unwrap()).sum::<u32>(),
            500000
        );
        assert_eq!(bins.last().unwrap().last, 999999);
        assert!(bins.last().unwrap().selected);
    }
    #[test]
    fn unavailable_membership_differs_from_an_observed_empty_set() {
        let members = vec![
            Member {
                label: "Unavailable".into(),
                ranges: None,
            },
            Member {
                label: "Empty".into(),
                ranges: Some(Vec::new()),
            },
        ];
        let cells = bins(&[(0, 1)], &members, None);
        assert_eq!(cells[0].counts, vec![None, Some(0)]);
        assert!(bins(&[], &members, None).is_empty());
    }
}
