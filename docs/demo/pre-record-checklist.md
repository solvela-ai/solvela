# Pre-Record Checklist

Run through this list **before** hitting record. Skipping any of these will force a retake or, worse, ship a video with a flaw nobody noticed in editing.

## Environment — 30 minutes before

- [ ] **Gateway is running locally** — `cargo run -p gateway --release` listening on `:8402`. Confirm with `curl -s http://localhost:8402/pricing | jq '.models | length'` returning a number.
- [ ] **Postgres + Redis are up** — `docker compose ps` shows both healthy.
- [ ] **At least one provider key is configured** — `.env` has a working `OPENAI_API_KEY` (or whichever provider you'll route to). Test with one real call before recording.
- [ ] **Wallet has USDC** — at least 5 USDC on Solana mainnet. Demo will use ~0.001 USDC but you don't want a "Insufficient funds" error mid-take.
- [ ] **Wallet keypair is at the path the SDK expects** — `~/.solvela/wallet.json` (or update the script). Permissions `0600`.
- [ ] **Solana RPC is responsive** — `solana balance` returns in <2 seconds. If you're using a public RPC, switch to a paid one (Helius / Triton / QuickNode) for the recording session — public RPCs sometimes throttle and the txn confirmation will lag.
- [ ] **Network is stable** — close anything that might pop a notification mid-take (Slack, Mail, calendar, OS update prompts).

## Visuals — 15 minutes before

- [ ] **Terminal font:** JetBrains Mono or similar, **16pt minimum**, **18pt recommended** at 1080p. Anything smaller and viewers can't read it.
- [ ] **Terminal theme:** dark background. The Solvela accent color is `#F97316` (orange) — set the prompt color to match if you can. Otherwise stick with a neutral white prompt.
- [ ] **Terminal window size:** width that fits ~110 columns without wrapping the curl command. Test by pasting the longest command from `shot-list.md`.
- [ ] **Hide the OS dock/taskbar** — full screen the terminal and browser.
- [ ] **Disable browser extensions that show overlays** — ad blockers showing counts, password managers showing badges, dev tools panels. You want clean GitHub and Solana Explorer pages.
- [ ] **Browser tabs:** only the tabs the demo uses. Close everything else. (Acquirers do read tab titles in screenshots, even if they don't admit it.)
- [ ] **Browser zoom:** 125% on GitHub and Solana Explorer.
- [ ] **Display resolution:** if recording at 1080p, set the OS display to 1920×1080 or use OBS's downsample. Recording at native 4K and exporting 1080p produces a sharper result if your machine handles it.

## OBS / recording setup — 10 minutes before

- [ ] **Scenes set up:** `intro-terminal`, `demo-terminal`, `readme-arch`, `solana-explorer`, `arch-diagram-zoom`, `github-badges`, `final-card`. Each scene captures the appropriate window or display source.
- [ ] **Audio levels:** voice peaks at -12 to -6 dB. Test with a 5-second sample. If you're peaking at 0 dB you'll clip and ruin the voiceover.
- [ ] **Recording format:** MP4, H.264, 30 or 60 fps, CRF 18 (high quality master, you can compress later).
- [ ] **Disk space:** at least 5 GB free. A 90-second high-quality master is ~500 MB.
- [ ] **Hotkeys configured:** F1–F7 for scene switches. Practice the sequence once cold before the real take.

## Voice — 5 minutes before

- [ ] **Drink water.** Not coffee, not soda. Cold water relaxes the vocal cords.
- [ ] **Read the script aloud once.** Find the words that snag your tongue. Mark them and slow down on those.
- [ ] **Do not whisper-rehearse.** Speak at full volume so your warm-up matches your take volume.
- [ ] **Phone on Do Not Disturb.** Including watch notifications.
- [ ] **Close your office door.** A passing siren or a delivery driver's knock will end your take.

## The take

- [ ] Roll camera (start OBS recording) **before** speaking. Add a 3-second silence head and tail — easier to trim than to recover from a clipped first word.
- [ ] If you flub a line, **don't stop**. Pause for 2 seconds, restart that sentence cleanly. You'll cut to the clean restart in editing. Stopping and restarting the take wastes 5 minutes per attempt.
- [ ] After the final word, **hold still and silent** for 3 seconds. Then stop recording.
- [ ] **Watch the take immediately.** If anything is broken (audio noise, terminal cut off, network error visible), retake now while everything is still set up.

## After the take

- [ ] **Save raw recording with a clear name:** `solvela-demo-v1-take<N>-YYYY-MM-DD.mp4`.
- [ ] **Note the txn signature** from the demo's Solana payment — you'll want it for the description and for verification artifacts.
- [ ] **Back up the raw recording** to a second location before you start editing. Editors crash. Lost takes can't be reshot if you've already torn down the demo state.

---

If you've checked all of the above and you're sitting in front of a stable, ready, full-screen terminal and your voice feels warmed up — you're ready. Hit record.
