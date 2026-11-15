//! Environment variables that were renamed with the app: the new name first, and the old one still read.

use std::ffi::OsString;

/// The value of the first of `names` that is set and not empty, asking `lookup` (`std::env::var_os` in the programs).
/// An empty variable counts as unset, so `LOAH_X=` does not hide a `DICTATE_X` that someone set before the rename.
pub fn first_set(lookup: impl Fn(&str) -> Option<OsString>, names: &[&str]) -> Option<OsString> {
    names
        .iter()
        .find_map(|name| lookup(name).filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs = pairs.to_vec();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn the_first_name_that_is_set_wins() {
        let env = with(&[("OLD", "old"), ("NEW", "new")]);
        assert_eq!(first_set(env, &["NEW", "OLD"]), Some("new".into()));
    }

    #[test]
    fn a_later_name_is_read_when_the_first_is_unset() {
        let env = with(&[("OLD", "old")]);
        assert_eq!(first_set(env, &["NEW", "OLD"]), Some("old".into()));
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let env = with(&[("NEW", ""), ("OLD", "old")]);
        assert_eq!(first_set(env, &["NEW", "OLD"]), Some("old".into()));
        let env = with(&[("NEW", ""), ("OLD", "")]);
        assert_eq!(first_set(env, &["NEW", "OLD"]), None);
    }

    #[test]
    fn nothing_set_gives_nothing() {
        assert_eq!(first_set(with(&[]), &["NEW", "OLD"]), None);
    }
}
