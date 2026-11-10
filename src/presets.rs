use std::path::PathBuf;
pub fn path(name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err("preset names must contain only letters, digits, '-' and '_'".into());
    }
    // Keep all investigation artifacts inside the project/current directory.
    Ok(PathBuf::from("captures/presets").join(format!("{name}.json")))
}
