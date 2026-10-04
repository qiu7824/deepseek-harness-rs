//! Plan private navigation and preserve migration authority across equivalent ACL updates.
use std::collections::BTreeSet;

pub(crate) enum MigrationAce {
    ManagedDeny(Vec<u8>),
    ManagedNavigationAllow(Vec<u8>),
    Ordered(Vec<u8>),
}

/// Managed denials and verified navigation allows commute within their own
/// blocks. Their placement relative to other ACEs remains authoritative.
pub(crate) fn migration_ace_records(aces: impl IntoIterator<Item = MigrationAce>) -> Vec<Vec<u8>> {
    let mut records = Vec::new();
    let mut denials = Vec::new();
    let mut navigation = Vec::new();
    let flush = |records: &mut Vec<Vec<u8>>, pending: &mut Vec<Vec<u8>>| {
        pending.sort();
        records.append(pending);
    };
    for ace in aces {
        match ace {
            MigrationAce::ManagedDeny(ace) => {
                flush(&mut records, &mut navigation);
                denials.push(ace);
            }
            MigrationAce::ManagedNavigationAllow(ace) => {
                flush(&mut records, &mut denials);
                navigation.push(ace);
            }
            MigrationAce::Ordered(ace) => {
                flush(&mut records, &mut denials);
                flush(&mut records, &mut navigation);
                records.push(ace);
            }
        }
    }
    flush(&mut records, &mut denials);
    flush(&mut records, &mut navigation);
    records
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrivateAccess {
    Denied,
    Navigate,
    Read,
    Write,
}

pub(crate) fn within_key(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('\\'))
}

pub(crate) fn navigation_ancestor_keys(
    grants: &[String],
    private_roots: &[String],
) -> BTreeSet<String> {
    let mut ancestors = BTreeSet::new();
    for grant in grants {
        let Some(boundary) = private_roots
            .iter()
            .filter(|root| within_key(grant, root))
            .max_by_key(|root| root.len())
        else {
            continue;
        };
        if grant == boundary {
            continue;
        }
        let mut path = grant.as_str();
        while let Some((parent, _)) = path.rsplit_once('\\') {
            if !private_roots.iter().any(|root| within_key(parent, root)) {
                break;
            }
            ancestors.insert(parent.to_owned());
            path = parent;
        }
    }
    ancestors
}

