//! Keeps the three copies of the restriction lint list in step.
//!
//! The list is written out in `src/lib.rs` (the `deny` for non-test builds and
//! the `allow` on the `testing` module) and in `src/main.rs` (the AVR-only
//! `deny`). Cargo's `[lints]` table can't express `not(test)` or an AVR-only
//! condition, so the copies can't be merged, and this test is what stops one
//! drifting from the others.

use std::collections::BTreeSet;

const LIB_RS: &str = include_str!("../../src/lib.rs");
const MAIN_RS: &str = include_str!("../../src/main.rs");

/// The lint names in the attribute list that opens at the first `(` after
/// `opener`, which is searched for from `from`. Panics if the anchor is
/// missing, if the list is empty, or if an entry isn't a `clippy::` lint, so
/// the test can't pass on a set it failed to read.
fn lints_after(source: &str, label: &str, from: usize, opener: &str) -> BTreeSet<String> {
    let rest = source.get(from..).expect("anchor offset is in range");
    let start = rest
        .find(opener)
        .unwrap_or_else(|| panic!("{label}: anchor {opener:?} not found"))
        + opener.len();
    let list = rest
        .get(start..)
        .and_then(|tail| tail.split_once(')'))
        .unwrap_or_else(|| panic!("{label}: list after {opener:?} never closes"))
        .0;
    let lints: BTreeSet<String> = list
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            assert!(
                entry.starts_with("clippy::") && !entry.contains(char::is_whitespace),
                "{label}: unexpected entry {entry:?} in the lint list"
            );
            entry.to_owned()
        })
        .collect();
    assert!(!lints.is_empty(), "{label}: the lint list is empty");
    lints
}

/// The offset of `needle`, which must occur exactly once in `source`.
fn only(source: &str, label: &str, needle: &str) -> usize {
    let mut found = source.match_indices(needle);
    let (at, _) = found
        .next()
        .unwrap_or_else(|| panic!("{label}: anchor {needle:?} not found"));
    assert!(
        found.next().is_none(),
        "{label}: anchor {needle:?} occurs more than once"
    );
    at
}

#[test]
fn restriction_lint_lists_match() {
    // lib.rs deny list, which follows the `not(test)` condition.
    let lib_deny = lints_after(
        LIB_RS,
        "src/lib.rs deny",
        only(LIB_RS, "src/lib.rs deny", "not(test),"),
        "deny(",
    );

    // lib.rs allow list, which sits directly above `pub mod testing;`.
    let testing_at = only(LIB_RS, "src/lib.rs allow", "pub mod testing;");
    let allow_at = LIB_RS
        .get(..testing_at)
        .and_then(|before| before.rfind("#[allow("))
        .expect("src/lib.rs allow: no allow attribute above `pub mod testing;`");
    let lib_allow = lints_after(LIB_RS, "src/lib.rs allow", allow_at, "#[allow(");

    // main.rs deny list, which is the only one in the file.
    let main_deny = lints_after(
        MAIN_RS,
        "src/main.rs deny",
        only(MAIN_RS, "src/main.rs deny", "deny("),
        "deny(",
    );

    let reference = &lib_deny;
    let mut problems = Vec::new();
    for (name, set) in [
        ("src/lib.rs allow on testing", &lib_allow),
        ("src/main.rs deny", &main_deny),
    ] {
        for lint in reference.difference(set) {
            problems.push(format!(
                "{lint} is in src/lib.rs deny but missing from {name}"
            ));
        }
        for lint in set.difference(reference) {
            problems.push(format!(
                "{lint} is in {name} but missing from src/lib.rs deny"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "restriction lint lists have drifted apart\n{}",
        problems.join("\n")
    );
}
