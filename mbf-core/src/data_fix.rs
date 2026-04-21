use anyhow::{anyhow, Context, Result};
use std::path::Path;

pub fn fix_colour_schemes(path: impl AsRef<Path>) -> Result<()> {
    let data_file_buf = crate::hal().read_file(path.as_ref())
        .context("Opening player data file for reading")?;

    let mut player_data: serde_json::Value =
        serde_json::from_slice(&data_file_buf).context("Parsing PlayerData.dat as JSON")?;

    let local_players = player_data
        .get_mut("localPlayers")
        .ok_or(anyhow!("No localPlayers array found"))?
        .as_array_mut()
        .ok_or(anyhow!("localPlayers was not a valid array"))?;

    for player in local_players {
        let color_schemes_settings = player
            .get_mut("colorSchemesSettings")
            .ok_or(anyhow!("No colorSchemesSettings found"))?
            .as_object_mut()
            .ok_or(anyhow!("colorSchemesSettings were invalid"))?;

        color_schemes_settings.insert("selectedColorSchemeId".to_string(), "User0".into());
    }

    let output_str =
        serde_json::to_string(&player_data).context("Converting player data back to JSON")?;

    crate::hal().write_file(path.as_ref(), output_str.as_bytes())?;

    Ok(())
}
