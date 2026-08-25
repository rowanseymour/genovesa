//! The furniture every menu screen is built out of.
//!
//! One sheet, one panel, one button, and the inks they are drawn in. The
//! screens themselves are the modules beside this one; what they share is
//! here, because a second button that was nearly the first is how a set of
//! screens stops looking like one thing.
//!
//! Two palettes and no more. A screen either stands on the paper of the main
//! menu or over a world that is still there behind it, and [`Palette`] is the
//! whole of the difference — see [`ON_PAPER`] and [`OVER_THE_WORLD`], and the
//! builders that take one so a screen can be stood up on either side.

use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;

use crate::chart::{INK, INK_DIM, PAPER};

use super::{MenuButton, StatusText};

/// How big a dialog's own title is drawn — well under [`TITLE_SIZE`], since it
/// names a screen rather than the game.
pub(super) const HEADING_SIZE: f32 = 34.0;

/// The inks a menu is drawn with.
///
/// There are two sets and there have to be, because there are two places a
/// menu goes up. Outside a world it stands on [`crate::backdrop`]'s sheet and
/// is drawn *on paper*, as part of the chart; inside one it stands over the
/// world itself, where paper would be a chart laid on the sea. They are not
/// one thing lit two ways — a button drawn in ink and a button drawn as a
/// panel are two different objects — so the whole set travels together and no
/// screen mixes them.
#[derive(Component, Clone, Copy)]
pub(super) struct Palette {
    pub(super) text: Color,
    pub(super) dim: Color,
    /// Every edge the menu draws, panels and buttons alike. One colour for all
    /// of them, so a button can never come out brighter than the frame it sits
    /// in.
    pub(super) edge: Color,
    pub(super) panel: Color,
    /// What the full-screen node behind the menu is washed with. Nothing on
    /// paper — the sheet is already the colour it should be, and a wash over
    /// it would only make it dirty paper.
    pub(super) behind: Color,
    /// Whether a panel is a *cartouche*: the double rule with the paper
    /// showing between the two lines that an engraver letters a chart's title
    /// inside. It is the whole difference between a box drawn on the paper and
    /// a box laid over it.
    pub(super) cartouche: bool,
    /// What a button does under the pointer, and what one that is switched on
    /// or waiting for a key looks like.
    pub(super) button: Highlight,
}

/// On the sheet.
///
/// A button is drawn rather than filled: paper has no lights to turn on, so
/// the only thing that can happen to a shape on it is more ink, and pressing
/// one is a wash of the reading ink. Armed is the one thing in another colour,
/// the same red the chart marks the reader's own position in, which is how the
/// eye finds it.
pub(super) const ON_PAPER: Palette = Palette {
    text: INK,
    dim: INK_DIM,
    edge: INK,
    panel: PAPER,
    behind: Color::NONE,
    cartouche: true,
    button: Highlight {
        idle: Color::NONE,
        hover: Color::srgba(0.24, 0.17, 0.11, 0.13),
        press: Color::srgba(0.24, 0.17, 0.11, 0.26),
        armed: Color::srgba(0.55, 0.16, 0.12, 0.22),
    },
};

/// Over the world: the pause menu, and the controls screen reached from it.
pub(super) const OVER_THE_WORLD: Palette = Palette {
    text: Color::srgb(0.88, 0.87, 0.80),
    dim: Color::srgb(0.60, 0.60, 0.55),
    edge: Color::srgb(0.70, 0.69, 0.62),
    panel: Color::srgba(0.09, 0.11, 0.10, 0.94),
    behind: Color::srgba(0.05, 0.07, 0.09, 0.72),
    cartouche: false,
    button: Highlight {
        idle: Color::srgb(0.17, 0.20, 0.16),
        hover: Color::srgb(0.26, 0.31, 0.22),
        press: Color::srgb(0.35, 0.42, 0.28),
        armed: Color::srgb(0.44, 0.40, 0.18),
    },
};

/// What a button is filled with in each of its states.
///
/// Carried on the button entity rather than looked up when one is hovered,
/// because [`highlight_buttons`] sees every button in the app and cannot tell
/// from one which of the two sheets it was drawn on.
#[derive(Component, Clone, Copy)]
pub(super) struct Highlight {
    pub(super) idle: Color,
    pub(super) hover: Color,
    pub(super) press: Color,
    /// A button switched on, or a key row waiting for a key.
    pub(super) armed: Color,
}

