//! Plan private navigation from already validated, canonical Windows path keys.
use std::collections::BTreeSet;

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
