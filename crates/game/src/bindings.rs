//! Which key does what, and the means to change it.
//!
//! Bindings are held as [`KeyCode`], which is a *position* on the keyboard
//! rather than a letter: `KeyCode::KeyW` is whichever key sits where W sits on
//! a US layout, whatever it happens to be labelled. That is the right thing to
//! store — a binding made by pressing a key keeps working when the same key is
//! pressed again — but it is the wrong thing to *show*, because on a Dvorak or
//! Colemak keyboard that key isn't a W and saying so helps nobody. So a binding
//! also remembers what the keypress that made it actually typed, and the
//! settings screen prefers that.

use bevy::input::keyboard::Key;
use bevy::prelude::*;

/// A control the player can put on a key of their choosing: driving their
/// boat — ahead, astern, helm over — stepping ashore and back aboard, or
/// turning the view around them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveForward,
    MoveBack,
    SteerLeft,
    SteerRight,
    /// One key for both directions of the same threshold: ashore it boards,
    /// aboard it goes ashore — see `player::embark_or_land`.
    Board,
    /// Held at anchor through the night, to have it over with — see
    /// `sky::ask_for_dawn`.
    WaitOutNight,
    TurnLeft,
    TurnRight,
}

impl Action {
    /// Every action, in the order the settings screen lists them.
    pub const ALL: [Action; 8] = [
        Action::MoveForward,
        Action::MoveBack,
        Action::SteerLeft,
        Action::SteerRight,
        Action::Board,
        Action::WaitOutNight,
        Action::TurnLeft,
        Action::TurnRight,
    ];

    /// How the settings screen names the action. The last two say "view"
    /// because steering left and turning left are different keys doing
    /// different things, and a list that read "Steer left … Turn left" would
    /// leave the player to guess which is which.
    pub fn label(self) -> &'static str {
        match self {
            Action::MoveForward => "Forward",
            Action::MoveBack => "Back",
            Action::SteerLeft => "Steer left",
            Action::SteerRight => "Steer right",
            Action::Board => "Go ashore / board",
            Action::WaitOutNight => "Wait for dawn",
            Action::TurnLeft => "Turn view left",
            Action::TurnRight => "Turn view right",
        }
    }

    /// Where the action starts out: WASD to drive, F to step ashore or
    /// aboard, R to wait a night out, and Q/E to turn the view — WASD and
    /// Q/E being what the game had before any of this was configurable.
    pub fn default_key(self) -> KeyCode {
        match self {
            Action::MoveForward => KeyCode::KeyW,
            Action::MoveBack => KeyCode::KeyS,
            Action::SteerLeft => KeyCode::KeyA,
            Action::SteerRight => KeyCode::KeyD,
            Action::Board => KeyCode::KeyF,
            Action::WaitOutNight => KeyCode::KeyR,
            Action::TurnLeft => KeyCode::KeyQ,
            Action::TurnRight => KeyCode::KeyE,
        }
    }

    /// Position in [`Action::ALL`], which is how [`KeyBindings`] indexes them.
    ///
    /// The discriminant, since the variants are declared in the order `ALL`
    /// lists them and carry no data. Spelling the mapping out a third time was
    /// only a third place for it to disagree with the other two;
    /// `every_action_indexes_to_its_own_slot` is what holds the two that are
    /// left together.
    fn index(self) -> usize {
        self as usize
    }
}

/// Keys the player may not take, because taking them would leave no way back.
/// The arrows are a permanent second set of movement keys, so however
/// thoroughly the rest is rebound the player can always get about; Escape is
/// the one step back from wherever the player is — into the pause menu, out of
/// it again, and out of setting a key; and the backquote is the way into and
/// out of the debug console — see `crate::console`.
pub const RESERVED: [KeyCode; 6] = [
    KeyCode::Escape,
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::Backquote,
];

/// Whether a key is the player's to give away.
pub fn is_bindable(key: KeyCode) -> bool {
    !RESERVED.contains(&key)
}

