use crate::domain::runner::LaunchOptions;

pub(crate) const VANESSA_TEST_MANAGER_ARG: &str = "/TESTMANAGER";

pub(crate) fn vanessa_enterprise_launch_keys(
    config_keys: &[String],
    launch: &LaunchOptions,
) -> Vec<String> {
    let mut keys = config_keys.to_vec();
    if has_launch_key(&launch.raw_args, VANESSA_TEST_MANAGER_ARG) {
        remove_launch_key(&mut keys, VANESSA_TEST_MANAGER_ARG);
    } else {
        ensure_launch_key(&mut keys, VANESSA_TEST_MANAGER_ARG);
    }
    keys
}

fn ensure_launch_key(keys: &mut Vec<String>, key: &str) {
    if has_launch_key(keys, key) {
        return;
    }
    keys.push(key.to_owned());
}

fn has_launch_key(keys: &[String], key: &str) -> bool {
    let expected = normalize_launch_key(key);
    keys.iter()
        .any(|existing| is_launch_key(existing) && normalize_launch_key(existing) == expected)
}

fn remove_launch_key(keys: &mut Vec<String>, key: &str) {
    let expected = normalize_launch_key(key);
    keys.retain(|existing| {
        !(is_launch_key(existing) && normalize_launch_key(existing) == expected)
    });
}

fn is_launch_key(key: &str) -> bool {
    let trimmed = key.trim_start();
    trimmed.starts_with('/') || trimmed.starts_with('-')
}

fn normalize_launch_key(key: &str) -> String {
    key.trim_start_matches(['/', '-'])
        .trim()
        .to_ascii_lowercase()
}