/// Full-screen, centred column that every menu screen is built inside.
pub(super) fn screen(ink: &Palette) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: Val::Px(12.0),
            // On paper the room a menu has is the *paper*, not the window:
            // the sheet's edge is ruled just inside it, and a cartouche laid
            // across that rule reads as a chart with a hole cut in it. The
            // same margin the sheet's own furniture stands off by.
            padding: UiRect::all(Val::Px(if ink.cartouche {
                crate::chart::PAPER_MARGIN
            } else {
                0.0
            })),
            ..default()
        },
        BackgroundColor(ink.behind),
    )
}

/// The bordered box a menu is built inside.
///
/// `row_gap` is the space between the rows stacked in it, and `pad` the room
/// inside its rule. Both are the controls screen's doing: it packs its list of
/// key rows and has to fit them between the sheet's own edges, where a dialog
/// of a few fields has all the paper it wants.
pub(super) fn panel(ink: &Palette, row_gap: f32, pad: f32) -> (Node, BackgroundColor, BorderColor) {
    (
        Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::axes(Val::Px(32.0), Val::Px(pad)),
            border: UiRect::all(Val::Px(2.0)),
            row_gap: Val::Px(row_gap),
            ..default()
        },
        BackgroundColor(ink.panel),
        BorderColor::all(ink.edge),
    )
}

/// Marks the panel the wheel rolls — see [`scrolling_panel`]. A marker rather
/// than a query for [`ScrollPosition`], because every UI node carries one of
/// those whether or not it scrolls.
#[derive(Component)]
pub(super) struct ScrollingPanel;

/// The same box, allowed to be taller than the window: held to the room the
/// screen gives it, with everything past that reached by the wheel — see
/// [`scroll_the_panel`]. Only the controls screen wears this. Its stack of key
/// rows is the one thing on any menu whose height follows from a count, and at
/// the UI's smallest it is the one stack a short window cannot hold.
pub(super) fn scrolling_panel(ink: &Palette, row_gap: f32, pad: f32) -> impl Bundle {
    let (mut node, colour, border) = panel(ink, row_gap, pad);
    node.max_height = Val::Percent(100.0);
    node.overflow = Overflow::scroll_y();
    (node, colour, border, ScrollingPanel)
}

/// How much room a panel leaves inside its rule, above and below.
pub(super) const PANEL_PADDING: f32 = 32.0;

/// And how much the controls screen leaves, which is as little as the key
/// rows and two lines of prose can be got into a sheet in.
pub(super) const CONTROLS_PADDING: f32 = 14.0;

/// How far inside a cartouche's outer rule the second one runs, in pixels.
pub(super) const CARTOUCHE_INSET: f32 = 6.0;

/// The second rule that makes a panel a cartouche.
///
/// Taken out of the flow and pinned inside the panel's padding, so it costs
/// the contents no room and does not have to know how much there are of them.
/// A double rule with the paper showing between the two is what an engraver
/// puts a chart's title inside, and it is the whole difference between a box
/// drawn on paper and a box laid over it.
pub(super) fn cartouche_rule(parent: &mut ChildSpawnerCommands, ink: &Palette) {
    if !ink.cartouche {
        return;
    }
    parent.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(CARTOUCHE_INSET),
            top: Val::Px(CARTOUCHE_INSET),
            right: Val::Px(CARTOUCHE_INSET),
            bottom: Val::Px(CARTOUCHE_INSET),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        // Pinned to the box rather than to what is in it, so on the one panel
        // that scrolls the rule stays where a rule is: on the panel.
        bevy::ui::IgnoreScroll(BVec2::TRUE),
        BorderColor::all(ink.edge),
    ));
}

/// A dialog's title, and how much room it keeps between itself and what
/// follows — which is a whole line's worth on the dialogs that open with a
/// field, and almost nothing on the controls screen, where the line under it
/// is part of the same thought.
pub(super) fn heading(parent: &mut ChildSpawnerCommands, ink: &Palette, text: &str, below: f32) {
    parent.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(HEADING_SIZE),
            ..default()
        },
        TextColor(ink.text),
        Node {
            margin: UiRect::bottom(Val::Px(below)),
            ..default()
        },
    ));
}

