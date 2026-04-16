use std::fs;
use std::path::Path;

use crate::parsers::NormalizedParse;

pub fn normalize_file(path: &Path) -> std::io::Result<NormalizedParse<Vec<String>>> {
    let contents = fs::read_to_string(path)?;
    let mut errors = Vec::new();

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if lower.contains("error") || lower.contains("ошибк") || lower.contains("exception") {
            errors.push(trimmed.to_owned());
        }
    }

    Ok(NormalizedParse::default().with_payload(errors))
}

#[cfg(test)]
mod tests {
    use super::normalize_file;
    use tempfile::tempdir;

    #[test]
    fn extracts_error_like_lines() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("vanessa.log");
        std::fs::write(&path, "info line\nERROR boom\nИсключение\nОшибка теста\n").expect("write");

        let parsed = normalize_file(&path).expect("parse");
        assert_eq!(
            parsed.payload.expect("payload"),
            vec!["ERROR boom".to_owned(), "Ошибка теста".to_owned()]
        );
    }
}
