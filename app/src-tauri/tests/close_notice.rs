#[path = "../src/lifecycle/close_notice.rs"]
mod close_notice;

use close_notice::{CloseAction, CloseNotice};

#[test]
fn first_close_waits_for_acknowledgement_and_deduplicates() {
    let notice = CloseNotice::new(None);
    assert_eq!(notice.admit(), CloseAction::Explain);
    assert_eq!(notice.admit(), CloseAction::Pending);
    notice.finish(false).unwrap();
    assert_eq!(notice.admit(), CloseAction::Explain);
    notice.finish(true).unwrap();
    assert_eq!(notice.admit(), CloseAction::Hide);
}

#[test]
fn acknowledgement_is_remembered_across_launches_in_the_same_profile() {
    let root = tempfile::tempdir().unwrap();
    let notice = CloseNotice::new(Some(root.path().to_owned()));
    assert_eq!(notice.admit(), CloseAction::Explain);
    assert_eq!(
        CloseNotice::new(Some(root.path().to_owned())).admit(),
        CloseAction::Explain
    );
    notice.finish(true).unwrap();
    assert_eq!(
        CloseNotice::new(Some(root.path().to_owned())).admit(),
        CloseAction::Hide
    );
}

#[test]
fn a_new_profile_still_gets_the_explanation() {
    let old = tempfile::tempdir().unwrap();
    CloseNotice::new(Some(old.path().to_owned()))
        .finish(true)
        .unwrap();
    let fresh = tempfile::tempdir().unwrap();
    assert_eq!(
        CloseNotice::new(Some(fresh.path().to_owned())).admit(),
        CloseAction::Explain
    );
}

#[test]
fn persistence_failure_does_not_repeat_the_notice_in_this_process() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("not-a-directory");
    std::fs::write(&file, b"existing bytes").unwrap();
    let notice = CloseNotice::new(Some(file.clone()));
    assert_eq!(notice.admit(), CloseAction::Explain);
    assert!(notice.finish(true).is_err());
    assert_eq!(notice.admit(), CloseAction::Hide);
    assert_eq!(std::fs::read(file).unwrap(), b"existing bytes");
}