/// One action's key, and what that key typed when it was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding {
    key: KeyCode,
    /// `None` until the key has been set by pressing it — the defaults have
    /// never been through a keypress, so there is nothing to have read off one.
    typed: Option<String>,
}

/// What every action is bound to. Changing it changes the controls immediately;
/// nothing caches a lookup.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct KeyBindings {
    /// One entry per [`Action`], indexed by [`Action::index`].
    bound: [Binding; Action::ALL.len()],
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            bound: Action::ALL.map(|action| Binding {
                key: action.default_key(),
                typed: None,
            }),
        }
    }
}

impl KeyBindings {
    /// The key that drives an action.
    pub fn key(&self, action: Action) -> KeyCode {
        self.bound[action.index()].key
    }

    /// True while an action's own key is down, or the arrow key that
    /// permanently shadows it. The arrows aren't rebindable and aren't listed
    /// in the settings screen: they're the floor under it, so that no set of
    /// bindings, however muddled, can leave the player unable to move.
    pub fn held(&self, keys: &ButtonInput<KeyCode>, action: Action, arrow: KeyCode) -> bool {
        keys.any_pressed([self.key(action), arrow])
    }

    /// What to call that key on screen — what it typed if it was set by
    /// pressing it, and otherwise the name of the position it sits at.
    pub fn name(&self, action: Action) -> String {
        let binding = &self.bound[action.index()];
        binding
            .typed
            .clone()
            .unwrap_or_else(|| key_name(binding.key))
    }

    /// The action a key currently drives, if any.
    pub fn action_bound_to(&self, key: KeyCode) -> Option<Action> {
        Action::ALL
            .into_iter()
            .find(|action| self.key(*action) == key)
    }

    /// Puts an action on a key. If another action already had that key the two
    /// trade places, rather than the other one being left with nothing: an
    /// action with no key at all is a control the player can't get back to
    /// except by resetting the lot.
    pub fn bind(&mut self, action: Action, key: KeyCode, typed: Option<String>) {
        if let Some(other) = self.action_bound_to(key) {
            if other != action {
                let displaced = self.bound[action.index()].clone();
                self.bound[other.index()] = displaced;
            }
        }

        self.bound[action.index()] = Binding { key, typed };
    }
}

/// Names a key by its position, for keys that have never been pressed here.
/// `KeyCode`'s own spelling is close enough to readable that only its prefixes
/// need taking off — `KeyW` and `Digit4` say W and 4.
pub fn key_name(key: KeyCode) -> String {
    let raw = format!("{key:?}");
    let name = raw
        .strip_prefix("Key")
        .or_else(|| raw.strip_prefix("Digit"))
        .unwrap_or(raw.as_str());
    name.to_string()
}

