use std::io::BufRead;

use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::Reader;
use thiserror::Error;

use crate::domain::test::{TestCase, TestReport, TestStatus, TestSuite, TestSummary};

#[derive(Debug, Error)]
pub enum JunitError {
    #[error("junit file is empty or contains no test suites")]
    Empty,
    #[error("failed to parse junit xml: {0}")]
    Malformed(String),
}

#[derive(Debug, Default)]
struct SuiteBuilder {
    name: String,
    duration_ms: u64,
    cases: Vec<TestCase>,
}

#[derive(Debug, Default)]
struct CaseBuilder {
    name: String,
    class_name: Option<String>,
    duration_ms: u64,
    status: TestStatus,
    failure_message: Option<String>,
    stack_trace: Option<String>,
}

impl Default for TestStatus {
    fn default() -> Self {
        Self::Passed
    }
}

pub fn parse<R: BufRead>(reader: R) -> Result<TestReport, JunitError> {
    let mut xml = Reader::from_reader(reader);
    xml.config_mut().trim_text(false);

    let mut buf = Vec::new();
    let mut suites = Vec::new();
    let mut suite_stack: Vec<SuiteBuilder> = Vec::new();
    let mut current_case: Option<CaseBuilder> = None;

    loop {
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(ref event)) if event.name().as_ref() == b"testsuite" => {
                suite_stack.push(SuiteBuilder {
                    name: attr_value(event, b"name"),
                    duration_ms: seconds_attr_to_ms(option_attr_value(event, b"time").as_deref()),
                    cases: Vec::new(),
                });
            }
            Ok(Event::End(ref event)) if event.name().as_ref() == b"testsuite" => {
                if let Some(suite) = suite_stack.pop() {
                    suites.push(TestSuite {
                        name: suite.name,
                        cases: suite.cases,
                        duration_ms: suite.duration_ms,
                    });
                }
            }
            Ok(Event::Start(ref event)) if event.name().as_ref() == b"testcase" => {
                current_case = Some(CaseBuilder {
                    name: attr_value(event, b"name"),
                    class_name: option_attr_value(event, b"classname"),
                    duration_ms: seconds_attr_to_ms(option_attr_value(event, b"time").as_deref()),
                    status: TestStatus::Passed,
                    failure_message: None,
                    stack_trace: None,
                });
            }
            Ok(Event::Empty(ref event)) if event.name().as_ref() == b"testcase" => {
                if let Some(suite) = suite_stack.last_mut() {
                    suite.cases.push(TestCase {
                        name: attr_value(event, b"name"),
                        class_name: option_attr_value(event, b"classname"),
                        status: TestStatus::Passed,
                        duration_ms: seconds_attr_to_ms(option_attr_value(event, b"time").as_deref()),
                        failure_message: None,
                        stack_trace: None,
                    });
                }
            }
            Ok(Event::End(ref event)) if event.name().as_ref() == b"testcase" => {
                if let (Some(case), Some(suite)) = (current_case.take(), suite_stack.last_mut()) {
                    suite.cases.push(TestCase {
                        name: case.name,
                        class_name: case.class_name,
                        status: case.status,
                        duration_ms: case.duration_ms,
                        failure_message: case.failure_message,
                        stack_trace: case.stack_trace,
                    });
                }
            }
            Ok(Event::Start(ref event)) if event.name().as_ref() == b"failure" => {
                if let Some(case) = &mut current_case {
                    let text = read_element_text(&mut xml, QName(b"failure"))?;
                    case.status = TestStatus::Failed;
                    case.failure_message = option_attr_value(event, b"message").or_else(|| {
                        let trimmed = text.trim();
                        (!trimmed.is_empty()).then(|| trimmed.to_owned())
                    });
                    case.stack_trace = non_empty(text);
                }
            }
            Ok(Event::Empty(ref event)) if event.name().as_ref() == b"failure" => {
                if let Some(case) = &mut current_case {
                    case.status = TestStatus::Failed;
                    case.failure_message = option_attr_value(event, b"message");
                    case.stack_trace = None;
                }
            }
            Ok(Event::Start(ref event)) if event.name().as_ref() == b"error" => {
                if let Some(case) = &mut current_case {
                    let text = read_element_text(&mut xml, QName(b"error"))?;
                    case.status = TestStatus::Error;
                    case.failure_message = option_attr_value(event, b"message").or_else(|| {
                        let trimmed = text.trim();
                        (!trimmed.is_empty()).then(|| trimmed.to_owned())
                    });
                    case.stack_trace = non_empty(text);
                }
            }
            Ok(Event::Empty(ref event)) if event.name().as_ref() == b"error" => {
                if let Some(case) = &mut current_case {
                    case.status = TestStatus::Error;
                    case.failure_message = option_attr_value(event, b"message");
                    case.stack_trace = None;
                }
            }
            Ok(Event::Start(ref event)) if event.name().as_ref() == b"skipped" => {
                if let Some(case) = &mut current_case {
                    case.status = TestStatus::Skipped;
                    let _ = read_element_text(&mut xml, QName(b"skipped"))?;
                }
            }
            Ok(Event::Empty(ref event)) if event.name().as_ref() == b"skipped" => {
                if let Some(case) = &mut current_case {
                    case.status = TestStatus::Skipped;
                }
            }
            Ok(Event::Eof) => {
                if !suite_stack.is_empty() || current_case.is_some() {
                    return Err(JunitError::Malformed(
                        "unexpected eof while parsing junit xml".to_owned(),
                    ));
                }
                break;
            }
            Ok(_) => {}
            Err(error) => return Err(JunitError::Malformed(error.to_string())),
        }
        buf.clear();
    }

    if suites.is_empty() {
        return Err(JunitError::Empty);
    }

    let mut summary = TestSummary {
        total: 0,
        passed: 0,
        failed: 0,
        skipped: 0,
        errors: 0,
    };
    for suite in &suites {
        for case in &suite.cases {
            summary.total += 1;
            match case.status {
                TestStatus::Passed => summary.passed += 1,
                TestStatus::Failed => summary.failed += 1,
                TestStatus::Skipped => summary.skipped += 1,
                TestStatus::Error => summary.errors += 1,
            }
        }
    }

    if summary.total == 0 {
        return Err(JunitError::Empty);
    }

    Ok(TestReport {
        summary,
        suites,
        extracted_errors: Vec::new(),
    })
}

