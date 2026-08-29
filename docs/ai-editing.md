# Editing video with an AI

Viode's MCP server gives an AI the same editing verbs a human has, plus
senses: it can look at frames, read waveforms and scopes, find silences
and scene changes, and check its own work with preview renders. This
document is a hands-on scenario you can run yourself. It uses the free
test media from the README benchmarks, but any footage works.

## Setup

Start a Claude Code session inside the Viode repository (the `.mcp.json`
there registers the server automatically), or add the server to any MCP
client with `viode serve --mcp`. Approve the `viode` server when asked.
That is the entire setup.

## Scenario 1 — the dead-air remover

This is the one-sentence workflow that sells the whole idea. Say:

> Create a project called `episode` in ~/Videos, add
> ~/Videos/viode-test-media/his_girl_friday.mp4, and cut out every
> silence longer than a second. Tell me how much time you removed.

The AI creates the project, runs silence detection over the full film,
cuts the gaps with padding so speech stays natural, and reports the
saved minutes. On the benchmark machine the analysis of 92 minutes of
dialogue takes about nine seconds.

## Scenario 2 — the AI actually looks

Ask for something that requires eyes:

> Find the scene changes in the first ten minutes, grab a frame from
> each scene, look at them, and keep only the three most visually
> striking scenes on the timeline. Explain your choices.

The `frame_grab` tool returns real images, so the AI's choices are
judgments about pictures, not guesses about timestamps. You can argue
with its taste, which is the point.

## Scenario 3 — the multicam sync

> Add ~/Videos/viode-test-media/sita_1080p.mp4 as a second camera
> angle, and cut to it for ten seconds somewhere in the middle.

The angle syncs by audio cross-correlation and lands as a disabled
track; the `take` swaps the synced footage onto the main timeline. No
clap slate, no manual nudging.

## Scenario 4 — finish like a professional

> Add a lower-third title with my name at the start, grade the opening
> shot black and white, and export a loudness-normalized version for
> YouTube and an audio-only version for podcast feeds.

Titles, color, and the two-pass EBU R128 presets are all tools the AI
holds directly.

## Scenario 5 — the showcase video (the full brief)

This is a complete editorial brief for a ~40-second YouTube piece whose
message is the method: every cut in it is made by the AI through MCP
tools. Connect the server, then hand the AI this brief verbatim.

> Create a project called `showcase` (1920x1080, 30fps) in ~/Videos.
> The media lives in ~/Videos/viode-test-media/. Build this timeline:
>
> 1. Cold open: the rapid-dialogue stretch of his_girl_friday.mp4 around
>    04:04-04:10 (verify it with silence detection first — pick a run
>    with no gaps). Overlay the title "This video was edited entirely by
>    an AI" for the first four seconds.
> 2. A second dialogue burst from around 08:29, crossfaded in, with the
>    lower-third "It cut the silences out of this 1940 dialogue".
> 3. A three-shot montage from sita_1080p.mp4 cut EXACTLY on scene
>    changes you detect near 31:00-31:30 — wipe into it, and caption it
>    "It found these cuts by watching the film". Grab a frame first and
>    look at it to confirm the section is visually strong.
> 4. Four seconds of bbb_4k.mp4 from ~00:52 with a second BBB moment
>    (~01:15) placed as picture-in-picture in the top-right corner at
>    about quarter size. Caption: "4K, with a second angle it placed
>    itself".
> 5. Two seconds from ~01:24 of bbb_4k.mp4 at half speed. Caption:
>    "Slow motion: one command".
> 6. The same Sita shot (~31:30) twice back to back: first graded fully
>    black and white, then in color. Caption: "It graded this shot.
>    Then changed its mind."
> 7. End on five seconds of black (generate it if needed) with three
>    SEQUENTIAL titles: "No hands touched a timeline.", then "Viode —
>    the AI-native video editor for Linux", then
>    "github.com/edimoldovan/viode".
>
> Check your work with frame grabs at the tricky points (the PiP shot,
> the grade pair), then export with the YouTube loudness preset and
> tell me the output path.

Everything in the brief maps to tools the AI already holds: silence and
scene detection, frame grabs it can see, clip placement, grading, speed,
titles, and preset exports.

## What makes this different from a plugin

The AI is not driving a GUI with a fake mouse. It speaks the same
protocol as every other Viode client, edits the same plain-TOML project
file you can open in an editor, and every change it makes is a line in
`git diff`. You review an AI edit the way you review a pull request —
and revert it the same way.

## The proof

The showcase video in this repository's README was edited entirely this
way: an AI analyzed the footage, chose the moments by looking at frames,
and assembled every cut, title, grade, and export with the same tools
described above.
