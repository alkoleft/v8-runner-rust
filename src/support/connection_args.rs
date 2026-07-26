/// Split a raw V8 connection argument string using the same quote rules as process execution.
///
/// Backslashes are ordinary characters. The boolean reports whether all double quotes were
/// balanced so validation callers can reject malformed input without changing execution parsing.
pub(crate) fn split_v8_arg_string(raw: &str) -> (Vec<String>, bool) {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for character in raw.chars() {
        match character {
            '"' => in_quotes = !in_quotes,
            value if value.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            value => current.push(value),
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    (args, !in_quotes)
}
