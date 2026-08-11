# Credits

Where the files here came from, and what may be done with them.

## boat

Original to this project, and under the same licence as the rest of it. Nothing
third-party went into it, so nothing here is owed to anybody.

`assets-src/models/boat/` holds the Blender master; `assets-src/models/export.sh`
builds it, and is where the export settings the look depends on are written
down.

The shape started as the placeholder the game used to build in code — ten
triangles and a spar — and was moved into a model file unchanged, so that the
first thing through the pipeline could be checked against a picture of the last
thing before it. It is meant to be taken further.

## palm

Original to this project, and under the same licence as the rest of it. A
trunk of four segments and seven fronds, each frond a closed blade rather than
a flat leaf so that the whole model obeys the one rule the rest of the world
does about which way a face points.

`assets-src/models/palm/` holds the Blender master. Where palms *stand* is not
here at all — that is the world's business, and is decided in `world`'s
`palms` module and sent to clients with the ground.

## eagle

Original to this project, and under the same licence as the rest of it. A
soaring silhouette — body, two kinked wings and a fanned tail, every sheet a
closed solid so the model obeys the same winding rule everything else does.

`assets-src/models/eagle/` holds the Blender master and the notes on what the
shape is for. Where eagles *fly* is the game's own business, decided
client-side in `game`'s `wildlife` module — nothing about them crosses the
wire.

## dolphin

Original to this project, and under the same licence as the rest of it. A
lofted hex-ring body with dorsal fin, horizontal flukes and pectorals as thin
closed sheets.

`assets-src/models/dolphin/` holds the Blender master and the notes. Like the
eagle, where pods swim is decided client-side in `game`'s `wildlife` module.

## seabird

Original to this project, and under the same licence as the rest of it. A
gliding silhouette with drooped wingtips and a long bill, seen only as one of
a line skimming the shallows.

`assets-src/models/seabird/` holds the Blender master and the notes. Like all
wildlife, where lines fly is decided client-side in `game`'s `wildlife`
module.

## whale

Original to this project, and under the same licence as the rest of it. A
rorqual — lofted hex-ring body, small dorsal fin far aft, broad horizontal
flukes — built for the one view a whale gets: a back through the surface.

`assets-src/models/whale/` holds the Blender master and the notes. Like all
wildlife, where whales swim is decided client-side in `game`'s `wildlife`
module.

## menu-loop

Cut from [Sailboat Bow](https://freesound.org/s/852108/) by myLoop, published on
Freesound under [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) —
a public domain dedication, so no attribution is owed and nothing here is a
condition of using it. Recorded at the bow of a boat under way.

The original runs 75 seconds and quietens markedly over its last third; this is
a stretch of it taken from where the water is at a steady state, crossfaded end
to end so that it loops.

`assets-src/audio/menu-loop/` holds what that was made from: the master, kept as FLAC
rather than as the WAV Freesound serves, and the script that cuts it, which is
where the numbers live. Freesound hands the master out only to an account, so a
clone could not fetch it back on its own — hence carrying it here.