/// What a keypress reads as on the keyboard it was made on, when it reads as
/// anything at all. Keys that type nothing — Space, Tab, the function row —
/// have no character to show and fall back to [`key_name`].
pub fn typed_label(logical: &Key) -> Option<String> {
    match logical {
        Key::Character(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_uppercase())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_indexes_to_its_own_slot() {
        for (position, action) in Action::ALL.into_iter().enumerate() {
            assert_eq!(action.index(), position, "{action:?} indexes elsewhere");
        }
    }

    #[test]
    fn defaults_give_every_action_its_own_key() {
        let bindings = KeyBindings::default();
        let mut keys: Vec<KeyCode> = Action::ALL.into_iter().map(|a| bindings.key(a)).collect();
        let count = keys.len();
        keys.sort_by_key(|key| format!("{key:?}"));
        keys.dedup();
        assert_eq!(keys.len(), count, "two actions share a default key");
    }

    #[test]
    fn defaults_are_all_keys_the_player_could_have_chosen() {
        for action in Action::ALL {
            assert!(
                is_bindable(action.default_key()),
                "{action:?} defaults to a reserved key"
            );
        }
    }

    #[test]
    fn binding_a_free_key_leaves_every_other_action_alone() {
        let mut bindings = KeyBindings::default();
        bindings.bind(Action::MoveForward, KeyCode::KeyM, None);

        assert_eq!(bindings.key(Action::MoveForward), KeyCode::KeyM);
        for action in Action::ALL
            .into_iter()
            .filter(|a| *a != Action::MoveForward)
        {
            assert_eq!(bindings.key(action), action.default_key());
        }
    }

    #[test]
    fn binding_a_taken_key_trades_it_for_the_old_one() {
        let mut bindings = KeyBindings::default();
        // Pan forward takes the key that turns left, so turning left gets the
        // key panning forward gave up.
        bindings.bind(Action::MoveForward, KeyCode::KeyQ, None);

        assert_eq!(bindings.key(Action::MoveForward), KeyCode::KeyQ);
        assert_eq!(bindings.key(Action::TurnLeft), KeyCode::KeyW);
    }

    #[test]
    fn rebinding_an_action_to_the_key_it_already_has_keeps_it() {
        let mut bindings = KeyBindings::default();
        bindings.bind(Action::TurnLeft, KeyCode::KeyQ, Some("'".to_string()));

        assert_eq!(bindings.key(Action::TurnLeft), KeyCode::KeyQ);
        assert_eq!(bindings.name(Action::TurnLeft), "'");
        // And nothing else moved to fill a gap that was never made.
        assert_eq!(bindings.key(Action::MoveForward), KeyCode::KeyW);
    }

    #[test]
    fn no_two_actions_ever_share_a_key() {
        let mut bindings = KeyBindings::default();
        // Walk one key through every action in turn. Each hand-off has to leave
        // the six actions on six distinct keys.
        for action in Action::ALL {
            bindings.bind(action, KeyCode::KeyZ, None);

            let mut keys: Vec<String> = Action::ALL
                .into_iter()
                .map(|a| format!("{:?}", bindings.key(a)))
                .collect();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), Action::ALL.len(), "after binding {action:?}");
        }
    }

    #[test]
    fn a_key_set_by_pressing_it_is_named_by_what_it_typed() {
        let mut bindings = KeyBindings::default();
        // What a Dvorak keyboard does: the key where W sits types a comma.
        bindings.bind(Action::MoveForward, KeyCode::KeyW, Some(",".to_string()));

        assert_eq!(bindings.name(Action::MoveForward), ",");
        assert_eq!(bindings.key(Action::MoveForward), KeyCode::KeyW);
    }

    #[test]
    fn a_key_never_pressed_is_named_by_its_position() {
        let bindings = KeyBindings::default();
        assert_eq!(bindings.name(Action::MoveForward), "W");
        assert_eq!(bindings.name(Action::TurnRight), "E");
    }

    #[test]
    fn key_names_lose_their_prefixes_but_keep_their_shape() {
        assert_eq!(key_name(KeyCode::KeyW), "W");
        assert_eq!(key_name(KeyCode::Digit4), "4");
        assert_eq!(key_name(KeyCode::Space), "Space");
        assert_eq!(key_name(KeyCode::Semicolon), "Semicolon");
    }

    #[test]
    fn reserved_keys_are_not_the_players_to_take() {
        for key in RESERVED {
            assert!(!is_bindable(key), "{key:?} should be reserved");
        }
        assert!(is_bindable(KeyCode::KeyM));
    }

    #[test]
    fn typed_labels_come_from_characters_and_are_upper_case() {
        assert_eq!(typed_label(&Key::Character("q".into())), Some("Q".into()));
        assert_eq!(typed_label(&Key::Character(",".into())), Some(",".into()));
    }

    #[test]
    fn keys_that_type_nothing_have_no_typed_label() {
        assert_eq!(typed_label(&Key::Space), None);
        assert_eq!(typed_label(&Key::Character(" ".into())), None);
        assert_eq!(typed_label(&Key::Shift), None);
    }

    #[test]
    fn action_bound_to_finds_the_action_and_nothing_else() {
        let bindings = KeyBindings::default();
        assert_eq!(
            bindings.action_bound_to(KeyCode::KeyA),
            Some(Action::SteerLeft)
        );
        assert_eq!(bindings.action_bound_to(KeyCode::KeyM), None);
    }
}
