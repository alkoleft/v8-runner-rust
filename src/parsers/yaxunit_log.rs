use regex::Regex;
use std::io::BufRead;
use std::path::Path;
use std::sync::OnceLock;

use crate::domain::execution::ExecutionError;
use crate::parsers::NormalizedParse;

fn timestamp_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{2}:\d{2}:\d{2}\.\d{3}").expect("regex"))
}

pub fn parse_file(path: &Path) -> std::io::Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    Ok(parse_reader(reader))
}

pub fn parse_reader<R: BufRead>(reader: R) -> Vec<String> {
    let mut errors = Vec::new();
    let mut current = String::new();
    let mut in_error = false;

    for line in reader.lines().map_while(Result::ok) {
        if line.contains("[ERR]") {
            if in_error && !current.trim().is_empty() {
                errors.push(current.trim().to_owned());
                current.clear();
            }
            in_error = true;
            current.push_str(&line);
            continue;
        }

        if in_error && timestamp_pattern().is_match(line.trim()) {
            if !current.trim().is_empty() {
                errors.push(current.trim().to_owned());
            }
            current.clear();
            in_error = false;
            continue;
        }

        if in_error {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(&line);
        }
    }

    if in_error && !current.trim().is_empty() {
        errors.push(current.trim().to_owned());
    }

    errors
}

pub fn normalize_file(path: &Path) -> std::io::Result<NormalizedParse<Vec<String>>> {
    if !path.exists() {
        return Ok(NormalizedParse {
            warnings: vec!["YaXUnit log file was not produced".to_owned()],
            ..NormalizedParse::default()
        });
    }

    let payload = parse_file(path)?;
    Ok(normalize_errors(payload))
}

#[cfg(test)]
pub fn normalize_reader<R: BufRead>(reader: R) -> NormalizedParse<Vec<String>> {
    normalize_errors(parse_reader(reader))
}

fn normalize_errors(errors: Vec<String>) -> NormalizedParse<Vec<String>> {
    NormalizedParse {
        payload: Some(errors.clone()),
        metrics: None,
        diagnostics: errors.clone(),
        errors: errors
            .iter()
            .cloned()
            .map(|message| ExecutionError::new("yaxunit_log_error", message))
            .collect(),
        warnings: Vec::new(),
        artifacts: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_reader, parse_reader};

    const YAXUNIT_LOG_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/parsers/yaxunit.log"
    ));

    #[test]
    fn extracts_multiline_error_block_from_fixture() {
        let errors = parse_reader(std::io::Cursor::new(YAXUNIT_LOG_FIXTURE));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("failed block"));
        assert!(errors[0].contains("more details"));
    }

    #[test]
    fn extracts_multiple_multiline_error_blocks() {
        let log = "\
12:00:00.000 [INF] start
12:00:01.000 [ERR] first
details
12:00:02.000 [INF] mid
12:00:03.000 [ERR] second
stack line
";
        let errors = parse_reader(std::io::Cursor::new(log));
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("details"));
        assert!(errors[1].contains("stack line"));
    }

    #[test]
    fn normalizes_error_blocks_without_losing_multiline_content() {
        let normalized = normalize_reader(std::io::Cursor::new(YAXUNIT_LOG_FIXTURE));
        assert_eq!(normalized.errors.len(), 1);
        assert!(normalized.errors[0].message.contains("failed block"));
        assert!(normalized.diagnostics[0].contains("more details"));
    }
}
