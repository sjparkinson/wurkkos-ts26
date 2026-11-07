//! docs/ui.md's examples, for running on the firmware. `ts26::testing` reads
//! them and says how a row has to look; this adds where in the file a row's
//! wrong. `cargo test` runs them against the library in `src/torch.rs`, and
//! `cargo xtask hex` against the firmware on the simulated chip (`sim.rs`).

use ts26::testing::{self, Example};

/// docs/ui.md, built in so a change to it rebuilds xtask.
pub const UI_MD: &str = include_str!("../../docs/ui.md");

/// Every example in a UI spec written like docs/ui.md, or the first line
/// that's wrong.
pub fn examples(markdown: &str) -> Result<Vec<Example<'_>>, String> {
    testing::examples(markdown)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("docs/ui.md:{}: {e}", e.line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_which_line_is_wrong() {
        let spec = "## Off\n\n| Action | Does | Status |\n|---|---|---|\n";
        assert_eq!(
            examples(spec),
            Err("docs/ui.md:3: this table of actions has no Example column".into())
        );
        assert!(!examples(UI_MD).unwrap().is_empty());
    }
}