/// The line a dialog reports a dial on. Spawned even when there is nothing to
/// say — a line that appeared only when it had text would push the buttons
/// under it down the moment the player pressed one — and spawned with whatever
/// there is to say, since the set-sail screen can be built again mid-dial and
/// must not come back having forgotten it.
pub(super) fn status_line(parent: &mut ChildSpawnerCommands, ink: &Palette, saying: &str) {
    parent.spawn((
        StatusText,
        Text::new(saying.to_string()),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(ink.text),
        Node {
            margin: UiRect::top(Val::Px(8.0)),
            // Held open so an empty line still takes its room, for the reason
            // above.
            height: Val::Px(18.0),
            ..default()
        },
    ));
}

pub(super) fn label(parent: &mut ChildSpawnerCommands, ink: &Palette, text: &str) {
    parent.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(ink.dim),
    ));
}

/// A menu button without its label, so that callers who need to mark the label
/// — as the controls screen does, to rewrite it later — can spawn their own.
pub(super) fn button(ink: &Palette, action: MenuButton, width: f32) -> impl Bundle {
    padded_button(ink, action, width, 12.0)
}

/// How much shorter the button on a row is than a menu's: worn by the controls
/// screen's key rows, and by the display screen's switches, its resolution
/// button and every rung of that button's list.
///
/// Set by the controls screen, which is the one screen whose height follows
/// from how many things there are to bind and has to fit in the window at
/// every count it is ever going to have — it can scroll, but scrolling at the
/// laid-out size would be the packing having failed. Taking six pixels off
/// each row's button buys back two rows and change, which is what got the
/// tenth action in, and a key row is a wide target that loses nothing by not
/// being a tall one as well.
///
/// The display screen is not packed and gains nothing by it, but wears it
/// anyway: its rows are the same row — a name on the left, a control of the
/// same width on the right — and two screens one press apart under Options
/// whose rows stood at different heights would read as two hands.
pub(super) const ROW_BUTTON_PADDING: f32 = 6.0;

pub(super) fn padded_button(
    ink: &Palette,
    action: MenuButton,
    width: f32,
    pad: f32,
) -> impl Bundle {
    (
        Button,
        action,
        ink.button,
        Node {
            width: Val::Px(width),
            padding: UiRect::axes(Val::Px(12.0), Val::Px(pad)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(ink.button.idle),
        BorderColor::all(ink.edge),
    )
}

pub(super) fn button_label(ink: &Palette, text: &str) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(19.0),
            ..default()
        },
        TextColor(ink.text),
        TextLayout::justify(Justify::Center),
    )
}

pub(super) fn spawn_button(
    parent: &mut ChildSpawnerCommands,
    ink: &Palette,
    action: MenuButton,
    text: &str,
    width: f32,
) {
    parent
        .spawn(button(ink, action, width))
        .with_children(|button| {
            button.spawn(button_label(ink, text));
        });
}

/// What one notch of the wheel moves the controls screen by: about a key row,
/// so the list walks in the units it is made of. Trackpads report pixels of
/// screen rather than notches, and are answered in those — see
/// [`scroll_the_panel`].
pub(super) const SCROLL_NOTCH: f32 = 40.0;

/// The wheel, on the one screen that can be taller than the window.
///
/// No asking what the pointer is over: one panel on the screen is marked as
/// scrolling — see [`scrolling_panel`] — so the wheel can only mean it.
pub(super) fn scroll_the_panel(
    scroll: Res<AccumulatedMouseScroll>,
    mut panels: Query<(&mut ScrollPosition, &ComputedNode), With<ScrollingPanel>>,
) {
    if scroll.delta.y == 0.0 {
        return;
    }
    for (mut position, panel) in &mut panels {
        // A scroll position is written in the pixels the UI is laid out in,
        // and neither unit the wheel arrives in is one of those. A notch is
        // near enough already, being a key row and a key row being a laid-out
        // measurement; a trackpad's pixels are the screen's own, as many to a
        // laid-out one as the display's density and the UI's scale together
        // make — which is the number the layout has already worked out for
        // this panel.
        let step = match scroll.unit {
            MouseScrollUnit::Line => SCROLL_NOTCH,
            MouseScrollUnit::Pixel => panel.inverse_scale_factor,
        };
        // Rolled down reads further down the list. Both ends are held here,
        // the far one out of what the layout has just measured: the layout
        // clamps only the copy it draws from, and leaves this one to run on —
        // so a list overscrolled at the bottom would pile up travel that had
        // to be wound back through before anything moved.
        let last = (panel.content_size.y - panel.size.y + panel.scrollbar_size.y).max(0.0)
            * panel.inverse_scale_factor;
        position.0.y = (position.0.y - scroll.delta.y * step).clamp(0.0, last);
    }
}
