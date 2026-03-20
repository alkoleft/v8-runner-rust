Test fixtures used by parser and selected integration tests.

- `parsers/designer_validation.log` maps to `src/parsers/designer_validation.rs`
- `parsers/edt_validation.log` maps to `src/parsers/edt_validation.rs`
- `parsers/junit_report.xml` maps to `src/parsers/junit.rs`
- `parsers/junit_smoke_report.xml` maps to `tests/cli_test.rs`
- `parsers/yaxunit.log` maps to `src/parsers/yaxunit_log.rs` and `tests/cli_test.rs`

All fixtures are UTF-8 and are intended to hold larger realistic samples, while tiny edge cases stay inline in the individual tests.
