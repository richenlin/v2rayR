//! Control-plane optimization. Never sort: first matching rule wins.
use crate::{CustomRule, RuleKind};
use std::collections::HashSet;

pub fn deduplicate(rules: impl IntoIterator<Item = CustomRule>) -> Vec<CustomRule> {
    let mut seen = HashSet::new();
    rules
        .into_iter()
        .filter(|rule| seen.insert((rule.kind, rule.value.clone())))
        .collect()
}

/// Eliminate default-direct suffix rules only when they cannot protect any
/// later non-direct destination. Intended for the PAC tail, never user rules:
/// no lower-priority rules may be appended after this optimization.
///
/// Both ancestor and descendant overlaps matter. Index suffixes instead of
/// comparing every direct rule with every proxy rule. Input is normalized.
pub fn simplify_pac(rules: Vec<CustomRule>) -> Vec<CustomRule> {
    if rules.iter().any(|rule| rule.kind != RuleKind::DomainSuffix) {
        return rules;
    }
    let mut keep = vec![true; rules.len()];
    {
        let mut non_direct = HashSet::new();
        let mut non_direct_ancestors = HashSet::new();
        for (index, rule) in rules.iter().enumerate().rev() {
            let domain = rule.value.as_str();
            if rule.action == crate::RuleAction::Direct {
                keep[index] = non_direct_ancestors.contains(domain)
                    || suffixes(domain).any(|suffix| non_direct.contains(suffix));
            } else {
                non_direct.insert(domain);
                non_direct_ancestors.extend(suffixes(domain));
            }
        }
    }
    rules
        .into_iter()
        .zip(keep)
        .filter_map(|(rule, keep)| keep.then_some(rule))
        .collect()
}

fn suffixes(domain: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(domain), |suffix| {
        suffix.split_once('.').map(|(_, rest)| rest)
    })
}

/// Merge only adjacent equal-kind/equal-action domain predicates. Native
/// engines index the resulting domain arrays; unrelated priorities stay put.
pub(crate) fn batches(rules: &[CustomRule]) -> Vec<&[CustomRule]> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < rules.len() {
        let first = &rules[start];
        let mut end = start + 1;
        if matches!(first.kind, RuleKind::Domain | RuleKind::DomainSuffix) {
            while end < rules.len()
                && rules[end].kind == first.kind
                && rules[end].action == first.action
            {
                end += 1;
            }
        }
        result.push(&rules[start..end]);
        start = end;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuleAction::*;
    fn rule(value: &str, action: crate::RuleAction) -> CustomRule {
        CustomRule {
            kind: RuleKind::DomainSuffix,
            value: value.into(),
            action,
        }
    }
    fn decide(rules: &[CustomRule], host: &str) -> crate::RuleAction {
        rules
            .iter()
            .find(|r| host == r.value || host.ends_with(&format!(".{}", r.value)))
            .map(|r| r.action)
            .unwrap_or(Direct)
    }
    #[test]
    fn preserves_conflicts_boundaries_and_first_match() {
        let original = vec![
            rule("example.com", Direct),
            rule("child.example.com", Proxy),
            rule("example.com", Block),
            rule("other.com", Proxy),
        ];
        let optimized = deduplicate(original.clone());
        assert_eq!(optimized.len(), 3);
        for host in [
            "example.com",
            "child.example.com",
            "evil-example.com",
            "a.other.com",
            "unmatched.net",
        ] {
            assert_eq!(decide(&original, host), decide(&optimized, host));
        }
        assert_eq!(
            batches(&optimized)
                .iter()
                .map(|b| b.len())
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }
    #[test]
    fn batches_do_not_cross_action_or_type_boundaries() {
        let mut rules = vec![
            rule("a.com", Proxy),
            rule("b.com", Proxy),
            rule("c.com", Direct),
        ];
        rules.push(CustomRule {
            kind: RuleKind::Domain,
            value: "d.com".into(),
            action: Direct,
        });
        assert_eq!(
            batches(&rules).iter().map(|b| b.len()).collect::<Vec<_>>(),
            [2, 1, 1]
        );
    }
    #[test]
    fn pac_removes_unrelated_and_trailing_direct_rules_but_keeps_exceptions() {
        let original = vec![
            rule("unrelated.cn", Direct),
            rule("safe.example.com", Direct),
            rule("example.com", Proxy),
            rule("tail.net", Direct),
        ];
        let optimized = simplify_pac(original.clone());
        assert_eq!(
            optimized,
            vec![rule("safe.example.com", Direct), rule("example.com", Proxy)]
        );
        for host in [
            "unrelated.cn",
            "safe.example.com",
            "a.safe.example.com",
            "example.com",
            "x.example.com",
            "evil-example.com",
            "tail.net",
        ] {
            assert_eq!(decide(&original, host), decide(&optimized, host));
        }
        assert!(simplify_pac(vec![rule("direct.cn", Direct)]).is_empty());
    }

    #[test]
    fn pac_preserves_parent_and_equal_exceptions_and_first_match() {
        for original in [
            vec![
                rule("example.com", Direct),
                rule("child.example.com", Proxy),
            ],
            vec![rule("example.com", Direct), rule("example.com", Proxy)],
            vec![
                rule("example.com", Proxy),
                rule("child.example.com", Direct),
            ],
            vec![rule("safe.example.com", Direct), rule("example.com", Block)],
        ] {
            let optimized = simplify_pac(original.clone());
            for host in [
                "example.com",
                "child.example.com",
                "x.child.example.com",
                "safe.example.com",
                "other.net",
            ] {
                assert_eq!(decide(&original, host), decide(&optimized, host));
            }
        }
    }

    #[test]
    fn pac_simplification_is_equivalent_for_all_small_ordered_lists() {
        let targets = ["com", "example.com", "safe.example.com", "other.net"];
        let hosts = [
            "com",
            "example.com",
            "a.example.com",
            "safe.example.com",
            "x.safe.example.com",
            "other.net",
            "x.other.net",
            "evil-example.com",
            "unmatched.org",
        ];
        // Exhaust all 12^4 combinations of target and action; duplicates and
        // overlapping groups exercise first-match order as well as boundaries.
        for mut encoded in 0..20736 {
            let mut original = Vec::new();
            for _ in 0..4 {
                let choice = encoded % 12;
                encoded /= 12;
                original.push(rule(
                    targets[choice / 3],
                    [Direct, Proxy, Block][choice % 3],
                ));
            }
            let optimized = simplify_pac(deduplicate(original.clone()));
            for host in hosts {
                assert_eq!(
                    decide(&original, host),
                    decide(&optimized, host),
                    "{host}: {original:?} -> {optimized:?}"
                );
            }
        }
    }
}
