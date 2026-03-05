use schemars::schema_for;

use crate::config::Config;

/// Print the JSON Schema for `.edict.toml` to stdout.
pub fn run_schema() -> anyhow::Result<()> {
    let schema = schema_for!(Config);
    let json = serde_json::to_string_pretty(&schema)?;
    println!("{json}");
    Ok(())
}
