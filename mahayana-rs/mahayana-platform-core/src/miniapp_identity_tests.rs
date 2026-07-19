use super::*;

#[test]
fn github_spellings_produce_the_same_instance_id() {
    let https = plugin_instance_id("https://GitHub.com/Fabushi/Example.git", "reader").unwrap();
    let ssh = plugin_instance_id("git@github.com:fabushi/example.git", "reader").unwrap();
    assert_eq!(https, ssh);
}

#[test]
fn official_identity_is_stable_across_hosts() {
    assert_eq!(
        plugin_instance_id("https://github.com/fabushi/fabushi", "global-dharma").unwrap(),
        "global-dharma@184d7e8c5a737b9e1f62590f834fda9d"
    );
}

#[test]
fn same_named_plugins_from_different_repositories_are_isolated() {
    let left = plugin_instance_id("https://github.com/acme/left", "reader").unwrap();
    let right = plugin_instance_id("https://github.com/acme/right", "reader").unwrap();
    assert_ne!(left, right);
}

#[test]
fn credentials_and_ref_queries_do_not_change_identity() {
    let source =
        canonical_repository_source("https://token@example.com/owner/repo.git?ref=main#readme")
            .unwrap();
    assert_eq!(source, "example.com/owner/repo");
}
