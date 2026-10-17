//! Shared confirmation gate for destructive / outward-facing commands.
//!
//! One copy, used by `fileshare`, `sign`, and `service-account`. Callers own
//! the `action` / `detail` wording; this module owns the gate itself.

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{Context, Result};

/// Gate a destructive / outward-facing action behind explicit confirmation.
///
/// `--yes` proceeds unconditionally. Without it, an interactive TTY is prompted
/// y/N; a non-interactive caller that omitted `--yes` is blocked
/// deterministically (so an unattended script never silently deletes, revokes,
/// sends, voids, or writes a new version).
pub(crate) fn confirm_destructive(action: &str, detail: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
    if !interactive {
        anyhow::bail!("'{action}' {detail}. Re-run with --yes to proceed.");
    }
    eprint!("'{action}' {detail}. Proceed? [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("failed to read confirmation from stdin")?;
    let answer = answer.trim().to_ascii_lowercase();
    if answer == "y" || answer == "yes" {
        Ok(())
    } else {
        anyhow::bail!("aborted: '{action}' not confirmed");
    }
}

#[cfg(test)]
mod tests {
    use super::confirm_destructive;

    #[test]
    fn yes_proceeds() {
        assert!(confirm_destructive("x", "does y", true).is_ok());
    }

    /// In the test harness stdin/stderr are not TTYs, so omitting `--yes` must
    /// hard-error rather than prompt — and the message names the action, the
    /// detail, and the flag that proceeds.
    #[test]
    fn non_interactive_without_yes_blocks_with_the_action_and_detail() {
        let err = confirm_destructive("thing delete", "removes it", false)
            .expect_err("a non-interactive caller without --yes must be blocked");
        assert_eq!(
            err.to_string(),
            "'thing delete' removes it. Re-run with --yes to proceed."
        );
    }
}
