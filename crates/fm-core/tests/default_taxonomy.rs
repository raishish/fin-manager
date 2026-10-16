//! The shipped default category list (T2.1, SPEC decision 27) must load as a valid
//! `Taxonomy` and keep the shape that the test data and evals rely on.

use std::collections::HashMap;

use fm_core::model::Taxonomy;

const DEFAULT_V1: &str = include_str!("../assets/taxonomy/default-v1.json");

fn default_v1() -> Taxonomy {
    serde_json::from_str(DEFAULT_V1).expect("default-v1.json parses as a Taxonomy")
}

#[test]
fn loads_and_validates() {
    let t = default_v1();
    t.validate()
        .expect("default-v1.json passes Taxonomy::validate");
    assert_eq!(t.version.base, "1");
    assert_eq!(t.version.revision, 0);
}

#[test]
fn has_the_documented_size() {
    let t = default_v1();
    assert_eq!(t.groups.len(), 17);
    assert_eq!(t.categories.len(), 66);
}

#[test]
fn keys_are_snake_case_and_nested_under_their_group() {
    let t = default_v1();
    let snake = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    let group_keys: HashMap<_, _> = t.groups.iter().map(|g| (g.id, g.key.as_str())).collect();
    for g in &t.groups {
        assert!(snake(&g.key), "group key {:?}", g.key);
    }
    for c in &t.categories {
        let (group, name) = c
            .key
            .split_once('.')
            .unwrap_or_else(|| panic!("category key {:?} has no group prefix", c.key));
        assert_eq!(Some(&group), group_keys.get(&c.group_id), "{:?}", c.key);
        assert!(snake(name), "category key {:?}", c.key);
    }
}

#[test]
fn ships_unedited() {
    let t = default_v1();
    assert!(t.groups.iter().all(|g| !g.hidden));
    assert!(
        t.categories
            .iter()
            .all(|c| !c.hidden && c.merged_into.is_none())
    );
    assert!(
        t.groups
            .iter()
            .all(|g| t.categories.iter().any(|c| c.group_id == g.id)),
        "every group has at least one category"
    );
}
