//! What the main window shows of the saved areas: the notices of the
//! store as toasts.

use gettextrs::gettext;

use crate::{
    areas::{AreaStore, file::LoadError},
    i18n::gettext_f,
};

/// The toast for an areas file that couldn't be loaded.
pub fn notice_for_load_error(err: &LoadError) -> String {
    match err {
        LoadError::Broken { moved_to } => gettext_f(
            "The list of areas was unreadable; it was moved to {path}",
            &[("path", &moved_to.display().to_string())],
        ),
        LoadError::Io(err) => gettext_f(
            "The list of areas was unreadable; changes to it won’t be saved: {error}",
            &[("error", &err.to_string())],
        ),
        LoadError::UnsupportedVersion(_) => {
            gettext("The list of areas is from a newer Kooha; changes to it won’t be saved")
        }
    }
}

/// The toast for why the store started without the saved list, once: the
/// next call returns `None`.
pub fn notice_for_store(store: &AreaStore) -> Option<String> {
    store.load_notice().map(|err| notice_for_load_error(&err))
}

#[cfg(test)]
mod tests {
    use std::{fs, io};

    use super::*;
    use crate::areas::file::test_utils::TempDir;

    #[test]
    fn broken_file_is_noticed_once_with_where_it_went() {
        let dir = TempDir::new();
        let path = dir.file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();

        let store = AreaStore::with_path(path);

        let notice = notice_for_store(&store).expect("a broken file is noticed");
        assert!(
            notice.starts_with("The list of areas was unreadable; it was moved to "),
            "{notice}"
        );
        assert!(notice.contains("rectangles.json.broken-"), "{notice}");
        assert_eq!(notice_for_store(&store), None);
    }

    #[test]
    fn readable_file_has_no_notice() {
        let dir = TempDir::new();
        let store = AreaStore::with_path(dir.file());

        assert_eq!(notice_for_store(&store), None);
    }

    #[test]
    fn newer_and_unmovable_files_say_changes_are_not_saved() {
        assert_eq!(
            notice_for_load_error(&LoadError::UnsupportedVersion(2)),
            "The list of areas is from a newer Kooha; changes to it won’t be saved"
        );
        assert_eq!(
            notice_for_load_error(&LoadError::Io(io::Error::other("denied"))),
            "The list of areas was unreadable; changes to it won’t be saved: denied"
        );
    }
}
