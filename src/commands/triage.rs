/// Triage is now handled by `bn triage` directly.
/// This module is kept as a thin wrapper for backwards compatibility
/// with `edict run triage`.
use crate::subprocess::Tool;

/// Run triage: delegates to `bn triage` in the default workspace
///
/// # Errors
/// Returns an error if the default workspace cannot be resolved or the
/// `bn triage` subprocess fails.
pub fn run_triage() -> anyhow::Result<()> {
    let output = Tool::new("bn")
        .arg("triage")
        .in_workspace("default")?
        .run_ok()?;

    print!("{}", output.stdout);
    Ok(())
}
