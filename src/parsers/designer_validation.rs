use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::domain::issue::{Issue, IssueSeverity, ModuleIssue, ObjectIssue};

static MODULE_ISSUE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\{(?:(?P<extension>[[:word:]А-Яа-яЁё_]+)\s)?(?P<path>[^}]+)\((?P<line>\d+),(?P<column>\d+)\)}:\s*(?P<message>.+)$")
        .expect("module issue regex")
});
static MODULE_CONTEXT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\{\d+\}\s*:").expect("module context regex"));

pub fn parse(content: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_success_message(trimmed) {
            continue;
        }

        if let Some(issue) = parse_module_issue(trimmed) {
            if lines
                .peek()
                .is_some_and(|next_line| is_module_context(next_line.trim()))
            {
                let _ = lines.next();
            }
            issues.push(issue);
            continue;
        }

        if let Some(issue) = parse_object_issue(trimmed) {
            issues.push(issue);
        }
    }

    issues
}

pub fn parse_path(path: &Path) -> std::io::Result<Vec<Issue>> {
    if !path.exists() {
        return Ok(vec![]);
    }

    Ok(parse(&std::fs::read_to_string(path)?))
}

fn parse_module_issue(line: &str) -> Option<Issue> {
    let captures = MODULE_ISSUE_RE.captures(line)?;
    let extension = captures
        .name("extension")
        .map(|m| format!("{} ", m.as_str()));
    let path = format!(
        "{}{}",
        extension.as_deref().unwrap_or(""),
        captures.name("path")?.as_str()
    );
    let line_num = captures.name("line")?.as_str().parse().ok()?;
    let column = captures.name("column")?.as_str().parse().ok()?;
    let message = captures.name("message")?.as_str().trim().to_owned();

    Some(Issue::Module(ModuleIssue {
        path,
        line: Some(line_num),
        column: Some(column),
        severity: classify_severity(&message),
        message,
    }))
}

fn parse_object_issue(line: &str) -> Option<Issue> {
    let (first, tail) = line.split_once(' ')?;
    let (second, rest) = match tail.split_once(' ') {
        Some((second, rest)) => (second, Some(rest.trim())),
        None => ("", None),
    };

    let (object, message) = if contains_issue_marker(tail.trim()) {
        (first.to_owned(), tail.trim().to_owned())
    } else if let Some(rest) = rest {
        if contains_issue_marker(rest) {
            (format!("{first} {second}"), rest.to_owned())
        } else {
            return None;
        }
    } else {
        return None;
    };
    let severity = classify_severity(&message);

    Some(Issue::Object(ObjectIssue {
        object,
        message,
        severity,
    }))
}

fn classify_severity(message: &str) -> IssueSeverity {
    let lower = message.to_lowercase();
    if contains_warning_marker(&lower) {
        IssueSeverity::Warning
    } else {
        IssueSeverity::Error
    }
}

fn is_module_context(line: &str) -> bool {
    MODULE_CONTEXT_RE.is_match(line)
}

fn contains_issue_marker(message: &str) -> bool {
    let lower = message.to_lowercase();
    contains_warning_marker(&lower)
        || lower.contains("error")
        || lower.contains("ошиб")
        || lower.contains("fatal")
        || lower.contains("неразрешим")
}

fn is_success_message(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("ошибок не обнаружено")
}

