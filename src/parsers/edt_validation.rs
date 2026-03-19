use std::path::Path;

use tracing::warn;

use crate::domain::issue::{EdtIssue, Issue, IssueSeverity};

/// Parse `1cedtcli validate --file` log content into structured EDT issues.
///
/// Contract note: real `1cedtcli` output can vary by version. This parser accepts
/// tab-separated lines with 3..=6 columns and skips malformed rows with warnings.
pub fn parse(content: &str) -> Vec<Issue> {
    let mut issues = Vec::new();

    for (index, raw_line) in content.lines().enumerate() {
        let line_no = index + 1;
        if raw_line.trim().is_empty() {
            continue;
        }

        if let Some(issue) = parse_line(raw_line) {
            issues.push(issue);
            continue;
        }

        if is_header_line(raw_line) {
            continue;
        }

        warn!(
            line_no,
            line = raw_line,
            "skipping unrecognized edt validation line"
        );
    }

    issues
}

pub fn parse_path(path: &Path) -> std::io::Result<Vec<Issue>> {
    if !path.exists() {
        return Ok(vec![]);
    }

    Ok(parse(&std::fs::read_to_string(path)?))
}

fn parse_line(line: &str) -> Option<Issue> {
    let columns = line.split('\t').collect::<Vec<_>>();
    if columns.len() < 3 {
        return None;
    }

    let severity = parse_severity(columns[0].trim())?;
    let path = columns[1].trim();
    if path.is_empty() {
        return None;
    }

    // Supported layouts (all tab-separated):
    // 6 columns: severity, path, line, column, check, message
    // 5 columns: severity, path, line, column, message
    // 4 columns: severity, path, line, message
    // 3 columns: severity, path, message
    let (line_num, column_num, check, message) = match columns.len() {
        len if len >= 6 => {
            let message = columns[5..].join("\t");
            (
                parse_optional_u32(columns[2]),
                parse_optional_u32(columns[3]),
                non_empty(columns[4]),
                message,
            )
        }
        5 => (
            parse_optional_u32(columns[2]),
            parse_optional_u32(columns[3]),
            None,
            columns[4].to_owned(),
        ),
        4 => (
            parse_optional_u32(columns[2]),
            None,
            None,
            columns[3].to_owned(),
        ),
        3 => (None, None, None, columns[2].to_owned()),
        _ => return None,
    };

    let message = message.trim();
    if message.is_empty() {
        return None;
    }

    Some(Issue::Edt(EdtIssue {
        path: path.to_owned(),
        line: line_num,
        column: column_num,
        message: message.to_owned(),
        severity,
        check,
    }))
}

fn parse_optional_u32(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return None;
    }
    trimmed.parse::<u32>().ok()
}

fn parse_severity(value: &str) -> Option<IssueSeverity> {
    let normalized = value.trim().to_lowercase();
    if normalized.is_empty() {
        return None;
    }

    if normalized.starts_with("err") || normalized.contains("ошиб") || normalized == "e" {
        Some(IssueSeverity::Error)
    } else if normalized.starts_with("warn") || normalized.contains("предупр") || normalized == "w"
    {
        Some(IssueSeverity::Warning)
    } else if normalized.starts_with("info") || normalized.contains("инф") || normalized == "i" {
        Some(IssueSeverity::Info)
    } else {
        None
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn is_header_line(line: &str) -> bool {
    let columns: Vec<&str> = line.split('\t').collect();
    if columns.len() < 3 {
        return false;
    }

    let c0 = columns[0].trim().to_ascii_lowercase();
    let c1 = columns[1].trim().to_ascii_lowercase();
    let c_last = columns
        .last()
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();

    c0 == "severity"
        && c1 == "path"
        && (c_last == "message" || c_last == "issue" || c_last == "description")
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_path};
    use crate::domain::issue::{Issue, IssueSeverity};
    use tempfile::tempdir;

    #[test]
    fn parses_tsv_happy_path() {
        let issues = parse("ERROR\tCommonModules.Test\t12\t3\tUnusedVariables\tunused variable");

        assert_eq!(issues.len(), 1);
        match &issues[0] {
            Issue::Edt(issue) => {
                assert_eq!(issue.path, "CommonModules.Test");
                assert_eq!(issue.line, Some(12));
                assert_eq!(issue.column, Some(3));
                assert_eq!(issue.check.as_deref(), Some("UnusedVariables"));
                assert_eq!(issue.severity, IssueSeverity::Error);
            }
            _ => panic!("expected edt issue"),
        }
    }

    #[test]
    fn parses_message_with_embedded_tabs() {
        let issues = parse("WARNING\tCatalogs.Items\t\t\t\tpart1\tpart2");

        assert_eq!(issues.len(), 1);
        match &issues[0] {
            Issue::Edt(issue) => {
                assert_eq!(issue.message, "part1\tpart2");
                assert_eq!(issue.severity, IssueSeverity::Warning);
                assert!(issue.line.is_none());
                assert!(issue.column.is_none());
            }
            _ => panic!("expected edt issue"),
        }
    }

    #[test]
    fn skips_malformed_rows_without_panicking() {
        let issues =
            parse("random noise\nERROR\t\t12\t3\tcheck\tmessage\nINFO\tCatalogs.Items\tmessage");

        assert_eq!(issues.len(), 1);
        match &issues[0] {
            Issue::Edt(issue) => {
                assert_eq!(issue.path, "Catalogs.Items");
                assert_eq!(issue.severity, IssueSeverity::Info);
            }
            _ => panic!("expected edt issue"),
        }
    }

    #[test]
    fn empty_log_produces_empty_issues() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn parses_cyrillic_severity_labels() {
        let issues = parse("Ошибка\tCatalogs.Items\t1\t2\tRule\tbad");

        assert_eq!(issues.len(), 1);
        match &issues[0] {
            Issue::Edt(issue) => assert_eq!(issue.severity, IssueSeverity::Error),
            _ => panic!("expected edt issue"),
        }
    }

    #[test]
    fn rejects_row_with_empty_message_column() {
        let issues = parse("ERROR\tCatalogs.Items\t1\t2\tRule\t");
        assert!(issues.is_empty());
    }

    #[test]
    fn does_not_treat_issue_with_header_words_as_header_row() {
        let issues =
            parse("ERROR\tCommonModules.PathUtils\t1\t1\tRule\tInvalid severity/path message");
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn parse_path_returns_empty_for_missing_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("missing.log");

        let issues = parse_path(&path).expect("parse path");

        assert!(issues.is_empty());
    }
}
