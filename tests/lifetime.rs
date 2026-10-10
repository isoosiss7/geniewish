use geniewish::{
    router::{Command, route},
    state::{State, Update},
    storage::Store,
};

fn team() -> State {
    let mut state = State::default();
    state.hire("Mike", "HR manager").unwrap();
    state.hire("Annie", "Product manager").unwrap();
    state
}

#[test]
fn switching_and_restart_preserve_the_same_thread() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = team();
    let annie = state.named("Annie").unwrap().id;
    state.bind_thread(annie, "original-thread").unwrap();
    state.switch("Annie").unwrap();
    state.switch("Mike").unwrap();
    {
        let store = Store::open(directory.path()).unwrap();
        store.save(&state).unwrap();
    }
    let store = Store::open(directory.path()).unwrap();
    let mut resumed = store.load().unwrap();
    resumed.switch("Annie").unwrap();
    assert_eq!(
        resumed.current().unwrap().thread_id.as_deref(),
        Some("original-thread")
    );
    assert!(resumed.bind_thread(annie, "replacement-thread").is_err());
}

#[test]
fn retirement_reuses_a_name_with_a_fresh_identity() {
    let mut state = team();
    let old = state.named("Annie").unwrap().id;
    assert!(state.confirmed_retirement("Annie").is_err());
    state.request_retirement("Annie").unwrap();
    let confirmed = state.confirmed_retirement("Annie").unwrap();
    state.finish_retirement(confirmed).unwrap();
    let new = state.hire("Annie", "Designer").unwrap();
    assert_ne!(old, new);
    assert!(state.get(new).unwrap().thread_id.is_none());
    assert!(state.confirmed_retirement("Annie").is_err());
}

#[test]
fn mention_does_not_switch_but_an_address_routes_only_the_body() {
    assert_eq!(route("Explain Annie's role").unwrap().target, None);
    assert_eq!(route("Annie is my product manager").unwrap().target, None);
    let routed = route("Annie, build the landing page").unwrap();
    assert_eq!(routed.target.as_deref(), Some("Annie"));
    assert_eq!(
        routed.command,
        Command::Message("build the landing page".into())
    );
    assert!(route("Talk to Brian").is_err());
}

#[test]
fn damaged_state_is_not_silently_replaced() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("state.json"), b"{broken").unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert!(store.load().is_err());
    assert_eq!(
        std::fs::read(directory.path().join("state.json")).unwrap(),
        b"{broken"
    );
}

#[test]
fn two_processes_cannot_own_the_same_state() {
    let directory = tempfile::tempdir().unwrap();
    let _first = Store::open(directory.path()).unwrap();
    assert!(Store::open(directory.path()).is_err());
}

#[test]
fn background_updates_are_deduplicated_and_keep_the_foreground() {
    let mut state = team();
    let annie = state.named("Annie").unwrap().id;
    let mike = state.current().unwrap().id;
    state.bind_thread(annie, "annie-thread").unwrap();
    let update = Update {
        owner: annie,
        thread_id: "annie-thread".into(),
        turn_id: "turn-1".into(),
        item_id: "item-1".into(),
        read: false,
    };
    assert!(state.record_update(update.clone()).unwrap());
    assert!(!state.record_update(update).unwrap());
    assert_eq!(state.active, Some(mike));
    assert_eq!(state.unread().count(), 1);
    state.mark_read("annie-thread", "turn-1", "item-1").unwrap();
    assert_eq!(state.unread().count(), 0);
}

#[test]
fn threads_cannot_be_shared_between_identities() {
    let mut state = team();
    let annie = state.named("Annie").unwrap().id;
    let mike = state.current().unwrap().id;
    state.bind_thread(annie, "a-thread").unwrap();
    assert!(state.bind_thread(mike, "a-thread").is_err());
}
