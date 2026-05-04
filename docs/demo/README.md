# Solvela Demo Video — Production Kit

This directory contains everything needed to record the canonical 90-second Solvela demo video.

## Files

| File | Purpose |
|---|---|
| [`script.md`](./script.md) | The 90-second voiceover script with timestamps, on-screen text, and shot list |
| [`shot-list.md`](./shot-list.md) | Detailed shot-by-shot breakdown with terminal commands and screen targets |
| [`pre-record-checklist.md`](./pre-record-checklist.md) | Everything to set up before hitting record (gateway running, wallet funded, terminal styled, etc.) |
| [`b-roll.md`](./b-roll.md) | Optional supplementary footage to capture (architecture animations, code zoom-ins, Solana Explorer) |

## Recording targets

- **Length:** 90 seconds. Hard cap 105 seconds. Anything longer loses acquihire-evaluator attention.
- **Aspect ratio:** 16:9 at 1080p minimum (1920×1080). Record at 60fps if your machine handles it; 30fps is acceptable.
- **Audio:** Voiceover only. No music bed for the v1 cut — music is a polish pass after the cut works.
- **Format:** MP4 (H.264) for upload. Master in your editor's native format, export MP4 last.

## Recommended tooling

| Need | Tool | Why |
|---|---|---|
| Screen capture | **OBS Studio** (free) | Reliable, scriptable scene switching, separate audio tracks |
| Alternative | **Loom** | One-click record/share if OBS feels heavy |
| Terminal styling | iTerm2 (mac) / Windows Terminal | Set font to JetBrains Mono 16pt+, dark theme, matching Solvela orange (`#F97316`) accent |
| Voiceover mic | Anything decent — phone earbuds beat laptop mic | Listeners forgive okay video, never bad audio |
| Editing | DaVinci Resolve (free) or CapCut Desktop | Both handle cuts + lower-thirds + export |
| Upload host | YouTube (unlisted) **plus** raw MP4 in a GitHub release | YouTube embed in README; raw file for grant/acquirer offline review |

## Distribution checklist (after recording)

- [ ] Upload to YouTube as **unlisted** (not private — grant officers can't see private videos without a Google login)
- [ ] Add the embed to `README.md` directly under the project tagline
- [ ] Add a thumbnail image (`docs/demo/thumbnail.png`) — the YouTube auto-thumbnail is almost always wrong
- [ ] Attach the raw MP4 to a GitHub release (tag it `demo-v1`) so it survives YouTube link rot
- [ ] Drop the link into `docs/exit-readiness.md` under the "Demo materials" section
- [ ] Drop the link into `docs/grants/` application templates
- [ ] Tweet/post the link with the GitHub link as the second line

## Why 90 seconds

Acquirers, grant evaluators, and prospective enterprise buyers all share one behavior: they decide whether to read the README in the first 60 seconds of contact with a project. A 90-second video gives them:

- **0–15s:** the hook (what problem this solves)
- **15–60s:** proof it works (live demo of the actual product)
- **60–90s:** what makes it defensible (the architecture and traction signal)

Anything longer than 105 seconds and they bounce. Anything shorter than 60 and they don't believe the demo is real. 90 is the sweet spot.