fn contains_warning_marker(lower_message: &str) -> bool {
    lower_message.contains("warning")
        || lower_message.contains("warn:")
        || lower_message.contains("предупреждение")
        || lower_message.contains("warning:")
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_path};
    use crate::domain::issue::{Issue, IssueSeverity};
    use tempfile::tempdir;

    const DESIGNER_VALIDATION_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/parsers/designer_validation.log"
    ));

    #[test]
    fn parses_realistic_fixture_sample() {
        let issues = parse(DESIGNER_VALIDATION_FIXTURE);

        assert_eq!(issues.len(), 4);
        match &issues[0] {
            Issue::Module(issue) => {
                assert_eq!(issue.path, "CommonModules.TestModule");
                assert_eq!(issue.line, Some(12));
                assert_eq!(issue.column, Some(3));
                assert_eq!(issue.severity, IssueSeverity::Error);
            }
            _ => panic!("expected module issue"),
        }

        match &issues[1] {
            Issue::Object(issue) => assert_eq!(issue.object, "Catalogs.Items"),
            _ => panic!("expected object issue"),
        }

        match &issues[2] {
            Issue::Object(issue) => {
                assert_eq!(issue.object, "Справочники.Номенклатура");
                assert_eq!(issue.severity, IssueSeverity::Warning);
            }
            _ => panic!("expected warning object issue"),
        }

        match &issues[3] {
            Issue::Object(issue) => {
                assert_eq!(issue.object, "ОбщаяФорма.НастройкиРегистрации.Справка");
                assert_eq!(issue.severity, IssueSeverity::Error);
            }
            _ => panic!("expected unresolvable reference issue"),
        }
    }

    #[test]
    fn parses_object_issue() {
        let issues = parse("Catalogs.Items Ошибка проверки объекта");

        assert_eq!(issues.len(), 1);
        match &issues[0] {
            Issue::Object(issue) => {
                assert_eq!(issue.object, "Catalogs.Items");
                assert_eq!(issue.severity, IssueSeverity::Error);
            }
            _ => panic!("expected object issue"),
        }
    }

    #[test]
    fn parses_cyrillic_names() {
        let issues = parse("Справочники.Номенклатура Предупреждение: неиспользуемый реквизит");

        assert_eq!(issues.len(), 1);
        match &issues[0] {
            Issue::Object(issue) => {
                assert_eq!(issue.object, "Справочники.Номенклатура");
                assert_eq!(issue.severity, IssueSeverity::Warning);
            }
            _ => panic!("expected object issue"),
        }
    }

    #[test]
    fn keeps_issue_after_module_issue_when_no_context_line_exists() {
        let issues =
            parse("{CommonModules.A(1,1)}: Ошибка компиляции\nКонфигурация Ошибка проверки");

        assert_eq!(issues.len(), 2);
        match &issues[1] {
            Issue::Object(issue) => assert_eq!(issue.object, "Конфигурация"),
            _ => panic!("expected object issue"),
        }
    }

    #[test]
    fn parses_root_level_object_issue_without_dot() {
        let issues = parse("Конфигурация Ошибка проверки объекта");

        assert_eq!(issues.len(), 1);
        match &issues[0] {
            Issue::Object(issue) => assert_eq!(issue.object, "Конфигурация"),
            _ => panic!("expected object issue"),
        }
    }

    #[test]
    fn ignores_noise_only_log() {
        let issues = parse("some random text\n\nanother line");

        assert!(issues.is_empty());
    }

    #[test]
    fn parses_empty_log() {
        let issues = parse("");

        assert!(issues.is_empty());
    }

    #[test]
    fn ignores_success_message() {
        let issues = parse("Синтаксических ошибок не обнаружено!");

        assert!(issues.is_empty());
    }

    #[test]
    fn parse_path_returns_empty_for_missing_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("missing.log");

        let issues = parse_path(&path).expect("parse path");

        assert!(issues.is_empty());
    }

    #[test]
    fn parses_unresolvable_references_issue() {
        let issues = parse(
            "ОбщаяФорма.НастройкиРегистрации.Справка Неразрешимые ссылки на объекты метаданных (1)",
        );

        assert_eq!(issues.len(), 1);
        match &issues[0] {
            Issue::Object(issue) => {
                assert_eq!(issue.object, "ОбщаяФорма.НастройкиРегистрации.Справка");
                assert_eq!(issue.severity, IssueSeverity::Error);
            }
            _ => panic!("expected object issue"),
        }
    }
}
