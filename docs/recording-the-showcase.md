# Recording the showcase session

The goal is a screen recording that proves the showcase video was
edited by an AI: the Claude session making the cuts on the left, the
Viode timeline growing live on the right, and the finished edit playing
at the end.

## Stage

Three windows on one screen (Omarchy/Hyprland):

- LEFT — the Claude Code app, with the viode MCP server connected.
- TOP-RIGHT — a terminal (ghostty), empty at first.
- BOTTOM-RIGHT — a terminal (ghostty), the command post.

Before recording: hide the bar (`omarchy toggle bar`) and enlarge fonts
until they look oversized in person (Ctrl+Plus in Claude Code,
Ctrl+Shift+Plus in each terminal) — that is what stays readable after
YouTube compression. Record a region, not the full monitor, on large
displays.

## Steps

1. BOTTOM-RIGHT — `omarchy screenrecord`, drag the crosshair over all
   three windows. Recording is on.
2. LEFT — type: `Run scenario 5 from docs/ai-editing.md` and press
   enter.
3. Wait. Watch LEFT until it says the project is created.
4. TOP-RIGHT — `cd ~/Videos/showcase && viode tui`. The TUI live-reloads
   as the AI edits; touch nothing.
5. Wait until LEFT says the render is finished.
6. TOP-RIGHT — press space. The finished edit plays inside the timeline.
7. BOTTOM-RIGHT — `omarchy screenrecord --stop-recording`, then
   `omarchy toggle bar`.

## Output

Two artifacts: the AI-edited video itself
(`~/Videos/showcase/renders/showcase-youtube.mp4`, already
loudness-normalized for upload) and the screen recording of the session
(in the Pictures/screen recordings directory). The recording is the
proof; the video is the product.
