/// Parsed V8 connection and optional authentication parameters.
#[derive(Debug, Clone)]
pub struct V8Connection {
    raw: String,
    connection_args: Vec<String>,
    /// Optional username added as `/N <value>`.
    pub user: Option<String>,
    /// Optional password added as `/P <value>`.
    pub password: Option<String>,
    /// Optional infobase unlock code emitted as `/UC <value>`.
    ///
    /// Configurations protected by a locking code (`Конфигурация → Установить пароль`) refuse any
    /// administrative DESIGNER operation until the matching `/UC` is supplied; an empty string is
    /// treated as "no unlock code" for backwards compatibility with explicit nulling in overlays.
    pub unlock_code: Option<String>,
}

impl V8Connection {
    /// Build a reusable connection model from a raw connection string.
    pub fn from_connection_string(raw: &str) -> Self {
        let trimmed = raw.trim();
        let connection_args = if trimmed.starts_with('/') || trimmed.starts_with('-') {
            split_arg_string(trimmed)
        } else {
            vec!["/IBConnectionString".to_owned(), trimmed.to_owned()]
        };

        Self {
            raw: trimmed.to_owned(),
            connection_args,
            user: None,
            password: None,
            unlock_code: None,
        }
    }

    /// Build CLI arguments for a V8 utility launch.
    pub fn args(&self) -> Vec<String> {
        let mut args = self.connection_args.clone();
        if let Some(user) = &self.user {
            args.push("/N".to_owned());
            args.push(user.clone());
        }
        if let Some(password) = &self.password {
            if !password.is_empty() {
                args.push("/P".to_owned());
                args.push(password.clone());
            }
        }
        if let Some(unlock_code) = &self.unlock_code {
            // Empty value is intentionally treated as absent: a misconfigured overlay setting
            // `unlock_code: ""` should not push an `/UC` token without a value to the platform.
            if !unlock_code.is_empty() {
                args.push("/UC".to_owned());
                args.push(unlock_code.clone());
            }
        }
        args
    }

    /// Return the file-based infobase path when connection string contains `File=...`.
    pub fn file_path(&self) -> Option<&str> {
        if self.raw.starts_with('/') || self.raw.starts_with('-') {
            return file_path_from_args(&self.connection_args);
        }

        self.raw.split(';').find_map(|part| {
            let part = part.trim();
            let lower = part.to_lowercase();
            if lower.starts_with("file=") {
                Some(&part[5..])
            } else {
                None
            }
        })
    }

    /// Returns a stable file-based infobase connection string when available.
    pub fn create_infobase_arg(&self) -> Option<String> {
        self.file_path()
            .map(|path| format!("File='{}'", path.replace('\'', "''")))
    }
}

fn file_path_from_args(args: &[String]) -> Option<&str> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg.eq_ignore_ascii_case("/f") || arg.eq_ignore_ascii_case("-f") {
            return args.next().map(String::as_str);
        }
    }

    None
}

fn split_arg_string(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in raw.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ch if ch.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

#[cfg(test)]
mod tests {
    use super::V8Connection;

    #[test]
    fn wraps_plain_connection_string_as_flag_and_value() {
        let connection = V8Connection::from_connection_string("File=/tmp/ib");

        assert_eq!(
            connection.args(),
            vec!["/IBConnectionString", "File=/tmp/ib"]
        );
    }

    #[test]
    fn splits_raw_connection_and_auth_into_separate_tokens() {
        let mut connection = V8Connection::from_connection_string("/F \"/tmp/my ib\"");
        connection.user = Some("alice".to_owned());
        connection.password = Some("secret".to_owned());

        assert_eq!(
            connection.args(),
            vec!["/F", "/tmp/my ib", "/N", "alice", "/P", "secret"]
        );
    }

    #[test]
    fn extracts_file_path_from_connection_string() {
        let connection = V8Connection::from_connection_string("Srvr=demo;File=/tmp/ib;Ref=test");

        assert_eq!(connection.file_path(), Some("/tmp/ib"));
    }

    #[test]
    fn extracts_file_path_from_raw_f_args() {
        let connection = V8Connection::from_connection_string("/F \"/tmp/my ib\"");

        assert_eq!(connection.file_path(), Some("/tmp/my ib"));
    }

    #[test]
    fn extracts_file_path_from_dash_f_args() {
        let connection = V8Connection::from_connection_string("-F /tmp/ib");

        assert_eq!(connection.file_path(), Some("/tmp/ib"));
    }

    #[test]
    fn trims_leading_whitespace_before_parsing_raw_args() {
        let connection = V8Connection::from_connection_string("  /F /tmp/ib  ");

        assert_eq!(connection.args(), vec!["/F", "/tmp/ib"]);
        assert_eq!(connection.file_path(), Some("/tmp/ib"));
    }

    #[test]
    fn appends_unlock_code_after_credentials() {
        let mut connection = V8Connection::from_connection_string("File=/tmp/ib");
        connection.user = Some("Admin".to_owned());
        connection.password = Some("pw".to_owned());
        connection.unlock_code = Some("uc-secret".to_owned());

        assert_eq!(
            connection.args(),
            vec![
                "/IBConnectionString",
                "File=/tmp/ib",
                "/N",
                "Admin",
                "/P",
                "pw",
                "/UC",
                "uc-secret",
            ]
        );
    }

    #[test]
    fn omits_unlock_code_when_value_is_empty() {
        let mut connection = V8Connection::from_connection_string("File=/tmp/ib");
        connection.unlock_code = Some(String::new());

        assert!(!connection.args().iter().any(|arg| arg == "/UC"));
    }

    #[test]
    fn omits_unlock_code_when_not_configured() {
        let connection = V8Connection::from_connection_string("File=/tmp/ib");

        assert!(!connection.args().iter().any(|arg| arg == "/UC"));
    }
}
