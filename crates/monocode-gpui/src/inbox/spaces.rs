//! ClickUp spaces for a trigger filter: the picker rows and the lookup. Used by the
//! Workflows event inspector.
use crate::runtime::{Database, locked};

/// Picker rows for a ClickUp trigger. The trigger stores the space id so a
/// rename keeps the filter working; the name is only what the user reads.
pub(crate) fn space_options(spaces: &[(String, String)], saved: &str) -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "Any space".to_string())];
    options.extend(spaces.iter().cloned());
    let saved = saved.trim();
    // Not in the live list (deleted, or still loading): keep the saved id
    // selectable rather than silently widening the filter.
    if !saved.is_empty() && !options.iter().any(|(id, _)| id == saved) {
        options.push((saved.to_string(), format!("Space {saved}")));
    }
    options
}

pub(crate) fn space_label(spaces: &[(String, String)], saved: &str) -> String {
    space_options(spaces, saved)
        .into_iter()
        .find(|(id, _)| id == saved.trim())
        .map_or_else(|| "Any space".into(), |(_, name)| name)
}

/// The ClickUp spaces of the connected account as (id, name). Blocks (network): call it off the
/// UI thread.
pub(crate) fn list_spaces(database: &Database) -> Result<Vec<(String, String)>, String> {
    let root = locked(database)?
        .data_dir()
        .map(std::path::Path::to_path_buf)
        .ok_or("No data directory")?;
    Ok(monocode_core::inbox::clickup::clickup_list_spaces(root)?
        .into_iter()
        .map(|space| (space.id, space.name))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spaces() -> Vec<(String, String)> {
        vec![("901".into(), "Engineering".into())]
    }

    #[test]
    fn space_options_keep_a_saved_id_that_is_not_listed() {
        let options = space_options(&spaces(), "901");
        assert_eq!(
            options,
            [
                ("".to_string(), "Any space".to_string()),
                ("901".to_string(), "Engineering".to_string())
            ]
        );
        let options = space_options(&spaces(), " 77 ");
        assert_eq!(
            options.last(),
            Some(&("77".to_string(), "Space 77".to_string()))
        );
        assert_eq!(space_label(&spaces(), "901"), "Engineering");
        assert_eq!(space_label(&spaces(), ""), "Any space");
        assert_eq!(space_label(&spaces(), "77"), "Space 77");
    }
}
