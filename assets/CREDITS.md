# Credits

Where the files here came from, and what may be done with them.

## boat.glb

Original to this project, and under the same licence as the rest of it. Nothing
third-party went into it, so nothing here is owed to anybody.

`assets-src/boat.glb/` holds the Blender master; `assets-src/export.sh` builds
it, and is where the export settings the look depends on are written down.

The shape started as the placeholder the game used to build in code — ten
triangles and a spar — and was moved into a model file unchanged, so that the
first thing through the pipeline could be checked against a picture of the last
thing before it. It is meant to be taken further.

## palm.glb

Original to this project, and under the same licence as the rest of it. A
trunk of four segments and seven fronds, each frond a closed blade rather than
a flat leaf so that the whole model obeys the one rule the rest of the world
does about which way a face points.

`assets-src/palm.glb/` holds the Blender master. Where palms *stand* is not
here at all — that is the world's business, and is decided in `world`'s
`palms` module and sent to clients with the ground.

## menu.ogg

Cut from [Sailboat Bow](https://freesound.org/s/852108/) by myLoop, published on
Freesound under [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) —
a public domain dedication, so no attribution is owed and nothing here is a
condition of using it. Recorded at the bow of a boat under way.

The original runs 75 seconds and quietens markedly over its last third; this is
a stretch of it taken from where the water is at a steady state, crossfaded end
to end so that it loops.

`assets-src/menu.ogg/` holds what that was made from: the master, kept as FLAC
rather than as the WAV Freesound serves, and the script that cuts it, which is
where the numbers live. Freesound hands the master out only to an account, so a
clone could not fetch it back on its own — hence carrying it here.
