# The rowing boat

Why the shape is what it is; the numbers are in the file. Four metres of open
boat against the ship's seven — a working skiff, rowed rather than sailed, and
the small end of what a player gets about in.

- **An open boat, so it is two shells and not one.** The ship is a closed hull
  with a deck on top and nothing to see inside it; this one is looked *into*
  from a camera forty metres up, and the inside is most of what is looked at.
  So there is an outer skin, an inner one, and a flat gunwale capping the two
  together — which is also the only place the planking's thickness shows.
- **The sole stands above the waterline.** This is the rule the proportions
  are bent around, and it is not a mistake to be corrected towards realism:
  the sea is one unbroken surface drawn straight through everything (see
  `crates/game/src/sea.rs`), so an open boat with its floor where a real one
  has it is a boat with the sea standing inside it. Eight centimetres of
  freeboard on the sole is enough for the swell of an ordinary day; a hard
  enough blow will still slop through, which is a fair thing for it to do.
- **That is what makes it deep for its length.** Sole above the water, a
  thwart to sit on above the sole, and a gunwale above that, and the boat is
  two thirds of a metre from keel to rail. Lengthening it to four metres is
  what buys those proportions back — shorter, and the same interior makes a
  tub.
- **The widest point is a little abaft amidships**, the bow fine and the
  transom wide, so which way it is pointing reads from straight overhead. The
  sheer springs up to the stem and rises again at the transom.
- **A shallow V with a chine**, like the ship's, so the same grounding rule
  fits it: the keel line is the whole of what touches, and nothing is carried
  flat out to the beam that would need probing on its own.
- **Three thwarts.** The middle one is the rowing thwart and the two others
  are where the rest of the boat's business happens — but between them they
  are also what a shipped oar lies on, which is what fixes where they are.
- **The rowlocks are blocks on the gunwale, abaft the rowing thwart** by about
  the length of a forearm. That is not decoration: it is what puts the handles
  at the rower's chest at the finish and out at arm's length at the catch. Move
  the thwart or the rowlocks and the rower's reach goes with them.

## The colours

The ship's four, in different proportions, so the two read as one fleet — see
`../boat/NOTES.md`, where they are picked. Timber for the topsides and the
transom, the darker timber below the chine, scrubbed deck for the whole
interior, and bare spar cream for the gunwale, the thwarts and the looms.

The blades are timber rather than cream, which is the one choice made here
rather than borrowed: from overhead the stroke is two dark tips swinging past
the hull, and a blade shipped inboard is a dark shape lying on a pale one
instead of cream on cream.

## The two clips

`stowed` is one keyframe: the oars in, lying fore and aft across the stern
seat and the rowing thwart, blades flat and forward. `stroke` is one full
cycle — the catch, the drive, the blades lifted and feathered for the
recovery, squared again at the top. It closes, first frame matching last.

**The oars pivot at the rowlock**, which is why the model is cut with them
out and shipping them is the pose that moves: an oar turns about the crutch it
sits in and nowhere else, so the stroke is a rotation and nothing more. The
drive is the shorter half of the cycle, the way a stroke is.

**The stroke is meant to be seeked rather than played**, exactly as the
figure's run is — by ground covered, so the blades bite at whatever speed the
boat is making and a boat backing water runs the cycle backwards. That needs
one number the file cannot carry: how far one stroke drives the boat, which is
a little more than its own length. See `crates/game/src/figure.rs` for the
arrangement.

The two are **states and not a dial**. Crossfading them sweeps the looms
through the gunwale, which is what shipping the oars looks like and is fine
taken briskly, but nothing should be left standing halfway.

## What the game will want

Nothing loads this yet. When something does, the numbers it has to agree with
are the ones a `Hull` in `crates/game/src/boat.rs` names — length, beam, the
keel's depth and the stations it runs between, and where somebody aboard sits
rather than stands, which here is the rowing thwart amidships. They are the
model's to give and the game's to be pinned to, the way the ship's are.
