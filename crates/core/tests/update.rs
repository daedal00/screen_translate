use screen_translate_core::update::{Click, UpdateState};

#[test]
fn idle_offers_a_check() {
    let s = UpdateState::Idle;
    assert_eq!(s.label(), "Check for Updates…");
    assert!(s.enabled());
    assert_eq!(s.click(), Click::Check);
}

#[test]
fn available_offers_install_with_version() {
    let s = UpdateState::Available("0.3.1".into());
    assert_eq!(s.label(), "Install Update v0.3.1 and Restart");
    assert!(s.enabled());
    assert_eq!(s.click(), Click::Install);
}

#[test]
fn busy_states_are_disabled_and_ignore_clicks() {
    for s in [UpdateState::Checking, UpdateState::Installing] {
        assert!(!s.enabled(), "{s:?}");
        assert_eq!(s.click(), Click::Ignore, "{s:?}");
    }
    assert_eq!(UpdateState::Checking.label(), "Checking for Updates…");
    assert_eq!(UpdateState::Installing.label(), "Installing Update…");
}

#[test]
fn up_to_date_and_failed_allow_a_recheck() {
    let up = UpdateState::UpToDate;
    assert_eq!(up.label(), "Up to Date, Check Again");
    assert_eq!(up.click(), Click::Check);
    let failed = UpdateState::Failed;
    assert_eq!(failed.label(), "Update Failed, Try Again");
    assert_eq!(failed.click(), Click::Check);
}
