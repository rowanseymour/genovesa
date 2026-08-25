//! The menus, driven the way a player drives them: by clicking the buttons
//! and pressing the keys, and asking what the screen then says.
//!
//! One module rather than one per screen, because nearly every test here is
//! built on the same handful of helpers — an app in a state, a click, a key,
//! and a reading of what is on screen. Splitting them would mean a harness
//! module for the helpers and eight files that all import it, which is more
//! moving parts than the thing it would tidy.

use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::input::ButtonState;
use bevy::state::app::StatesPlugin;

use super::controls::KeyText;
use super::display::{DisplayText, Dropped};
use super::join::MAX_ADDRESS;
use super::kit::ON_PAPER;
use super::kit::{ScrollingPanel, SCROLL_NOTCH};
use super::new_world::MAX_SEED_DIGITS;
use super::set_sail::MOST_KEPT_WORLDS;
use super::*;
use crate::net;
use crate::net::fake_server;
use crate::testing::run_until;
use server::random_seed;
use server::KeptWorld;
use std::time::SystemTime;

/// A host that accepts a connection and then says nothing — a dial that
/// stays in the air for as long as the test needs it to. Its listener is
/// handed back so the test decides when it stops existing.
fn silent_server() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    (listener, address)
}

/// A headless app running the menu systems, with no renderer attached.
fn test_app(state: AppState) -> App {
    // The Start button keeps the world it opens, and a test's world must
    // not land among the player's real ones.
    crate::testing::quarantine_data_dir();
    let mut app = App::new();
    app.add_plugins((StatesPlugin, MenuPlugin))
        .insert_state(state)
        .add_sub_state::<Helm>()
        .init_resource::<ButtonInput<KeyCode>>()
        // Normally the input plugin's, written from the device each frame.
        .init_resource::<AccumulatedMouseScroll>()
        // Normally the camera plugin's, but entering a world moves the
        // view onto the served spawn — see `settle_dialing`.
        .init_resource::<View>()
        // Nothing advances it in a headless app, which is what lets a
        // test run a trial's clock out on its own terms — see
        // [`run_out_the_trial`].
        .init_resource::<Time>()
        .add_message::<AppExit>()
        .add_message::<KeyboardInput>();
    app.update();
    app
}

/// A keypress as the controls screen reads it: a real one carries both the
/// position pressed and what that position typed, and the screen wants
/// each for a different purpose.
fn a_press(key: KeyCode, typed: &str) -> KeyboardInput {
    KeyboardInput {
        key_code: key,
        logical_key: Key::Character(typed.into()),
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window: Entity::PLACEHOLDER,
    }
}

/// Sends one, down the channel [`settings_keys`] reads.
fn type_key(app: &mut App, key: KeyCode, typed: &str) {
    app.world_mut().write_message(a_press(key, typed));
    app.update();
}

fn bindings(app: &App) -> &KeyBindings {
    app.world().resource::<KeyBindings>()
}

fn waiting_on(app: &App) -> Option<Action> {
    app.world().resource::<Rebinding>().0
}

/// Moves between screens the way clicking would, so `OnEnter` and `OnExit`
/// run.
fn go_to(app: &mut App, state: AppState) {
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(state);
    app.update();
}

/// Simulates a click by spawning a pressed button; `Changed<Interaction>`
/// fires for a freshly inserted component.
///
/// Taken away again before the frame the transition lands on, and that
/// matters rather than being tidiness. A button left lying in the world
/// still reads as *freshly changed* to any system that has not run since it
/// was spawned — and a click that changes screens is exactly that, the
/// arriving screen's systems having sat out every frame until now. A Back
/// pressed on one screen would be read a second time by the screen it
/// returned to, which then went back again. Real buttons cannot do it: a
/// screen takes its own down on the way out, `DespawnOnExit`, during the
/// very transition this frame is here to let land. This is what gives the
/// stand-ins the same manners.
fn click(app: &mut App, button: MenuButton) {
    let pressed = click_once(app, button);
    app.world_mut().entity_mut(pressed).despawn();
    app.update();
}

/// A click and the one frame it takes to be seen, with none of the frames
/// in which something the click started could land. What the button *did*
/// is visible; what may come of it later is not. The button is handed back
/// so a caller that runs on can clear it up — see [`click`].
fn click_once(app: &mut App, button: MenuButton) -> Entity {
    let pressed = app.world_mut().spawn((button, Interaction::Pressed)).id();
    app.update();
    pressed
}

/// Taps a key for exactly one frame, down the channel [`options_keys`]
/// reads. Releasing afterwards matters: a key still held down never counts
/// as just-pressed again.
fn press_key(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    app.update();

    let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    input.release(key);
    input.clear();
}

/// One key, hit the way a keyboard hits it: both channels at once. A real
/// press is a message *and* a button held down for a frame, and the menus
/// read it both ways — [`options_keys`] off the button, [`settings_keys`]
/// off the message. A test that writes only the channel the system it is
/// about happens to read can never catch the two of them answering the
/// same press.
fn hit_key(app: &mut App, key: KeyCode, typed: &str) {
    app.world_mut().write_message(a_press(key, typed));
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    app.update();

    let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    input.release(key);
    input.clear();
}

fn state(app: &App) -> AppState {
    *app.world().resource::<State<AppState>>().get()
}

/// What the player is doing in the world, or `None` when there is no world
/// to be doing it in — which is itself worth asserting, since a pause menu
/// that outlived its world would be the bug this all exists to prevent.
fn helm(app: &App) -> Option<Helm> {
    app.world().get_resource::<State<Helm>>().map(|h| *h.get())
}

/// A match with the pause menu already up.
fn paused_app() -> App {
    let mut app = test_app(AppState::InWorld);
    app.world_mut()
        .resource_mut::<NextState<Helm>>()
        .set(Helm::Paused);
    app.update();
    app
}

/// A world actually being served, so that a test of what the pause menu
/// says about hosting is looking at a real [`Hosting`]. Cheap: the port is
/// the kernel's to pick and nothing ever dials it.
///
/// `bind` is what decides whether the world counts as shared — the
/// loopback is a world of one's own, anything wider is one others could be
/// in. An ephemeral port either way, so two test runs cannot collide the
/// way binding the real shared port would.
fn fake_host(bind: &str) -> server::Host {
    server::Server::bind(bind, 7)
        .expect("bind")
        .spawn()
        .expect("spawn")
}

/// How many screens of a given name are standing.
fn named(app: &mut App, name: &str) -> usize {
    app.world_mut()
        .query::<&Name>()
        .iter(app.world())
        .filter(|n| n.as_str() == name)
        .count()
}

