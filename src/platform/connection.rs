use crate::support::connection_args::split_v8_arg_string;

/// Parsed V8 connection and optional authentication parameters.
#[derive(Debug, Clone)]
pub struct V8Connection {
    raw: String,
    connection_args: Vec<String>,
    /// Optional username added as `/N <value>`.
    pub user: Option<String>,
    /// Optional password added as `/P <value>`.
    pub password: Option<String>,
}

impl V8Connection {
    /// Build a reusable connection model from a raw connection string.
    pub fn from_connection_string(raw: &str) -> Self {
        let trimmed = raw.trim();
        let connection_args = if trimmed.starts_with('/') || trimmed.starts_with('-') {
            split_v8_arg_string(trimmed).0
        } else {
            vec!["/IBConnectionString".to_owned(), trimmed.to_owned()]
        };

        Self {
            raw: trimmed.to_owned(),
            connection_args,
            user: None,
            password: None,
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
}