pub(crate) fn private_access(
    path: &str,
    private_roots: &[String],
    reads: &[String],
    writes: &[String],
    navigation: &BTreeSet<String>,
) -> PrivateAccess {
    let Some(boundary) = private_roots
        .iter()
        .filter(|root| within_key(path, root))
        .max_by_key(|root| root.len())
    else {
        return PrivateAccess::Denied;
    };
    let own = |grant: &String| {
        grant != boundary && within_key(grant, boundary) && within_key(path, grant)
    };
    if writes.iter().any(own) {
        PrivateAccess::Write
    } else if reads.iter().any(own) {
        PrivateAccess::Read
    } else if navigation.contains(path) {
        PrivateAccess::Navigate
    } else {
        PrivateAccess::Denied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| (*path).to_owned()).collect()
    }

    #[test]
    fn migration_denial_order_is_stable_only_inside_the_same_denial_block() {
        let denial = |value: &[u8]| MigrationAce::ManagedDeny(value.to_vec());
        let ordered = |value: &[u8]| MigrationAce::Ordered(value.to_vec());
        let before = migration_ace_records([
            denial(b"account self deny"),
            denial(b"account inherited deny"),
            denial(b"group self deny"),
            ordered(b"host allow"),
            ordered(b"account allow"),
        ]);
        let after = migration_ace_records([
            denial(b"group self deny"),
            denial(b"account inherited deny"),
            denial(b"account self deny"),
            ordered(b"host allow"),
            ordered(b"account allow"),
        ]);
        assert_eq!(before, after);
        let across_allow = migration_ace_records([
            denial(b"account inherited deny"),
            denial(b"group self deny"),
            ordered(b"host allow"),
            denial(b"account self deny"),
            ordered(b"account allow"),
        ]);
        assert_ne!(
            before, across_allow,
            "a denial moved behind an allow changes AccessCheck"
        );
        let across_unknown = migration_ace_records([
            denial(b"account self deny"),
            ordered(b"callback ACE"),
            denial(b"group self deny"),
        ]);
        let reordered_unknown = migration_ace_records([
            denial(b"group self deny"),
            ordered(b"callback ACE"),
            denial(b"account self deny"),
        ]);
        assert_ne!(across_unknown, reordered_unknown);
    }

    #[test]
    fn migration_records_keep_denial_masks_flags_sids_and_ordered_allows() {
        let original = migration_ace_records([
            MigrationAce::ManagedDeny(vec![1, 0, 0xa0, 0, 1]),
            MigrationAce::Ordered(vec![0, 0, 0x80, 0, 1]),
            MigrationAce::Ordered(vec![0, 0, 0x80, 0, 2]),
        ]);
        for byte in [1, 2, 4] {
            let mut denied = vec![1, 0, 0xa0, 0, 1];
            denied[byte] ^= 1;
            assert_ne!(
                original,
                migration_ace_records([
                    MigrationAce::ManagedDeny(denied),
                    MigrationAce::Ordered(vec![0, 0, 0x80, 0, 1]),
                    MigrationAce::Ordered(vec![0, 0, 0x80, 0, 2]),
                ])
            );
        }
        assert_ne!(
            original,
            migration_ace_records([
                MigrationAce::ManagedDeny(vec![1, 0, 0xa0, 0, 1]),
                MigrationAce::Ordered(vec![0, 0, 0x80, 0, 2]),
                MigrationAce::Ordered(vec![0, 0, 0x80, 0, 1]),
            ])
        );
    }

    #[test]
    fn verified_navigation_allows_commute_only_inside_their_own_allow_block() {
        let navigation = |value: &[u8]| MigrationAce::ManagedNavigationAllow(value.to_vec());
        let ordered = |value: &[u8]| MigrationAce::Ordered(value.to_vec());
        let before = migration_ace_records([
            ordered(b"host allow"),
            navigation(b"slot A allow attributes"),
            navigation(b"slot B allow attributes"),
        ]);
        assert_eq!(
            before,
            migration_ace_records([
                ordered(b"host allow"),
                navigation(b"slot B allow attributes"),
                navigation(b"slot A allow attributes"),
            ])
        );
        for boundary in [
            b"host allow".as_slice(),
            b"callback ACE",
            b"unrelated explicit deny",
        ] {
            assert_ne!(
                migration_ace_records([
                    navigation(b"slot A allow attributes"),
                    ordered(boundary),
                    navigation(b"slot B allow attributes"),
                ]),
                migration_ace_records([
                    navigation(b"slot B allow attributes"),
                    ordered(boundary),
                    navigation(b"slot A allow attributes"),
                ])
            );
        }
        assert_ne!(
            before,
            migration_ace_records([
                ordered(b"host allow"),
                navigation(b"slot A allow attributes"),
            ]),
            "revoking another slot's navigation remains visible"
        );
        assert_ne!(
            before,
            migration_ace_records([
                ordered(b"host allow"),
                navigation(b"slot A allow attributes"),
                navigation(b"slot B allow content"),
            ]),
            "allow mask changes remain visible"
        );
        assert_ne!(
            before,
            migration_ace_records([
                ordered(b"host allow"),
                navigation(b"slot A inheritable allow attributes"),
                navigation(b"slot B allow attributes"),
            ]),
            "allow inheritance changes remain visible"
        );
    }

    #[test]
    fn nested_private_roots_allow_only_the_owned_path_and_its_strict_ancestors() {
        let roots = keys(&[r"c:\home", r"c:\home\data", r"c:\home\data\scratch"]);
        let own = keys(&[r"c:\home\data\scratch\content\owner\worktree"]);
        let navigation = navigation_ancestor_keys(&own, &roots);
        assert_eq!(
            navigation,
            BTreeSet::from_iter(keys(&[
                r"c:\home",
                r"c:\home\data",
                r"c:\home\data\scratch",
                r"c:\home\data\scratch\content",
                r"c:\home\data\scratch\content\owner",
            ]))
        );
        for ancestor in &navigation {
            assert_eq!(
                private_access(ancestor, &roots, &[], &own, &navigation),
                PrivateAccess::Navigate
            );
        }
        assert_eq!(
            private_access(&own[0], &roots, &[], &own, &navigation),
            PrivateAccess::Write
        );
        assert_eq!(
            private_access(
                &(own[0].clone() + r"\new.txt"),
                &roots,
                &[],
                &own,
                &navigation
            ),
            PrivateAccess::Write
        );
        for other in [
            r"c:\home\secret.txt",
            r"c:\home\data\scratch\content\other",
            r"c:\homes\data",
        ] {
            assert_eq!(
                private_access(other, &roots, &[], &own, &navigation),
                PrivateAccess::Denied
            );
        }
    }

    #[test]
    fn switching_grants_revokes_old_navigation_and_preserves_exact_read_only_files() {
        let roots = keys(&[r"c:\private"]);
        let old = keys(&[r"c:\private\a\worktree"]);
        let current = keys(&[r"c:\private\b\input.txt"]);
        let old_navigation = navigation_ancestor_keys(&old, &roots);
        let navigation = navigation_ancestor_keys(&current, &roots);
        assert!(old_navigation.contains(r"c:\private\a"));
        assert!(!navigation.contains(r"c:\private\a"));
        assert_eq!(
            private_access(r"c:\private\a", &roots, &current, &[], &navigation),
            PrivateAccess::Denied
        );
        assert_eq!(
            private_access(r"c:\private\b", &roots, &current, &[], &navigation),
            PrivateAccess::Navigate
        );
        assert_eq!(
            private_access(&current[0], &roots, &current, &[], &navigation),
            PrivateAccess::Read
        );
        assert_eq!(
            private_access(
                r"c:\private\b\sibling.txt",
                &roots,
                &current,
                &[],
                &navigation
            ),
            PrivateAccess::Denied
        );
        assert_eq!(
            private_access(r"c:\private", &roots, &[], &[], &BTreeSet::new()),
            PrivateAccess::Denied
        );
    }

    #[test]
    fn a_broad_grant_does_not_open_a_nested_private_boundary() {
        let roots = keys(&[r"c:\private", r"c:\private\owned\nested"]);
        let own = keys(&[r"c:\private\owned"]);
        let navigation = navigation_ancestor_keys(&own, &roots);
        assert_eq!(
            private_access(
                r"c:\private\owned\nested\secret.txt",
                &roots,
                &[],
                &own,
                &navigation
            ),
            PrivateAccess::Denied
        );
        assert!(!navigation.contains(r"c:\private\owned\nested"));
    }

    #[test]
    fn unowned_boundaries_and_external_paths_cannot_authorize_navigation() {
        let roots = keys(&[r"c:\private", r"c:\private\nested"]);
        let unowned = keys(&[r"c:\private", r"c:\private\nested", r"c:\other\owned"]);
        let navigation = navigation_ancestor_keys(&unowned, &roots);
        assert!(navigation.is_empty());
        assert_eq!(
            private_access(
                r"c:\private\nested",
                &roots,
                &unowned,
                &unowned,
                &navigation
            ),
            PrivateAccess::Denied
        );
    }
}
