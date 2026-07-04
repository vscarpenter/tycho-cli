# tycho demo — render with:  vhs demo.tape   (produces demo.gif)
#
# Shows three reports against real ~/.claude/projects data. daily/cache/blocks
# never print project directory names, and the prompt is fixed to `$ `, so no
# paths, usernames, or cwd leak into the recording.

Output demo.gif

Set Shell "bash"
Set FontSize 13
Set Width 1280
Set Height 820
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 55ms

# Setup (not recorded): put the release binary on PATH, clean the prompt.
Hide
Type "export PATH=$PWD/target/release:$PATH; PS1='$ '; clear"
Enter
Show

Sleep 800ms

# 1. Per-day tokens and cost (recent days).
Type "tycho daily --since 2026-06-30"
Sleep 400ms
Enter
Sleep 3.5s

# 2. The flagship: what prompt caching saved you (all-time).
Type "tycho cache"
Sleep 400ms
Enter
Sleep 4s

# 3. Activity-anchored 5-hour billing blocks, with the active block projected.
Type "tycho blocks --since 2026-07-04"
Sleep 400ms
Enter
Sleep 4.5s

Sleep 1.5s
