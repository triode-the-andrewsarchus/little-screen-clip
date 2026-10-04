# Overall
- Use https://github.com/Rust-GPU/rust-gpu to get rendering times down
- Make sure as much as possible is keyboard driven, like numbers to get to the tools in the image markup editor
- Refactor everything to not be pixel based
- FFMPEG Fate + PyPydf/sample-files + codec-corpus testing

# Big new features:
## Text
## Image Studio
## Audio Studio
* Resample / change sample rate
* '+' over sample to get next region of the same size, that way a lot of drums can be extracted quickly.
    * This requires some careful thinking about how to handle 0-crossing and user intent vs the obvious
    * We might need to store the original target transient point targets seprately from the actual selecting boundry
* Reverse support
* Speech Fix
    * DeepFilterNet3 — primary speech denoiser.
    * ClearerVoice/MossFormer2_SE_48K — heavyweight speech rescue.
    * Um remover?
* Repair
    * Cathar — general-purpose restoration workhorse: de-click, de-crackle, de-clip, de-plosive, de-hum, de-wind, etc.
    * audiojs/denoise — useful lightweight/browser-compatible implementations and a good source of conventional algorithms.
* Some sort of indication of stereo separation on stereo files
* remove DC offset
* reverse selection, entire track
  * Slightly different from HPF, since we don't want to remove low frequencies, just DC offset. This means something closer to how that ableton device does it.
* resamlping
* Microphone stream along with system audio stream
* STT (passive)
## MIDI
* Better sound font / multiple options?
* The stop button is lower than the play button
    * This makes me worry the correct button thing we made with safe contrast isn't being used for one of them
## PDF
* add epub support
* add CBX support
* convert epub <-> pdf
* Diff
## 3D file viewer
* open in blender doesn't work.
* More file support
* Some sort of false-color would be good.
## .ply/.splat preview
* The thing
* .ply, .spz
* .sog, .splat, .kspalt
## Video
* Basic preview
* grab selection to clip .mkv/.mp4 or .gif
## Diff (both binary and text)
* The colored background rendering vs the colored numbers can make it low contrast
* Show SHA1 of each file at top
## Color picker
* Lazy multi-color space color picker
* Should also be able to pick from screen (?)
## CSV++
* diff
## Node Machine
* Allow machine to be used for auto actions
* More nodes. So many more nodes.


## New Viewers:
* Pcap

# Future Goals:

- replace app launcher with per-task system with stupid fast navigation
    - could type app name, but the point here is you can type what you want to do instead, and then get a easy, normal, and power-user tool list
- Unfortunately, we probably need to re-invet the wheel and eat the file manager for this too
- Meta, we might be able to make MCP/AI stuff for integration
- Impliment some of the https://mahlet-inc.github.io/#displays visualizers

# very-future goals:

-? hardware controller integration - MIDI control?

## Super Calculator

- The calculator to end all calculators
    - Always saved history
    - Unit conversion
    - Computer algebra system



## File Manager

Conflict between computer using the FS and the user. Lean into that, make user pinning and organizing on top of it a thing.

The next phase of this project is a file manager application that integrates with the clipboard. Unlike the workbench UI, which is meant to be an ephermal popup for quick use, this is meant to fundamentally replace a normal file manager like Thunar or Nautilus.

Notably, I prefer the style of navigation like thunar's where typring letters immediately navigates to the match in the folder, not doing a recursive search or filtering

But with a few quirks.

First, we want to have a small preview of the existing clipboard slots. This will need a different render, as while on the big workbench it can take up ~half the screen, in the file manager it simply can't eat that much of the realestate. This should allow for dragging in a file/folder or selection of files/folders to a clipboard slot - fundamentally no different than hitting copy, except with the mouse action to drop it into a specific slot.

Second, it should be able to use all the same previews we've alredy made. This will allow for individual waveforms for audio files, picture thumbnails, midi thumbnails, etc.

Third, the existing small editors should be able to be invoked, but here they should default to saving to the directory they're opened to, asking if it should be a replace (save) or new-name (save-as)
- clipboard mirror with drag/drop at bottom (integration in my clipboard project)
- ALL of the file previews (actual waveforms for audio, etc.)
- Search local first, on match fail, start recursive searching (Just type to get what you want pretty often)
- Auto rules to be executed for some folders
  - Extract zips in DL's, move original zip to zips folder
    - Highlight if contents modified from ZIP
  - Prompt to rename (1) to current date, the original to the file date
- HOME page:
  - Recent from phone / Other devices (Syncthing)
  - Recent from downloads
- Better sort by's
  - If >90% are the same type, put the weird ones at the top/bottom, then have the better, type dependent preview view
  - Sort by image similarity, palette, etc.
    - Store a lot of this in a .sort-cache file
- Tree view
- Smart expansion - if a folder only contains 3 items or less, show it with an open icon and show the the contents to the right with a lighter background
  - Only 1? Put a folder icon on it, maybe color the bg different, but make it take up the same amount of space as any other item
- "Session" mode - mark things as unimportant/important to hide/reveal
- "AB sort mode" - color code things, run some sort of model that tries to learn to predict A/B sorting as the human marks more examples
- highlight if multiple selected items share the same hash
- Projects folder pinning, tags (projects get a new .fmproject which is globally .gitignored, home folder maintains a list of projects, their location, and a bool if they still exist. Also tracks moves.
  - TODO.md and README.md are displayed in tabs in project folders
- Because the computer sometimes needs to put things in specif places even if the user will never care, a .blacklist or .whitelist file should be able to be made of folders which sholudn't be rendered. If one of these is active, it sholud be obvious in the UI and easy to bypass, just like not showing dotfiles by default.
  - .undot should allow for showing some dot files even when dotfiles are otherwise hidden.
- fast filter search
  - ex: ">10MB"
  - "video"
  - ">10MB video"
  - https://github.com/killown/flux
- Columns view from https://github.com/euclio/fm
- https://filenq.app
- Git integration
- multi-file rename
- AI/OCR based content-aware rename
- folder rules
- automatic-actions per folder (use my existing graph system)
- clipboard as shelf
  - We might need to make it bigger?
- Paset clipboard as file
- transfer queue
- run copy/move on remote
- folder diff + merge
- usage analyzer
- dupe finder
- metadata editing
- git status