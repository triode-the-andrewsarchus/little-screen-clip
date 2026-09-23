# Overall
- Use https://github.com/Rust-GPU/rust-gpu to get rendering times down
- Make sure as much as possible is keyboard driven, like numbers to get to the tools in the image markup editor
- Refactor everything to not be pixel based

# Big new features:
## Text
## Image Studio
* Depth-map driven contrast/saturation/brightness adjustment
* mask zoom
## Audio Studio
* Resample / change sample rate
* Channel operations — stereo→mono, extract L/R, swap L/R, invert polarity, perhaps copy L→R/R→L.
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
## MIDI
* Better sound font / multiple options?
* The stop button is lower than the play button
    * This makes me worry the correct button thing we made with safe contrast isn't being used for one of them
## PDF
* add epub support
* add CBX support
* convert epub <-> pdf
## 3D file viewer
* open in blender doesn't work.
* More file support
* Some sort of false-color would be good.
## .ply/.splat preview
* The thing
## Video
* Basic preview
* grab selection to clip .mkv/.mp4 or .gif
## Diff (both binary and text)
* The colored background rendering vs the colored numbers can make it low contrast
- Show SHA1 of each file at top
## Color picker
- Lazy multi-color space color picker
- Should also be able to pick from screen (?)
## Node machine
- Allow machine to be used for auto actions
- More nodes. So many more nodes.


# future goals:

- replace app launcher with per-task system with stupid fast navigation
    - could type app name, but the point here is you can type what you want to do instead, and then get a easy, normal, and power-user tool list
- Unfortunately, we probably need to re-invet the wheel and eat the file manager for this too
- Meta, we might be able to make MCP/AI stuff for integration
- Impliment some of the https://mahlet-inc.github.io/#displays visualizers

# very-future goals:

- Replace file manager
    - Filemanager: Conflict between computer using the FS and the user. Lean into that, make user pinning and organizing on top of it a thing.
- The calculator to end all calculators
    - Always saved history
    - Unit conversion
    - Computer algebra system
- hardware controller integration - MIDI control?


# File Manager

The next phase of this project is a file manager application that integrates with the clipboard. Unlike the workbench UI, which is meant to be an ephermal popup for quick use, this is meant to fundamentally replace a normal file manager like Thunar or Nautilus.

Notably, I prefer the style of navigation like thunar's where typring letters immediately navigates to the match in the folder, not doing a recursive search or filtering

But with a few quirks.

First, we want to have a small preview of the existing clipboard slots. This will need a different render, as while on the big workbench it can take up ~half the screen, in the file manager it simply can't eat that much of the realestate. This should allow for dragging in a file/folder or selection of files/folders to a clipboard slot - fundamentally no different than hitting copy, except with the mouse action to drop it into a specific slot.

Second, it should be able to use all the same previews we've alredy made. This will allow for individual waveforms for audio files, picture thumbnails, midi thumbnails, etc.

Third, the existing small editors should be able to be invoked, but here they should default to saving to the directory they're opened to, asking if it should be a replace (save) or new-name (save-as)

Fourth, 