/// Everything the screen currently says, run together.
fn screen_text(app: &mut App) -> String {
    app.world_mut()
        .query::<&Text>()
        .iter(app.world())
        .map(|t| t.0.clone())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A wheel notch, the way the input plugin would report it: accumulated
/// for a frame, then gone.
fn roll_wheel(app: &mut App, notches: f32) {
    app.world_mut().insert_resource(AccumulatedMouseScroll {
        unit: MouseScrollUnit::Line,
        delta: Vec2::new(0.0, notches),
    });
    app.update();
    app.world_mut()
        .insert_resource(AccumulatedMouseScroll::default());
}

/// How far down the controls panel the screen has been rolled.
fn scrolled(app: &mut App) -> f32 {
    app.world_mut()
        .query_filtered::<&ScrollPosition, With<ScrollingPanel>>()
        .single(app.world())
        .expect("the controls panel scrolls")
        .0
        .y
}

/// Says how much list there is, the way the layout would have written it:
/// a panel `shown` screen pixels tall with `stacked` of them of rows in
/// it, laid out at `to_a_pixel` screen pixels to each of the UI's own.
fn a_list_of(app: &mut App, shown: f32, stacked: f32, to_a_pixel: f32) {
    let panel = app
        .world_mut()
        .query_filtered::<Entity, With<ScrollingPanel>>()
        .single(app.world())
        .expect("the controls panel scrolls");
    let mut panel = app.world_mut().entity_mut(panel);
    let mut measured = panel
        .get_mut::<ComputedNode>()
        .expect("every node is measured");
    measured.size.y = shown;
    measured.content_size.y = stacked;
    measured.inverse_scale_factor = 1.0 / to_a_pixel;
}

/// The controls screen's height follows from how many actions there are,
/// so it is the one screen that can be taller than a short window even at
/// the UI's smallest — and the wheel is what reaches the rest of it.
#[test]
fn the_controls_screen_scrolls_and_stops_at_both_ends() {
    let mut app = test_app(AppState::Controls);
    assert_eq!(scrolled(&mut app), 0.0, "the screen opened mid-list");

    let over = 2.0 * SCROLL_NOTCH;
    a_list_of(&mut app, 400.0, 400.0 + over, 1.0);

    roll_wheel(&mut app, -1.0);
    assert_eq!(scrolled(&mut app), SCROLL_NOTCH);

    // Both ends are this system's own. The layout clamps only the copy it
    // draws from, so a list rolled past its last row here would pile up
    // travel that had to be wound back through before anything moved —
    // and rolled all the way back and further, the list stands at its
    // start rather than being pulled past it.
    roll_wheel(&mut app, -5.0);
    assert_eq!(
        scrolled(&mut app),
        over,
        "the list rolled past its last row"
    );

    roll_wheel(&mut app, 5.0);
    assert_eq!(scrolled(&mut app), 0.0);
}

/// A wheel notch is a laid-out measurement — about a key row — and a
/// trackpad's pixels are the screen's, so the two cannot both be written
/// straight into a scroll position. A finger that dragged the list by an
/// inch has to move it by an inch of list.
#[test]
fn a_trackpad_moves_the_list_by_what_the_finger_moved() {
    let mut app = test_app(AppState::Controls);
    // A dense display with the UI drawn small on it: four screen pixels to
    // each pixel the list is laid out in.
    a_list_of(&mut app, 1000.0, 9000.0, 4.0);

    app.world_mut().insert_resource(AccumulatedMouseScroll {
        unit: MouseScrollUnit::Pixel,
        delta: Vec2::new(0.0, -40.0),
    });
    app.update();
    app.world_mut()
        .insert_resource(AccumulatedMouseScroll::default());

    // Forty screen pixels of finger, and forty screen pixels of list gone
    // under the top of the panel — which is ten of the pixels the list is
    // written in.
    assert_eq!(scrolled(&mut app), 10.0);
}

#[test]
fn setting_sail_opens_the_worlds() {
    let mut app = test_app(AppState::MainMenu);
    click(&mut app, MenuButton::SetSail);
    assert_eq!(state(&app), AppState::SetSail);
}

#[test]
fn new_world_opens_the_setup_dialog() {
    // From the worlds screen, which is where the choice between returning
    // to one and starting one is made. An empty harbour, so this is about
    // the press and not about the cap.
    let mut app = harbour_of(Vec::new());
    click(&mut app, MenuButton::NewWorld);
    assert_eq!(state(&app), AppState::NewWorld);
}

#[test]
fn exit_requests_shutdown() {
    let mut app = test_app(AppState::MainMenu);
    click(&mut app, MenuButton::Exit);

    let exits = app.world().resource::<Messages<AppExit>>();
    assert!(!exits.is_empty(), "no AppExit was sent");
}

/// One step back, not all the way out: the dialog is opened from the
/// worlds screen, so that is where Back belongs.
#[test]
fn back_out_of_the_new_world_dialog_returns_to_the_worlds() {
    let mut app = test_app(AppState::NewWorld);
    click(&mut app, MenuButton::Back);
    assert_eq!(state(&app), AppState::SetSail);
}

#[test]
fn back_out_of_the_worlds_returns_to_the_main_menu() {
    let mut app = test_app(AppState::SetSail);
    click(&mut app, MenuButton::Back);
    assert_eq!(state(&app), AppState::MainMenu);
}

/// A world for the screen to offer, with a real file behind it so that
/// discarding one has something to delete. Not a world anything could be
/// sailed in — what a discard needs is a name to take the lock on and
/// files to remove, and it never reads a byte of what is in them.
fn a_kept_world(id: u64) -> KeptWorld {
    // Before asking, not after: every caller builds its worlds before it
    // builds the app that would otherwise have done this, so a world put
    // together here is the first thing in the test to want a directory to
    // put it in.
    crate::testing::quarantine_data_dir();
    let dir = net::worlds_dir().expect("the quarantined data dir");
    std::fs::create_dir_all(&dir).expect("the worlds directory");
    let world = KeptWorld {
        path: dir.join(format!("{}.world", protocol::WorldId(id))),
        id: protocol::WorldId(id),
        name: format!("Test Water {id:x}"),
        age: 0.0,
        kept: SystemTime::now(),
    };
    std::fs::write(&world.path, "a world, as far as this test is concerned").expect("write");
    world
}

/// The worlds screen offering exactly the worlds a test names, rather than
/// whatever this machine happens to keep. Set after entering the screen,
/// which is what reads the directory — and a changed harbour is what
/// builds the screen again, so the list on screen is this one.
fn harbour_of(worlds: Vec<KeptWorld>) -> App {
    let mut app = test_app(AppState::SetSail);
    app.world_mut().resource_mut::<Harbour>().worlds = worlds;
    app.update();
    app
}

fn asked(app: &App) -> Option<usize> {
    app.world().resource::<Harbour>().asked
}

#[test]
fn a_kept_world_is_offered_with_a_way_to_throw_it_away() {
    let mut app = harbour_of(vec![a_kept_world(0x51)]);
    let text = screen_text(&mut app);
    assert!(
        text.contains("Test Water 51"),
        "the world is not offered: {text}"
    );
    assert!(text.contains("Discard"), "no way to throw it away: {text}");
}

#[test]
fn discarding_asks_first_and_takes_no_for_an_answer() {
    let world = a_kept_world(0x52);
    let path = world.path.clone();
    let mut app = harbour_of(vec![world]);

    click(&mut app, MenuButton::AskDiscard(0));
    assert_eq!(asked(&app), Some(0));
    assert!(
        screen_text(&mut app).contains("for good?"),
        "the row asked nothing"
    );
    assert!(path.exists(), "the world went before anybody said yes");

    click(&mut app, MenuButton::KeepIt);
    assert_eq!(asked(&app), None);
    assert!(path.exists(), "the world went on a no");
    assert_eq!(app.world().resource::<Harbour>().worlds.len(), 1);
    assert!(
        !screen_text(&mut app).contains("for good?"),
        "the question is still standing"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn yes_throws_the_world_away() {
    let world = a_kept_world(0x53);
    let path = world.path.clone();
    let mut app = harbour_of(vec![world]);

    click(&mut app, MenuButton::AskDiscard(0));
    click(&mut app, MenuButton::Discard(0));

    assert!(!path.exists(), "the world's file outlived the discard");
    assert!(
        app.world().resource::<Harbour>().worlds.is_empty(),
        "the row outlived the world"
    );
    assert_eq!(asked(&app), None);
}

/// Leaving the screen answers the question the safe way — and coming back
/// must not find it still standing over a world that was never chosen.
#[test]
fn a_question_walked_away_from_is_not_a_yes() {
    let world = a_kept_world(0x54);
    let path = world.path.clone();
    let mut app = harbour_of(vec![world]);

    click(&mut app, MenuButton::AskDiscard(0));
    click(&mut app, MenuButton::Back);
    assert_eq!(state(&app), AppState::MainMenu);

    go_to(&mut app, AppState::SetSail);
    assert_eq!(asked(&app), None, "the screen came back still asking");
    assert!(path.exists(), "the world went while nobody was looking");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_empty_harbour_says_so() {
    let mut app = harbour_of(Vec::new());
    assert!(screen_text(&mut app).contains("no world sailed from here yet"));
}

/// The cap is a closed door, not a hidden row: a full machine says so and
/// refuses the press, and every world it is keeping is still on the screen
/// to be returned to or thrown away.
#[test]
fn a_full_harbour_refuses_another_world() {
    let worlds: Vec<KeptWorld> = (0..MOST_KEPT_WORLDS)
        .map(|n| a_kept_world(0x60 + n as u64))
        .collect();
    let paths: Vec<_> = worlds.iter().map(|world| world.path.clone()).collect();
    let mut app = harbour_of(worlds);

    click(&mut app, MenuButton::NewWorld);
    assert_eq!(
        state(&app),
        AppState::SetSail,
        "the cap let a sixth world by"
    );
    assert!(
        app.world().resource::<Status>().0.contains("discard"),
        "the press was refused without saying why: {:?}",
        app.world().resource::<Status>().0
    );

    let text = screen_text(&mut app);
    assert!(
        text.contains("no room for another world"),
        "the screen does not say it is full: {text}"
    );
    for (n, _) in paths.iter().enumerate() {
        assert!(
            text.contains(&format!("Test Water {:x}", 0x60 + n)),
            "world {n} of a full harbour is not on the screen"
        );
    }

    // And room made is a way through: the same press, one discard later.
    click(&mut app, MenuButton::AskDiscard(0));
    click(&mut app, MenuButton::Discard(0));
    click(&mut app, MenuButton::NewWorld);
    assert_eq!(state(&app), AppState::NewWorld);

    for path in &paths {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn entering_a_served_world_afoot_stands_the_player_on_the_spawn() {
    // The path a player actually takes into a world: the welcome moves
    // the view onto the served spawn *and then* enters. This fake
    // server seats nobody at any helm and tells of no boats, so what
    // entry owes is a walker standing exactly where the server said —
    // the hulls are the server's to tell, not entry's to invent.
    let (address, _socket) = fake_server(Vec2::new(100.0, -200.0), Vec2::new(100.0, -400.0));
    let mut app = test_app(AppState::JoinWorld);
    // Time for the reporting the net plugin brings with it; the menu's
    // own systems never ask what o'clock it is. Assets because a boat
    // telling would arrive as meshes.
    app.add_plugins((
        TaskPoolPlugin::default(),
        AssetPlugin::default(),
        bevy::time::TimePlugin,
        crate::net::NetPlugin,
    ))
    .init_asset::<Mesh>()
    .init_resource::<Assets<StandardMaterial>>();

    app.world_mut().resource_mut::<JoinSettings>().address = address;
    click(&mut app, MenuButton::Connect);
    run_until(&mut app, "the world is entered", |app| {
        *app.world().resource::<State<AppState>>().get() == AppState::InWorld
    });
    app.update();

    let at = app
        .world_mut()
        .query_filtered::<&Transform, With<crate::player::Player>>()
        .single(app.world())
        .expect("entering a served world afoot should stand a walker up")
        .translation;
    assert_eq!(Vec2::new(at.x, at.z), Vec2::new(100.0, -200.0));
}

#[test]
fn join_world_opens_the_join_screen() {
    let mut app = test_app(AppState::MainMenu);
    click(&mut app, MenuButton::JoinWorld);
    assert_eq!(state(&app), AppState::JoinWorld);
}

#[test]
fn sharing_is_off_until_it_is_asked_for() {
    let mut app = test_app(AppState::NewWorld);
    assert!(!app.world().resource::<NewWorldSettings>().share);

    click(&mut app, MenuButton::ToggleShare);
    assert!(app.world().resource::<NewWorldSettings>().share);
    click(&mut app, MenuButton::ToggleShare);
    assert!(!app.world().resource::<NewWorldSettings>().share);
}

#[test]
fn even_a_world_of_ones_own_is_served() {
    // The ground comes from a server, so there is no such thing here as a
    // world without one — the sharing switch decides who can reach it and
    // nothing else. Keeping a world therefore starts a server and waits
    // for it, exactly as sharing one does.
    let mut app = test_app(AppState::NewWorld);
    assert!(
        !app.world().resource::<NewWorldSettings>().share,
        "this test is about the switch being off"
    );
    click_once(&mut app, MenuButton::Start);

    assert_eq!(state(&app), AppState::NewWorld, "entered without a world");
    assert!(
        app.world().contains_resource::<Dialing>(),
        "keeping a world started no server"
    );
}

#[test]
fn a_shared_world_waits_on_the_server_it_starts() {
    // Started, not entered: the player stays on the dialog until the
    // welcome comes back — which is where `settle_dialing` takes over,
    // tested below against a server this test file can name.
    //
    // Looked at after a single frame, before anything can have come of the
    // dial. What the well-known port does when it is asked for is not this
    // test's business — and on the machine a test runs on it may well
    // already be somebody's world.
    let mut app = test_app(AppState::NewWorld);
    click(&mut app, MenuButton::ToggleShare);
    click_once(&mut app, MenuButton::Start);

    assert_eq!(state(&app), AppState::NewWorld);
    assert!(
        app.world().contains_resource::<Dialing>(),
        "sharing a world started no server"
    );
}

#[test]
fn a_dial_that_lands_enters_the_served_world() {
    let (address, _socket) = fake_server(Vec2::new(100.0, -200.0), Vec2::new(100.0, -400.0));
    let mut app = test_app(AppState::JoinWorld);
    // Somewhere the menu's own drifting sea might have left the view. A
    // match must open where the server said, not where the menu was
    // looking.
    app.world_mut().resource_mut::<View>().focus = Vec3::new(4_000.0, 0.0, -2_500.0);
    app.world_mut().resource_mut::<JoinSettings>().address = address;
    click(&mut app, MenuButton::Connect);

    run_until(&mut app, "the world is entered", |app| {
        *app.world().resource::<State<AppState>>().get() == AppState::InWorld
    });

    // Where we are standing in the world is the server's to say, and so is
    // which way to look: the view opens with the first land dead ahead
    // rather than wherever the bearing happened to be.
    let view = *app.world().resource::<View>();
    assert_eq!(view.focus, Vec3::new(100.0, 0.0, -200.0));
    let ahead = Vec2::new(-view.yaw.sin(), -view.yaw.cos());
    assert!(
        ahead.dot(Vec2::new(0.0, -1.0)) > 0.999,
        "the match opens looking {ahead}, not at the land it was pointed at"
    );
    assert!(app.world().contains_resource::<Online>());
    assert!(
        !app.world().contains_resource::<Dialing>(),
        "a dial that landed is still on the app"
    );
}

#[test]
fn a_dial_that_fails_says_so_and_stays_put() {
    let mut app = test_app(AppState::JoinWorld);
    // Port 1, where nothing has ever listened.
    app.world_mut().resource_mut::<JoinSettings>().address = "127.0.0.1:1".to_string();
    click(&mut app, MenuButton::Connect);

    run_until(&mut app, "the failure is reported", |app| {
        !app.world().contains_resource::<Dialing>()
    });
    assert_eq!(state(&app), AppState::JoinWorld);
    assert!(
        app.world().resource::<Status>().0.contains("127.0.0.1:1"),
        "the screen does not say what went wrong: {:?}",
        app.world().resource::<Status>().0
    );
}

#[test]
fn leaving_a_screen_abandons_the_dial_it_started() {
    // Otherwise a server started here, and given up on, would go on
    // holding the port for the life of the process — and a late welcome
    // would drag the player into a world they had walked away from.
    //
    // Dialled at a host that accepts and then says nothing, so the dial is
    // still in the air when the player gives up on it: one that had
    // already landed would be testing a different moment.
    let (silent, address) = silent_server();
    let mut app = test_app(AppState::JoinWorld);
    app.world_mut().resource_mut::<JoinSettings>().address = address;
    click(&mut app, MenuButton::Connect);
    assert!(app.world().contains_resource::<Dialing>());

    click(&mut app, MenuButton::Back);
    assert_eq!(state(&app), AppState::MainMenu);
    assert!(!app.world().contains_resource::<Dialing>());

    // And it stays abandoned, however the far end comes to life.
    drop(silent);
    for _ in 0..20 {
        app.update();
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(state(&app), AppState::MainMenu);
    assert!(!app.world().contains_resource::<Online>());
}

#[test]
fn typing_edits_the_address() {
    let mut app = test_app(AppState::JoinWorld);
    app.world_mut().resource_mut::<JoinSettings>().address = String::new();

    for (key, typed) in [
        (KeyCode::KeyA, "a"),
        (KeyCode::Period, "."),
        (KeyCode::Digit1, "1"),
        (KeyCode::Semicolon, ":"),
        (KeyCode::Digit8, "8"),
    ] {
        type_key(&mut app, key, typed);
    }
    assert_eq!(app.world().resource::<JoinSettings>().address, "a.1:8");

    type_key(&mut app, KeyCode::Backspace, "\u{8}");
    assert_eq!(app.world().resource::<JoinSettings>().address, "a.1:");
}

#[test]
fn the_address_field_takes_only_what_could_be_an_address() {
    let mut app = test_app(AppState::JoinWorld);
    app.world_mut().resource_mut::<JoinSettings>().address = String::new();

    // A space, and a character no host name has ever contained.
    type_key(&mut app, KeyCode::Space, " ");
    type_key(&mut app, KeyCode::Slash, "/");
    assert_eq!(app.world().resource::<JoinSettings>().address, "");

    for _ in 0..MAX_ADDRESS + 5 {
        type_key(&mut app, KeyCode::KeyX, "x");
    }
    assert_eq!(
        app.world().resource::<JoinSettings>().address.len(),
        MAX_ADDRESS
    );
}

#[test]
fn enter_joins_and_escape_leaves_the_join_screen() {
    let mut app = test_app(AppState::JoinWorld);
    app.world_mut().resource_mut::<JoinSettings>().address = "127.0.0.1:1".to_string();

    type_key(&mut app, KeyCode::Enter, "\r");
    assert!(
        app.world().contains_resource::<Dialing>(),
        "enter did not join"
    );

    let mut app = test_app(AppState::JoinWorld);
    type_key(&mut app, KeyCode::Escape, "\u{1b}");
    app.update();
    assert_eq!(state(&app), AppState::MainMenu);
}

#[test]
fn keys_pressed_before_the_join_screen_opened_are_not_typed_into_it() {
    // The same backlog the controls screen has to ignore: escape leaves a
    // match, and whatever else was pressed on the way to the menu must not
    // land in the address.
    let mut app = test_app(AppState::InWorld);
    type_key(&mut app, KeyCode::KeyJ, "j");

    go_to(&mut app, AppState::JoinWorld);
    assert_eq!(
        app.world().resource::<JoinSettings>().address,
        JoinSettings::default().address
    );
}

#[test]
fn typing_edits_the_seed() {
    let mut app = test_app(AppState::NewWorld);
    app.world_mut().resource_mut::<NewWorldSettings>().seed = String::new();

    type_key(&mut app, KeyCode::Digit4, "4");
    // The number pad types the same digit from a different position,
    // which is the whole reason the field reads what was typed.
    type_key(&mut app, KeyCode::Numpad2, "2");
    assert_eq!(app.world().resource::<NewWorldSettings>().seed, "42");

    type_key(&mut app, KeyCode::Backspace, "\u{8}");
    assert_eq!(app.world().resource::<NewWorldSettings>().seed, "4");
}

#[test]
fn the_seed_field_takes_only_digits() {
    let mut app = test_app(AppState::NewWorld);
    app.world_mut().resource_mut::<NewWorldSettings>().seed = String::new();

    // A letter, a symbol and a space: a seed is a number, and anything
    // else would only fail to parse back out of the field.
    type_key(&mut app, KeyCode::KeyA, "a");
    type_key(&mut app, KeyCode::Period, ".");
    type_key(&mut app, KeyCode::Space, " ");
    assert_eq!(app.world().resource::<NewWorldSettings>().seed, "");

    type_key(&mut app, KeyCode::Digit7, "7");
    assert_eq!(app.world().resource::<NewWorldSettings>().seed, "7");
}

#[test]
fn keys_pressed_before_the_new_world_dialog_opened_are_not_typed_into_it() {
    // The same backlog the join and controls screens have to ignore: the
    // reader runs on every screen so its cursor keeps up, which means it
    // has to refuse everything pressed before this screen was the one on
    // it.
    // Held against the seed the dialog already had rather than against a
    // fresh default, which would be a different world every time it was
    // asked for — that being the point of `NewWorldSettings::default`.
    let mut app = test_app(AppState::InWorld);
    let before = app.world().resource::<NewWorldSettings>().seed.clone();
    type_key(&mut app, KeyCode::Digit9, "9");

    go_to(&mut app, AppState::NewWorld);
    assert_eq!(
        app.world().resource::<NewWorldSettings>().seed,
        before,
        "a digit pressed on the way here landed in the seed"
    );
}

#[test]
fn seed_field_is_length_capped() {
    let mut app = test_app(AppState::NewWorld);
    app.world_mut().resource_mut::<NewWorldSettings>().seed = String::new();

    for _ in 0..MAX_SEED_DIGITS + 5 {
        type_key(&mut app, KeyCode::Digit9, "9");
    }

    let seed = &app.world().resource::<NewWorldSettings>().seed;
    assert_eq!(seed.len(), MAX_SEED_DIGITS);
    // Whatever the player types has to survive the trip into a u32.
    assert!(seed.parse::<u32>().is_ok(), "{seed} does not fit a u32");
}

/// The whole ladder from the front of the game, and back down it again.
/// Each rung is a screen somebody has to be able to leave the way they
/// arrived, and Back on the bottom two means Options rather than the top.
#[test]
fn the_options_ladder_goes_up_from_the_main_menu_and_back_down() {
    let mut app = test_app(AppState::MainMenu);
    click(&mut app, MenuButton::Options);
    assert_eq!(state(&app), AppState::Options);

    click(&mut app, MenuButton::Display);
    assert_eq!(state(&app), AppState::Display);
    click(&mut app, MenuButton::Back);
    assert_eq!(state(&app), AppState::Options);

    click(&mut app, MenuButton::Controls);
    assert_eq!(state(&app), AppState::Controls);
    click(&mut app, MenuButton::Back);
    assert_eq!(state(&app), AppState::Options);

    click(&mut app, MenuButton::Back);
    assert_eq!(state(&app), AppState::MainMenu);
}

/// And Escape is Back on every rung of it, stopping at the menu it started
/// from rather than carrying on into whatever is behind that.
#[test]
fn escape_walks_back_down_the_options_ladder_one_rung_at_a_time() {
    let mut app = test_app(AppState::MainMenu);
    click(&mut app, MenuButton::Options);
    click(&mut app, MenuButton::Display);
    assert_eq!(state(&app), AppState::Display);

    for expected in [AppState::Options, AppState::MainMenu] {
        press_key(&mut app, KeyCode::Escape);
        // The frame the transition lands on.
        app.update();
        assert_eq!(state(&app), expected);
    }
    // A press with nowhere left to go leaves the menu where it is.
    press_key(&mut app, KeyCode::Escape);
    app.update();
    assert_eq!(state(&app), AppState::MainMenu);
}

/// The one key two systems both hear, on the one screen where they would
/// disagree about it. Escape arrives as a message and as a held button at
/// once, and on the controls screen [`settings_keys`] must be the only one
/// to act on it: with a row armed it means "not that key" and the screen
/// stays put, and with none armed it is one rung down rather than two.
#[test]
fn one_escape_on_the_controls_screen_is_answered_once() {
    let mut app = test_app(AppState::MainMenu);
    click(&mut app, MenuButton::Options);
    click(&mut app, MenuButton::Controls);

    click(&mut app, MenuButton::Rebind(Action::MoveForward));
    hit_key(&mut app, KeyCode::Escape, "\u{1b}");
    // A frame in which a step back would have landed, had one been taken.
    app.update();
    assert_eq!(waiting_on(&app), None, "the row is still waiting for a key");
    assert_eq!(
        state(&app),
        AppState::Controls,
        "cancelling a row also left the screen"
    );

    hit_key(&mut app, KeyCode::Escape, "\u{1b}");
    app.update();
    assert_eq!(state(&app), AppState::Options, "one press, one rung");
}

/// What a display row currently reads on its right-hand button.
fn row_says(app: &mut App, which: DisplayText) -> String {
    app.world_mut()
        .query::<(&DisplayText, &Text)>()
        .iter(app.world())
        .find(|(mark, _)| **mark == which)
        .map(|(_, text)| text.0.clone())
        .expect("no row for the setting")
}

/// What the machine is actually doing.
fn display(app: &App) -> DisplaySettings {
    *app.world().resource::<DisplaySettings>()
}

/// And what the screen is set to, which until Apply is a different thing.
fn wanted(app: &App) -> DisplaySettings {
    app.world().resource::<Wanted>().0
}

/// What the Apply label is drawn in, which says whether it is offering
/// anything as much as its words do.
fn apply_ink(app: &mut App) -> Color {
    app.world_mut()
        .query::<(&DisplayText, &TextColor)>()
        .iter(app.world())
        .find(|(mark, _)| **mark == DisplayText::Apply)
        .map(|(_, colour)| colour.0)
        .expect("no Apply label")
}

/// The rungs the open list is offering, which is none at all when it is up.
fn listed(app: &mut App) -> Vec<Resolution> {
    app.world_mut()
        .query::<&MenuButton>()
        .iter(app.world())
        .filter_map(|button| match button {
            MenuButton::PickResolution(rung) => Some(*rung),
            _ => None,
        })
        .collect()
}

/// Everything an open list has put on the screen — the list and the sheet
/// behind it — counted rather than read, since what matters about the
/// sheet is that there is one and that it goes away again.
fn dropped(app: &mut App) -> usize {
    app.world_mut()
        .query::<&Dropped>()
        .iter(app.world())
        .count()
}

/// What a rung of the open list is filled with.
fn rung_fill(app: &mut App, rung: Resolution) -> Color {
    app.world_mut()
        .query::<(&MenuButton, &BackgroundColor)>()
        .iter(app.world())
        .find(|(button, _)| **button == MenuButton::PickResolution(rung))
        .map(|(_, fill)| fill.0)
        .expect("no such rung on the list")
}

/// Presses the sheet behind an open list — the real one, rather than a
/// stand-in carrying the same button, since the sheet's own existence is
/// half of what is being asked about.
fn click_the_sheet(app: &mut App) {
    let sheet = app
        .world_mut()
        .query_filtered::<Entity, (With<Dropped>, With<MenuButton>)>()
        .iter(app.world())
        .next()
        .expect("nothing behind the list to click");
    *app.world_mut()
        .get_mut::<Interaction>(sheet)
        .expect("the sheet catches nothing") = Interaction::Pressed;
    app.update();
}

fn on_trial(app: &App) -> bool {
    app.world().resource::<OnTrial>().0.is_some()
}

/// Puts time on the clock and gives one frame to see it, which a test
/// cannot do by waiting: nothing advances [`Time`] in a headless app, so a
/// test owns the whole clock.
fn advance(app: &mut App, by: Duration) {
    app.world_mut().resource_mut::<Time>().advance_by(by);
    app.update();
}

/// Past the end of a trial, however much of it is left.
fn run_out_the_trial(app: &mut App) {
    advance(app, Duration::from_secs(11));
}

/// A switch has to say which way it is set, and go on saying it — the row
/// is built with the setting it has and rewritten whenever it changes, and
/// reading the row once would test only half of that.
#[test]
fn the_display_rows_say_which_way_they_are_set() {
    let mut app = test_app(AppState::Display);
    assert_eq!(row_says(&mut app, DisplayText::Fullscreen), "Off");

    click(&mut app, MenuButton::ToggleFullscreen);
    assert!(wanted(&app).fullscreen);
    assert_eq!(row_says(&mut app, DisplayText::Fullscreen), "On");

    click(&mut app, MenuButton::ToggleFullscreen);
    assert!(!wanted(&app).fullscreen);
    assert_eq!(row_says(&mut app, DisplayText::Fullscreen), "Off");
}

/// The whole point of the screen editing what is *wanted*: a switch thrown
/// is a switch thrown on paper, and the machine hears nothing about it.
#[test]
fn nothing_reaches_the_machine_until_apply() {
    let mut app = test_app(AppState::Display);
    assert_eq!(
        apply_ink(&mut app),
        ON_PAPER.dim,
        "a button with nothing to apply was drawn as one worth pressing"
    );

    click(&mut app, MenuButton::ToggleFullscreen);
    click(&mut app, MenuButton::PickResolution(Resolution::Rows(720)));
    assert_eq!(
        display(&app),
        DisplaySettings::default(),
        "the window was changed by somebody only reading the screen"
    );
    assert_eq!(
        row_says(&mut app, DisplayText::Apply),
        "Apply",
        "the button kept quiet about a change waiting to be made"
    );
    assert_eq!(apply_ink(&mut app), ON_PAPER.text);

    click(&mut app, MenuButton::ApplyDisplay);
    assert_eq!(display(&app), wanted(&app));
    click(&mut app, MenuButton::ApplyDisplay);
    assert_eq!(
        row_says(&mut app, DisplayText::Apply),
        "Applied",
        "the button still offered to do something it had already done"
    );
    assert_eq!(apply_ink(&mut app), ON_PAPER.dim);
}

/// Every rung is one press from every other, which is the reason the list
/// exists: the button it replaced applied four settings to reach the fifth.
#[test]
fn a_rung_is_one_press_away_whichever_one_is_taken() {
    let mut app = test_app(AppState::Display);
    assert_eq!(row_says(&mut app, DisplayText::Resolution), "Native");
    assert!(listed(&mut app).is_empty(), "the list was already down");

    click(&mut app, MenuButton::OpenResolutions);
    let rungs = listed(&mut app);
    assert_eq!(rungs.len(), settings::LADDER.len());
    for rung in settings::LADDER {
        assert!(rungs.contains(&rung), "{rung:?} is not on the list");
    }

    click(&mut app, MenuButton::PickResolution(Resolution::Rows(720)));
    assert_eq!(row_says(&mut app, DisplayText::Resolution), "720p");
    assert!(
        listed(&mut app).is_empty(),
        "the list stayed down after a pick"
    );
    assert_eq!(dropped(&mut app), 0, "a pick left the sheet behind");

    // And back up the ladder without walking every rung between.
    click(&mut app, MenuButton::OpenResolutions);
    click(&mut app, MenuButton::PickResolution(Resolution::Rows(2160)));
    assert_eq!(row_says(&mut app, DisplayText::Resolution), "2160p");
}

/// A list is shut by anything that is not a rung — a click on the sheet
/// behind it, or Escape, which on this screen means the list before it
/// means the screen, or simply leaving.
///
/// The sheet is as much of the point as the list. It covers the whole
/// window, so a list that came up without one would be a list nothing
/// could shut, and one left behind afterwards would be a menu that
/// swallows every press aimed at anything under it.
#[test]
fn a_list_shuts_without_taking_anything_from_it() {
    let mut app = test_app(AppState::Display);

    click(&mut app, MenuButton::OpenResolutions);
    assert_eq!(dropped(&mut app), 2, "the list came up without its sheet");
    click_the_sheet(&mut app);
    assert!(listed(&mut app).is_empty());
    assert_eq!(dropped(&mut app), 0, "the sheet outlived the list");
    assert_eq!(wanted(&app).resolution, Resolution::Native);

    click(&mut app, MenuButton::OpenResolutions);
    press_key(&mut app, KeyCode::Escape);
    assert!(listed(&mut app).is_empty(), "Escape left the list down");
    assert_eq!(dropped(&mut app), 0, "Escape left the sheet over the menu");
    assert_eq!(
        state(&app),
        AppState::Display,
        "one Escape shut the list and left the screen as well"
    );

    press_key(&mut app, KeyCode::Escape);
    // The frame the transition lands on.
    app.update();
    assert_eq!(state(&app), AppState::Options);

    // And a list still down when the screen goes takes the sheet with it,
    // which is the whole reason the sheet hangs from the screen rather
    // than from the row the list drops out of.
    go_to(&mut app, AppState::Display);
    click(&mut app, MenuButton::OpenResolutions);
    assert_eq!(dropped(&mut app), 2);
    click(&mut app, MenuButton::Back);
    assert_eq!(dropped(&mut app), 0, "the sheet outlived the screen");
}

/// The rung already taken is drawn as the taken one from the moment the
/// list exists. [`highlight_buttons`] cannot do that job — its
/// `Changed<Interaction>` cannot fire until the frame after the spawn — and
/// a list that spends its first frame claiming nothing is picked is the
/// flicker the whole screen is built out of [`Wanted`] to avoid.
#[test]
fn the_taken_rung_is_lit_on_the_frame_the_list_appears() {
    let mut app = test_app(AppState::Display);
    click(&mut app, MenuButton::PickResolution(Resolution::Rows(1080)));

    // The one frame the press takes to be seen, and no more: another would
    // let `highlight_buttons` cover for a list that came up wrong.
    click_once(&mut app, MenuButton::OpenResolutions);
    assert_eq!(
        rung_fill(&mut app, Resolution::Rows(1080)),
        ON_PAPER.button.armed,
        "the list opened saying nothing was picked"
    );
    assert_eq!(
        rung_fill(&mut app, Resolution::Native),
        ON_PAPER.button.idle
    );
}

/// A change is made and then put back on its own, because the setting most
/// worth having is the one that can leave the player unable to read the
/// screen they would undo it from.
#[test]
fn an_applied_change_is_put_back_unless_it_is_kept() {
    let mut app = test_app(AppState::Display);

    click(&mut app, MenuButton::ToggleFullscreen);
    click(&mut app, MenuButton::ApplyDisplay);
    assert!(display(&app).fullscreen);
    assert!(on_trial(&app));
    assert_eq!(
        row_says(&mut app, DisplayText::Apply),
        "Keep (10)",
        "the button did not say what it had become"
    );

    // Counted up rather than down to the nearest second, which is the
    // whole of what the rounding is for: a trial with any time left at all
    // must never read as none left.
    advance(&mut app, Duration::from_millis(9_600));
    assert_eq!(
        row_says(&mut app, DisplayText::Apply),
        "Keep (1)",
        "four hundred milliseconds of trial read as no trial"
    );

    run_out_the_trial(&mut app);
    assert!(!display(&app).fullscreen, "the change was never put back");
    assert!(!on_trial(&app));
    assert_eq!(
        wanted(&app),
        display(&app),
        "the screen was left saying something the machine was not doing"
    );
    assert_eq!(
        row_says(&mut app, DisplayText::Apply),
        "Applied",
        "the button offered to apply what was already back in force"
    );
}

/// A trial is one question, and the rows are not it.
///
/// There is nowhere on the screen for a fresh edit to show while a trial
/// runs — the caveat line is the trial's and the button reads Keep — and
/// [`run_trial`] puts back what is *wanted* along with the settings. A row
/// that still answered would take an edit nobody could see and then lose
/// it without a word.
#[test]
fn the_rows_say_nothing_while_a_change_is_on_trial() {
    let mut app = test_app(AppState::Display);

    click(&mut app, MenuButton::ToggleFullscreen);
    click(&mut app, MenuButton::ApplyDisplay);

    click(&mut app, MenuButton::ToggleFullscreen);
    assert!(
        wanted(&app).fullscreen,
        "an edit was taken with nothing on the screen to say it had been"
    );

    click(&mut app, MenuButton::OpenResolutions);
    assert!(
        listed(&mut app).is_empty(),
        "a list came down that could take nothing"
    );
    click(&mut app, MenuButton::PickResolution(Resolution::Rows(720)));
    assert_eq!(wanted(&app).resolution, Resolution::Native);

    // So the trial ends on its own terms, with nothing of anybody's to
    // throw away.
    run_out_the_trial(&mut app);
    assert!(
        !display(&app).fullscreen,
        "an edit nobody could see reached the window"
    );

    // And with it settled the rows answer again.
    click(&mut app, MenuButton::ToggleFullscreen);
    assert!(wanted(&app).fullscreen, "the rows never came back");
}

/// And stood by when somebody who can evidently still see it says so.
#[test]
fn keeping_a_change_stands_by_it() {
    let mut app = test_app(AppState::Display);

    click(&mut app, MenuButton::ToggleFullscreen);
    click(&mut app, MenuButton::ApplyDisplay);
    click(&mut app, MenuButton::ApplyDisplay);
    assert!(!on_trial(&app));

    run_out_the_trial(&mut app);
    assert!(
        display(&app).fullscreen,
        "a kept change was put back anyway"
    );
}

/// Leaving is the same answer as keeping, and for the same reason: a
/// player who found Back has shown they can see. What was never applied
/// goes the other way and is dropped.
#[test]
fn leaving_stands_by_a_trial_and_drops_what_was_never_applied() {
    let mut app = test_app(AppState::Display);

    click(&mut app, MenuButton::ToggleFullscreen);
    click(&mut app, MenuButton::ApplyDisplay);
    click(&mut app, MenuButton::PickResolution(Resolution::Rows(720)));
    click(&mut app, MenuButton::Back);
    assert!(!on_trial(&app));
    assert!(
        display(&app).fullscreen,
        "an applied change was undone by leaving"
    );
    assert_eq!(
        display(&app).resolution,
        Resolution::Native,
        "a change nobody applied reached the window"
    );

    // And the screen comes back saying what the machine is doing rather
    // than what it was last asked for.
    go_to(&mut app, AppState::Display);
    assert_eq!(wanted(&app), display(&app));
    assert_eq!(row_says(&mut app, DisplayText::Fullscreen), "On");
}

/// A machine that has reported no monitors cannot promise a resolution
/// below native, and the screen owns up to it rather than pretending.
/// Headless is exactly that machine, which is what makes this testable.
///
/// Only about filling the screen, though: a caveat is a word about a
/// display mode, and a window has no need of one.
#[test]
fn a_resolution_the_screen_cannot_promise_is_owned_up_to() {
    let mut app = test_app(AppState::Display);
    assert_eq!(row_says(&mut app, DisplayText::Caveat), "");

    click(&mut app, MenuButton::PickResolution(Resolution::Rows(2160)));
    click(&mut app, MenuButton::ApplyDisplay);
    click(&mut app, MenuButton::ApplyDisplay);
    assert_eq!(
        row_says(&mut app, DisplayText::Caveat),
        "",
        "a windowed size was called impossible, and it is only a size"
    );

    click(&mut app, MenuButton::ToggleFullscreen);
    assert!(
        row_says(&mut app, DisplayText::Caveat).contains("no 2160p mode"),
        "the screen claimed a mode it has no monitor to ask about"
    );
}

/// The display screen must not tell its two ways in apart: the same
/// settings and the same rows, whichever side it was opened from.
#[test]
fn the_display_screen_is_the_same_screen_over_a_paused_world() {
    let mut app = paused_app();
    click(&mut app, MenuButton::Options);
    click(&mut app, MenuButton::Display);

    click(&mut app, MenuButton::ToggleFullscreen);
    click(&mut app, MenuButton::ApplyDisplay);
    assert!(display(&app).fullscreen);
    assert_eq!(row_says(&mut app, DisplayText::Fullscreen), "On");
    assert_eq!(state(&app), AppState::InWorld);
}

#[test]
fn a_row_waits_for_a_key_and_then_takes_it() {
    let mut app = test_app(AppState::Controls);

    click(&mut app, MenuButton::Rebind(Action::MoveForward));
    assert_eq!(waiting_on(&app), Some(Action::MoveForward));

    type_key(&mut app, KeyCode::KeyJ, "j");
    assert_eq!(bindings(&app).key(Action::MoveForward), KeyCode::KeyJ);
    assert_eq!(waiting_on(&app), None, "the row is still waiting");
}

#[test]
fn a_key_is_named_by_what_it_typed_not_where_it_sits() {
    let mut app = test_app(AppState::Controls);

    // A Dvorak keyboard pressing the key marked "," reports the position
    // where a US keyboard keeps W. The row has to say what the keycap says.
    click(&mut app, MenuButton::Rebind(Action::TurnLeft));
    type_key(&mut app, KeyCode::KeyW, ",");

    assert_eq!(bindings(&app).key(Action::TurnLeft), KeyCode::KeyW);
    assert_eq!(bindings(&app).name(Action::TurnLeft), ",");
}

/// What an action's row currently reads on the right-hand button.
fn row_text(app: &mut App, action: Action) -> String {
    app.world_mut()
        .query::<(&KeyText, &Text)>()
        .iter(app.world())
        .find(|(key, _)| key.0 == action)
        .map(|(_, text)| text.0.clone())
        .expect("no row for the action")
}

#[test]
fn an_armed_row_says_it_is_waiting_and_then_shows_the_new_key() {
    let mut app = test_app(AppState::MainMenu);
    go_to(&mut app, AppState::Controls);
    assert_eq!(row_text(&mut app, Action::SteerLeft), "A");

    click(&mut app, MenuButton::Rebind(Action::SteerLeft));
    assert_eq!(row_text(&mut app, Action::SteerLeft), "press a key");

    type_key(&mut app, KeyCode::KeyH, "h");
    app.update();
    assert_eq!(row_text(&mut app, Action::SteerLeft), "H");
}

#[test]
fn escape_abandons_a_capture_and_changes_nothing() {
    let mut app = test_app(AppState::Controls);
    let before = bindings(&app).clone();

    click(&mut app, MenuButton::Rebind(Action::MoveBack));
    type_key(&mut app, KeyCode::Escape, "\u{1b}");

    assert_eq!(waiting_on(&app), None);
    assert_eq!(bindings(&app), &before);
    // And having cancelled, we're still on the screen rather than back out.
    assert_eq!(state(&app), AppState::Controls);
}

#[test]
fn a_reserved_key_is_refused_and_the_row_keeps_waiting() {
    let mut app = test_app(AppState::Controls);

    click(&mut app, MenuButton::Rebind(Action::MoveBack));
    type_key(&mut app, KeyCode::ArrowUp, "");

    assert_eq!(bindings(&app).key(Action::MoveBack), KeyCode::KeyS);
    assert_eq!(
        waiting_on(&app),
        Some(Action::MoveBack),
        "a refused key should leave the row armed"
    );

    // And a real key still lands afterwards.
    type_key(&mut app, KeyCode::KeyN, "n");
    assert_eq!(bindings(&app).key(Action::MoveBack), KeyCode::KeyN);
}

#[test]
fn taking_a_key_another_action_had_trades_the_two() {
    let mut app = test_app(AppState::Controls);

    click(&mut app, MenuButton::Rebind(Action::MoveForward));
    type_key(&mut app, KeyCode::KeyE, "e");

    assert_eq!(bindings(&app).key(Action::MoveForward), KeyCode::KeyE);
    assert_eq!(
        bindings(&app).key(Action::TurnRight),
        KeyCode::KeyW,
        "turning right should have taken the key panning gave up"
    );
}

#[test]
fn defaults_puts_every_key_back() {
    let mut app = test_app(AppState::Controls);

    click(&mut app, MenuButton::Rebind(Action::SteerLeft));
    type_key(&mut app, KeyCode::KeyZ, "z");
    click(&mut app, MenuButton::ResetKeys);

    assert_eq!(bindings(&app), &KeyBindings::default());
    assert_eq!(waiting_on(&app), None);
}

#[test]
fn escape_leaves_the_controls_screen_when_no_row_is_waiting() {
    let mut app = test_app(AppState::Controls);
    type_key(&mut app, KeyCode::Escape, "\u{1b}");
    app.update();
    assert_eq!(state(&app), AppState::Options);
}

#[test]
fn leaving_the_screen_forgets_a_waiting_row() {
    let mut app = test_app(AppState::Controls);

    click(&mut app, MenuButton::Rebind(Action::TurnRight));
    click(&mut app, MenuButton::Back);
    assert_eq!(state(&app), AppState::Options);

    go_to(&mut app, AppState::Controls);
    assert_eq!(
        waiting_on(&app),
        None,
        "the screen came back still waiting for a key"
    );
}

#[test]
fn keys_pressed_before_the_screen_opened_are_not_taken() {
    // Escape leaves a match, so the key that gets you to the menu is one
    // that was pressed moments before the controls screen can open. None of
    // that backlog may count as an answer to "press a key".
    let mut app = test_app(AppState::InWorld);
    type_key(&mut app, KeyCode::KeyJ, "j");

    go_to(&mut app, AppState::Controls);
    click(&mut app, MenuButton::Rebind(Action::MoveForward));

    assert_eq!(waiting_on(&app), Some(Action::MoveForward));
    assert_eq!(bindings(&app).key(Action::MoveForward), KeyCode::KeyW);
}

#[test]
fn escape_pauses_the_match_rather_than_leaving_it() {
    let mut app = test_app(AppState::InWorld);
    press_key(&mut app, KeyCode::Escape);
    app.update();
    assert_eq!(helm(&app), Some(Helm::Paused));
    // The whole point: the world is still there to go back to.
    assert_eq!(state(&app), AppState::InWorld);
}

#[test]
fn escape_again_returns_to_the_helm() {
    let mut app = test_app(AppState::InWorld);
    press_key(&mut app, KeyCode::Escape);
    app.update();
    press_key(&mut app, KeyCode::Escape);
    app.update();
    assert_eq!(helm(&app), Some(Helm::Sailing));
    assert_eq!(state(&app), AppState::InWorld);
}

#[test]
fn resume_returns_to_the_helm() {
    let mut app = paused_app();
    click(&mut app, MenuButton::Resume);
    assert_eq!(helm(&app), Some(Helm::Sailing));
    assert_eq!(state(&app), AppState::InWorld);
}

/// The one press that gives the world up — and the only one, which is what
/// the pause menu is for.
#[test]
fn leaving_the_world_is_a_button_of_its_own() {
    let mut app = paused_app();
    click(&mut app, MenuButton::LeaveWorld);
    assert_eq!(state(&app), AppState::MainMenu);
    // Gone with the world it belonged to.
    assert_eq!(helm(&app), None);
}

#[test]
fn pausing_puts_a_menu_up_and_resuming_takes_it_down() {
    let mut app = paused_app();
    assert_eq!(named(&mut app, "Pause menu"), 1);
    click(&mut app, MenuButton::Resume);
    assert_eq!(named(&mut app, "Pause menu"), 0);
}

/// The same ladder over a paused world, and the thing that matters at
/// every rung of it: the world is still there. Leaving `AppState::InWorld`
/// is what takes a world down — see [`Helm`] — so a settings screen that
/// reached for an `AppState` would evict everyone sailing in a shared one.
#[test]
fn the_options_ladder_over_a_paused_world_never_leaves_the_world() {
    let mut app = paused_app();
    click(&mut app, MenuButton::Options);
    assert_eq!(helm(&app), Some(Helm::Options));
    assert_eq!(named(&mut app, "Options screen"), 1);

    click(&mut app, MenuButton::Display);
    assert_eq!(helm(&app), Some(Helm::Display));
    assert_eq!(named(&mut app, "Display screen"), 1);
    click(&mut app, MenuButton::Back);
    assert_eq!(helm(&app), Some(Helm::Options));
    assert_eq!(named(&mut app, "Display screen"), 0);

    click(&mut app, MenuButton::Controls);
    assert_eq!(helm(&app), Some(Helm::Controls));
    assert_eq!(named(&mut app, "Controls screen"), 1);
    click(&mut app, MenuButton::Back);
    assert_eq!(helm(&app), Some(Helm::Options));

    click(&mut app, MenuButton::Back);
    assert_eq!(helm(&app), Some(Helm::Paused));
    assert_eq!(named(&mut app, "Options screen"), 0);
    assert_eq!(state(&app), AppState::InWorld, "the world was given up");
}

/// Escape is Back on every rung of the ladder, and stops at the pause menu
/// rather than carrying on out of the world.
#[test]
fn escape_walks_back_down_the_paused_ladder_one_rung_at_a_time() {
    let mut app = paused_app();
    click(&mut app, MenuButton::Options);
    click(&mut app, MenuButton::Display);

    for expected in [Helm::Options, Helm::Paused] {
        press_key(&mut app, KeyCode::Escape);
        // The frame the transition lands on.
        app.update();
        assert_eq!(helm(&app), Some(expected));
    }
    assert_eq!(state(&app), AppState::InWorld);
}

#[test]
fn escape_backs_out_of_the_paused_controls_to_the_options_screen() {
    let mut app = paused_app();
    click(&mut app, MenuButton::Options);
    click(&mut app, MenuButton::Controls);
    type_key(&mut app, KeyCode::Escape, "");
    // The frame the transition lands on.
    app.update();
    assert_eq!(helm(&app), Some(Helm::Options));
    assert_eq!(state(&app), AppState::InWorld);
}

/// Escape means "not that key" while a row is armed, wherever the screen
/// was opened from — so it must not also step back to the pause menu.
#[test]
fn escape_on_the_paused_controls_cancels_an_armed_row_first() {
    let mut app = paused_app();
    click(&mut app, MenuButton::Options);
    click(&mut app, MenuButton::Controls);
    click(&mut app, MenuButton::Rebind(Action::MoveForward));
    assert_eq!(waiting_on(&app), Some(Action::MoveForward));

    type_key(&mut app, KeyCode::Escape, "");
    // A frame in which a step back would have landed, had one been taken.
    app.update();
    assert_eq!(waiting_on(&app), None);
    assert_eq!(helm(&app), Some(Helm::Controls));
}

#[test]
fn keys_rebound_from_the_pause_menu_take() {
    let mut app = paused_app();
    click(&mut app, MenuButton::Options);
    click(&mut app, MenuButton::Controls);
    click(&mut app, MenuButton::Rebind(Action::MoveForward));
    type_key(&mut app, KeyCode::KeyT, "t");
    assert_eq!(bindings(&app).key(Action::MoveForward), KeyCode::KeyT);
}

/// Leaving a shared world shuts it on whoever else is in it, and that is
/// worth a word before the button that does it.
#[test]
fn the_pause_menu_warns_before_closing_a_shared_world() {
    let mut app = test_app(AppState::InWorld);
    app.insert_resource(Hosting(fake_host("0.0.0.0:0")));
    press_key(&mut app, KeyCode::Escape);
    app.update();
    assert!(screen_text(&mut app).contains("leaving closes it on them"));
}

/// But a world of one's own is served too — over the loopback — so the
/// warning must not go to somebody sailing alone, who has nobody to
/// strand.
#[test]
fn a_world_of_ones_own_gets_no_warning_though_it_is_hosted_too() {
    let mut app = test_app(AppState::InWorld);
    app.insert_resource(Hosting(fake_host("127.0.0.1:0")));
    press_key(&mut app, KeyCode::Escape);
    app.update();
    assert!(!screen_text(&mut app).contains("leaving closes it on them"));
}

#[test]
fn random_seeds_fit_the_field() {
    let seed = random_seed();
    assert!(seed.to_string().len() <= MAX_SEED_DIGITS);
}

#[test]
fn the_dialog_opens_on_a_world_nobody_chose() {
    let settings = NewWorldSettings::default();
    assert!(settings.seed.len() <= MAX_SEED_DIGITS);
    // And a different one each time the game is started, rather than one
    // island every player who pressed start ever saw.
    assert_ne!(settings.seed, NewWorldSettings::default().seed);
}
