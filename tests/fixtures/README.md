Test fixtures used by parser and selected integration tests.

- `parsers/designer_validation.log` maps to `src/parsers/designer_validation.rs`
- `parsers/edt_validation.log` maps to `src/parsers/edt_validation.rs`
- `parsers/junit_report.xml` maps to `src/parsers/junit.rs`
- `parsers/junit_smoke_report.xml` maps to `tests/cli_test.rs`
- `parsers/yaxunit.log` maps to `src/parsers/yaxunit_log.rs` and `tests/cli_test.rs`
- `edt/` stores a deterministic native EDT sample used by `config init` regressions; it mirrors the local Designer fixtures after `DESIGNER -> EDT` convert and keeps only the stable `.project` / `DT-INF/PROJECT.PMF` / `src/**/*.{mdo,bsl,form}` markers needed for tests.

All fixtures are UTF-8 and are intended to hold larger realistic samples, while tiny edge cases stay inline in the individual tests.