fn attr_value(event: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> String {
    option_attr_value(event, key).unwrap_or_default()
}

fn option_attr_value(event: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    event
        .attributes()
        .flatten()
        .find(|attr| attr.key.as_ref() == key)
        .map(|attr| String::from_utf8_lossy(&attr.value).into_owned())
        .filter(|value| !value.is_empty())
}

fn seconds_attr_to_ms(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.parse::<f64>().ok())
        .map(|seconds| (seconds * 1000.0).round() as u64)
        .unwrap_or(0)
}

fn read_element_text<R: BufRead>(
    reader: &mut Reader<R>,
    end: QName<'_>,
) -> Result<String, JunitError> {
    let mut buf = Vec::new();
    let mut content = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(text)) => content.push_str(&String::from_utf8_lossy(text.as_ref())),
            Ok(Event::CData(text)) => content.push_str(&String::from_utf8_lossy(text.as_ref())),
            Ok(Event::End(event)) if event.name() == end => break,
            Ok(Event::Eof) => {
                return Err(JunitError::Malformed(
                    "unexpected eof while reading element text".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) => return Err(JunitError::Malformed(error.to_string())),
        }
        buf.clear();
    }
    Ok(content)
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{parse, JunitError};

    #[test]
    fn parses_nested_suites_and_multiline_failures() {
        let xml = r#"
<testsuites>
  <testsuite name="root" time="1.0">
    <testsuite name="nested" time="0.2">
      <testcase name="ok" classname="A" time="0.1" />
      <testcase name="bad" classname="B" time="0.1">
        <failure message="boom"><![CDATA[line1
line2]]></failure>
      </testcase>
    </testsuite>
  </testsuite>
</testsuites>
"#;
        let report = parse(std::io::Cursor::new(xml)).expect("report");
        assert_eq!(report.summary.total, 2);
        assert_eq!(report.summary.failed, 1);
        assert_eq!(report.suites.len(), 2);
        assert!(report.suites.iter().any(|suite| suite.name == "nested"));
        let failed = report
            .suites
            .iter()
            .flat_map(|suite| &suite.cases)
            .find(|case| case.name == "bad")
            .expect("failed case");
        assert_eq!(failed.failure_message.as_deref(), Some("boom"));
        assert!(failed
            .stack_trace
            .as_deref()
            .expect("stack")
            .contains("line2"));
    }

    #[test]
    fn rejects_empty_junit_file() {
        let err = parse(std::io::Cursor::new("")).expect_err("empty");
        assert!(matches!(err, JunitError::Empty | JunitError::Malformed(_)));
    }

    #[test]
    fn rejects_malformed_xml() {
        let err = parse(std::io::Cursor::new("<testsuite>")).expect_err("malformed");
        assert!(matches!(err, JunitError::Malformed(_)));
    }

    #[test]
    fn parses_self_closing_failure_and_error_nodes() {
        let xml = r#"
<testsuite name="suite">
  <testcase name="failed"><failure message="boom"/></testcase>
  <testcase name="errored"><error message="oops"/></testcase>
</testsuite>
"#;
        let report = parse(std::io::Cursor::new(xml)).expect("report");
        assert_eq!(report.summary.failed, 1);
        assert_eq!(report.summary.errors, 1);
    }
}
