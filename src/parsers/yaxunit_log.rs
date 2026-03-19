use regex::Regex;
use std::io::BufRead;
use std::path::Path;
use std::sync::OnceLock;

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

#[cfg(test)]
mod tests {
    use super::parse_reader;

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
}
